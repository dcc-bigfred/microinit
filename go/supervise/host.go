package supervise

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/dcc-bigfred/microinit/go/client"
	"github.com/dcc-bigfred/microinit/go/config"
)

const (
	defaultReadyTimeout    = 10 * time.Second
	defaultShutdownTimeout = 15 * time.Second
	// hardKillGrace is how long to wait after SIGTERM to the process group
	// before escalating to SIGKILL, within the overall ShutdownTimeout budget.
	hardKillGrace = 5 * time.Second
	// spawnLogCap bounds the captured stdout/stderr of the spawned microinit
	// process so a chatty daemon cannot exhaust memory on embedded hosts.
	spawnLogCap = 256 * 1024
)

// Host joins or spawns a microinit supervise instance.
type Host struct {
	Socket     string
	Bin        string
	ConfigPath string
	// DropinDir is created during EnsureRunning when set (microinit loads it
	// from the config directory layout; callers still write drop-ins themselves).
	DropinDir string
	// PidFile is written after a successful spawn (pid + /proc starttime) so
	// embedders can reap orphans safely. Empty defaults to <socket-dir>/microinit.pid.
	PidFile string

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
// The spawned process is NOT tied to ctx: cancelling ctx triggers a graceful
// SIGTERM (soft-kill) so microinit can stop its services, then escalates to
// SIGKILL only after ShutdownTimeout. This prevents orphaned managed
// processes (redis, dcc-bus, …) when the embedding process receives SIGTERM.
//
// joined is true when an already-running daemon was used (this Host must not
// Shutdown that process).
func (h *Host) EnsureRunning(ctx context.Context) (joined bool, err error) {
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

	logBuf := newBoundedBuffer(spawnLogCap)
	// exec.Command (not CommandContext): ctx cancellation is handled below as a
	// graceful SIGTERM so microinit can stop managed services before exit.
	cmd := exec.Command(h.Bin, "--socket", h.Socket, "supervise", "--config", h.ConfigPath)
	cmd.Stdout = logBuf
	cmd.Stderr = logBuf
	// Run microinit in its own process group so a SIGTERM to the group reaches
	// the daemon and (optionally) its tracked children, not the embedder.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		return false, fmt.Errorf("start microinit (%s --socket %s supervise --config %s): %w", h.Bin, h.Socket, h.ConfigPath, err)
	}
	if err := h.writePidFile(cmd.Process.Pid); err != nil {
		_ = signalProcessGroup(cmd, syscall.SIGKILL)
		_, _ = cmd.Process.Wait()
		return false, fmt.Errorf("write microinit pid file: %w", err)
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
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			return false, fmt.Errorf("microinit startup cancelled: %w", h.terminateSoft(ctx, cmd, waitCh, logBuf))
		case waitErr := <-waitCh:
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			h.removePidFile()
			detail := strings.TrimSpace(logBuf.String())
			if detail == "" {
				detail = fmt.Sprintf("exit: %v", waitErr)
			}
			return false, fmt.Errorf("microinit exited before ready (bin=%s socket=%s config=%s): %s", h.Bin, h.Socket, h.ConfigPath, detail)
		case <-deadline.C:
			h.spawned, h.cmd, h.waitCh = false, nil, nil
			return false, fmt.Errorf("microinit did not become ready within %s (bin=%s socket=%s config=%s): %s", ready, h.Bin, h.Socket, h.ConfigPath, h.softKillDetail(cmd, waitCh, logBuf))
		case <-ticker.C:
		}
	}
}

// terminateSoft sends SIGTERM to the spawned microinit process group and waits
// for it to exit gracefully (up to ShutdownTimeout), then escalates to SIGKILL.
// Used when ctx is cancelled during startup.
func (h *Host) terminateSoft(ctx context.Context, cmd *exec.Cmd, waitCh <-chan error, logBuf *boundedBuffer) error {
	timeout := h.shutdownTimeout()
	_ = signalProcessGroup(cmd, syscall.SIGTERM)
	select {
	case <-waitCh:
		h.removePidFile()
		return ctx.Err()
	case <-time.After(timeout):
		_ = signalProcessGroup(cmd, syscall.SIGKILL)
		<-waitCh
		h.removePidFile()
		return fmt.Errorf("%w (microinit did not exit after SIGTERM within %s)", ctx.Err(), timeout)
	}
}

// softKillDetail SIGTERM-waits then SIGKILLs the process group and returns the captured log.
func (h *Host) softKillDetail(cmd *exec.Cmd, waitCh <-chan error, logBuf *boundedBuffer) string {
	timeout := h.shutdownTimeout()
	_ = signalProcessGroup(cmd, syscall.SIGTERM)
	select {
	case <-waitCh:
	case <-time.After(timeout):
		_ = signalProcessGroup(cmd, syscall.SIGKILL)
		<-waitCh
	}
	h.removePidFile()
	detail := strings.TrimSpace(logBuf.String())
	if detail == "" {
		detail = fmt.Sprintf("killed after %s", timeout)
	}
	return detail
}

