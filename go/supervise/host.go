package supervise

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/dcc-bigfred/microinit/go/client"
	"github.com/dcc-bigfred/microinit/go/config"
)

const (
	defaultReadyTimeout    = 10 * time.Second
	defaultShutdownTimeout = 15 * time.Second
)

// Host joins or spawns a microinit supervise instance.
type Host struct {
	Socket     string
	Bin        string
	ConfigPath string
	// DropinDir is created during EnsureRunning when set (microinit loads it
	// from the config directory layout; callers still write drop-ins themselves).
	DropinDir string

	// ReadyTimeout waits for IPC after spawn (default 10s).
	ReadyTimeout time.Duration
	// ShutdownTimeout waits after Shutdown IPC (default 15s).
	ShutdownTimeout time.Duration

	client  *client.Client
	spawned bool
	cmd     *exec.Cmd
	waitCh  <-chan error
	mu      sync.Mutex
}

// New returns a Host. Empty Socket defaults to client.DefaultSocket; empty Bin
// defaults to "microinit".
func New(socket, bin, configPath, dropinDir string) *Host {
	if socket == "" {
		socket = client.DefaultSocket
	}
	if bin == "" {
		bin = "microinit"
	}
	return &Host{
		Socket:     socket,
		Bin:        bin,
		ConfigPath: configPath,
		DropinDir:  dropinDir,
		client:     &client.Client{Socket: socket},
	}
}

// Client returns the IPC client bound to this host's socket.
func (h *Host) Client() *client.Client { return h.client }

// Spawned reports whether this Host started the microinit process.
func (h *Host) Spawned() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.spawned
}

// EnsureRunning joins an existing microinit when its socket responds.
// Otherwise it launches one `microinit supervise` and waits for IPC.
//
// joined is true when an already-running daemon was used (this Host must not
// Shutdown that process).
func (h *Host) EnsureRunning(ctx context.Context) (joined bool, err error) {
	if _, err := h.client.List(); err == nil {
		return true, nil
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, err := h.client.List(); err == nil {
		return true, nil
	}
	if h.spawned && h.cmd != nil && h.cmd.Process != nil {
		return false, fmt.Errorf("microinit process is running but IPC is unavailable (socket %s)", h.Socket)
	}
	if h.DropinDir != "" {
		if err := os.MkdirAll(h.DropinDir, 0o755); err != nil {
			return false, fmt.Errorf("create microinit drop-in dir %s: %w", h.DropinDir, err)
		}
	}
	if h.ConfigPath != "" {
		if err := os.MkdirAll(filepath.Dir(h.ConfigPath), 0o755); err != nil {
			return false, fmt.Errorf("create microinit config dir %s: %w", filepath.Dir(h.ConfigPath), err)
		}
	}
	if sockDir := filepath.Dir(h.Socket); sockDir != "" && sockDir != "." {
		if err := os.MkdirAll(sockDir, 0o755); err != nil {
			return false, fmt.Errorf("create microinit socket dir %s: %w", sockDir, err)
		}
	}
	if h.ConfigPath != "" {
		if _, err := os.Stat(h.ConfigPath); os.IsNotExist(err) {
			content, marshalErr := json.Marshal(map[string]any{"services": []any{}, "socket": h.Socket})
			if marshalErr != nil {
				return false, marshalErr
			}
			if err := config.WriteFileAtomically(h.ConfigPath, append(content, '\n')); err != nil {
				return false, fmt.Errorf("write microinit config %s: %w", h.ConfigPath, err)
			}
		} else if err != nil {
			return false, fmt.Errorf("stat microinit config %s: %w", h.ConfigPath, err)
		}
	}
	if h.ConfigPath == "" {
		return false, fmt.Errorf("ConfigPath is required to spawn microinit")
	}

	var logBuf bytes.Buffer
	cmd := exec.CommandContext(ctx, h.Bin, "--socket", h.Socket, "supervise", "--config", h.ConfigPath)
	cmd.Stdout = &logBuf
	cmd.Stderr = &logBuf
	if err := cmd.Start(); err != nil {
		return false, fmt.Errorf("start microinit (%s --socket %s supervise --config %s): %w", h.Bin, h.Socket, h.ConfigPath, err)
	}
	waitCh := make(chan error, 1)
	go func() { waitCh <- cmd.Wait() }()
	h.cmd, h.spawned, h.waitCh = cmd, true, waitCh

	ready := h.ReadyTimeout
	if ready <= 0 {
		ready = defaultReadyTimeout
	}
	deadline := time.NewTimer(ready)
	defer deadline.Stop()
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()
	for {
		if _, err := h.client.List(); err == nil {
			return false, nil
		}
		select {
		case <-ctx.Done():
			_ = cmd.Process.Kill()
			<-waitCh
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			return false, ctx.Err()
		case waitErr := <-waitCh:
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			detail := strings.TrimSpace(logBuf.String())
			if detail == "" {
				detail = fmt.Sprintf("exit: %v", waitErr)
			}
			return false, fmt.Errorf("microinit exited before ready (bin=%s socket=%s config=%s): %s", h.Bin, h.Socket, h.ConfigPath, detail)
		case <-deadline.C:
			_ = cmd.Process.Kill()
			<-waitCh
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			detail := strings.TrimSpace(logBuf.String())
			if detail == "" {
				detail = "no output captured"
			}
			return false, fmt.Errorf("microinit did not become ready within %s (bin=%s socket=%s config=%s): %s", ready, h.Bin, h.Socket, h.ConfigPath, detail)
		case <-ticker.C:
		}
	}
}

// Shutdown stops the microinit process only if this Host spawned it.
// When EnsureRunning joined an existing daemon, this is a no-op.
// Callers that need to stop their own services must do so themselves first.
func (h *Host) Shutdown(ctx context.Context) error {
	h.mu.Lock()
	spawned, cmd, waitCh := h.spawned, h.cmd, h.waitCh
	h.mu.Unlock()
	if !spawned || cmd == nil || cmd.Process == nil {
		return nil
	}
	_ = h.client.Shutdown()
	if waitCh == nil {
		done := make(chan error, 1)
		go func() { done <- cmd.Wait() }()
		waitCh = done
	}
	timeout := h.ShutdownTimeout
	if timeout <= 0 {
		timeout = defaultShutdownTimeout
	}
	select {
	case <-ctx.Done():
		_ = cmd.Process.Kill()
		<-waitCh
		h.clearSpawn()
		return ctx.Err()
	case <-time.After(timeout):
		_ = cmd.Process.Kill()
		<-waitCh
		h.clearSpawn()
		return fmt.Errorf("microinit shutdown timed out (socket %s)", h.Socket)
	case <-waitCh:
	}
	h.clearSpawn()
	return nil
}

func (h *Host) clearSpawn() {
	h.mu.Lock()
	h.spawned, h.cmd, h.waitCh = false, nil, nil
	h.mu.Unlock()
}