// Shutdown stops the microinit process only if this Host spawned it.
// When EnsureRunning joined an existing daemon, this is a no-op.
// Callers that need to stop their own services must do so themselves first.
//
// Sequence within ShutdownTimeout (default 15s): IPC halt → wait → SIGTERM to
// the process group → wait remaining budget (capped) → SIGKILL to the group.
func (h *Host) Shutdown(ctx context.Context) error {
	h.mu.Lock()
	spawned, cmd, waitCh := h.spawned, h.cmd, h.waitCh
	h.mu.Unlock()
	if !spawned || cmd == nil || cmd.Process == nil {
		return nil
	}
	timeout := h.shutdownTimeout()
	deadline := time.Now().Add(timeout)
	if dl, ok := ctx.Deadline(); ok && dl.Before(deadline) {
		deadline = dl
	}

	// Ask microinit to stop its services and exit cleanly.
	_ = h.client.Shutdown()
	if waitCh == nil {
		done := make(chan error, 1)
		go func() { done <- cmd.Wait() }()
		waitCh = done
	}
	remaining := time.Until(deadline)
	if remaining < 0 {
		remaining = 0
	}
	select {
	case <-ctx.Done():
		h.terminateHard(cmd, waitCh, time.Until(deadline))
		h.clearSpawn()
		return ctx.Err()
	case <-time.After(remaining):
		h.terminateHard(cmd, waitCh, hardKillGrace)
		h.clearSpawn()
		return fmt.Errorf("microinit shutdown timed out (socket %s)", h.Socket)
	case <-waitCh:
	}
	h.clearSpawn()
	return nil
}

func (h *Host) terminateHard(cmd *exec.Cmd, waitCh <-chan error, grace time.Duration) {
	_ = signalProcessGroup(cmd, syscall.SIGTERM)
	if grace <= 0 {
		grace = hardKillGrace
	}
	select {
	case <-waitCh:
	case <-time.After(grace):
		_ = signalProcessGroup(cmd, syscall.SIGKILL)
		<-waitCh
	}
}

// signalProcessGroup delivers sig to the process group of cmd (Setpgid).
// Falls back to signaling the process itself when the group kill fails.
func signalProcessGroup(cmd *exec.Cmd, sig syscall.Signal) error {
	if cmd == nil || cmd.Process == nil {
		return nil
	}
	pid := cmd.Process.Pid
	if err := syscall.Kill(-pid, sig); err != nil {
		return cmd.Process.Signal(sig)
	}
	return nil
}

func (h *Host) pidFilePath() string {
	if h.PidFile != "" {
		return h.PidFile
	}
	dir := filepath.Dir(h.Socket)
	if dir == "" || dir == "." {
		dir = "."
	}
	return filepath.Join(dir, "microinit.pid")
}

func (h *Host) writePidFile(pid int) error {
	path := h.pidFilePath()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	starttime, err := procStartTime(pid)
	if err != nil {
		// Best-effort: still write the PID so orphan cleanup has something.
		starttime = 0
	}
	content := fmt.Sprintf("%d\n%d\n", pid, starttime)
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, []byte(content), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (h *Host) removePidFile() {
	_ = os.Remove(h.pidFilePath())
}

// procStartTime reads field 22 (starttime) from /proc/<pid>/stat.
func procStartTime(pid int) (uint64, error) {
	data, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0, err
	}
	// Format: pid (comm) state ppid ... — comm may contain spaces/parens.
	s := string(data)
	idx := strings.LastIndex(s, ") ")
	if idx < 0 {
		return 0, fmt.Errorf("parse /proc/%d/stat: no comm", pid)
	}
	fields := strings.Fields(s[idx+2:])
	// After ") ": field[0]=state (stat field 3). starttime is field 22 → index 19.
	if len(fields) < 20 {
		return 0, fmt.Errorf("parse /proc/%d/stat: too few fields", pid)
	}
	var starttime uint64
	_, err = fmt.Sscanf(fields[19], "%d", &starttime)
	return starttime, err
}

func (h *Host) shutdownTimeout() time.Duration {
	if h.ShutdownTimeout > 0 {
		return h.ShutdownTimeout
	}
	return defaultShutdownTimeout
}

func (h *Host) clearSpawn() {
	h.removePidFile()
	h.mu.Lock()
	h.spawned, h.cmd, h.waitCh = false, nil, nil
	h.mu.Unlock()
}

// ForwardSignals subscribes the Host to SIGTERM/SIGINT on the embedder
// process and triggers a graceful Shutdown when either arrives. Call once
// after EnsureRunning when the embedder wants OS signals to tear down
// microinit (e.g. loco-server). The returned stop function restores the
// default signal behavior; call it before a subsequent EnsureRunning.
func (h *Host) ForwardSignals() (stop func()) {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
	done := make(chan struct{})
	go func() {
		select {
		case <-sigCh:
			_ = h.Shutdown(context.Background())
		case <-done:
		}
	}()
	return func() {
		signal.Stop(sigCh)
		close(done)
	}
}

// boundedBuffer is a small io.Writer that keeps at most cap bytes of the most
// recent output (ring-style: once full, new writes overwrite the oldest).
// Used to capture spawn diagnostics without unbounded memory growth.
type boundedBuffer struct {
	cap  int
	buf  []byte
	pos  int
	full bool
	mu   sync.Mutex
}

func newBoundedBuffer(cap int) *boundedBuffer {
	return &boundedBuffer{cap: cap, buf: make([]byte, 0, cap)}
}

func (b *boundedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	n := len(p)
	for len(p) > 0 {
		free := b.cap - len(b.buf)
		if b.full {
			free = 0
		}
		if free > len(p) {
			free = len(p)
		}
		if free > 0 {
			b.buf = append(b.buf, p[:free]...)
			p = p[free:]
		}
		if len(p) == 0 {
			break
		}
		// Buffer full: wrap around and overwrite oldest.
		copied := copy(b.buf[b.pos:], p)
		b.pos = (b.pos + copied) % b.cap
		p = p[copied:]
		b.full = true
	}
	return n, nil
}

func (b *boundedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	if !b.full {
		return string(b.buf)
	}
	return string(b.buf[b.pos:]) + string(b.buf[:b.pos])
}

// Ensure interface compliance.
var _ io.Writer = (*boundedBuffer)(nil)
