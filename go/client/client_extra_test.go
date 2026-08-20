package client_test

import (
	"encoding/binary"
	"errors"
	"io"
	"net"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/dcc-bigfred/microinit/go/client"
)

// TestReadFrameTooLarge verifies a server claiming a frame larger than
// maxFrameBytes is rejected without allocating the full buffer (3.4).
func TestReadFrameTooLarge(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		// Read the request frame so the client gets past the write.
		_, _ = readReq(conn)
		// Send a header claiming 32 MiB (> 16 MiB cap) with no body.
		var hdr [4]byte
		binary.LittleEndian.PutUint32(hdr[:], 32*1024*1024)
		_, _ = conn.Write(hdr[:])
		// Keep the connection open briefly so the client reads the header.
		time.Sleep(500 * time.Millisecond)
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second}
	_, err = c.List()
	if err == nil {
		t.Fatal("expected error for oversized frame, got nil")
	}
	if !strings.Contains(err.Error(), "too large") {
		t.Fatalf("expected 'too large' error, got %v", err)
	}
}

// TestResponseErrorCodeMapping verifies the stable `code` field maps to
// ErrNotFound without relying on substring matching (3.5).
func TestResponseErrorCodeMapping(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = readReq(conn)
		// Stable code, message does NOT contain "unknown"/"not found".
		_ = writeResp(conn, map[string]any{
			"type":    "error",
			"message": "service 'redis' is not registered",
			"code":    "not_found",
		})
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second}
	_, err = c.Status("redis")
	if !errors.Is(err, client.ErrNotFound) {
		t.Fatalf("expected ErrNotFound via code, got %v", err)
	}
}

// TestResponseErrorLegacyFallback verifies a server without a `code` field
// still maps via the legacy substring heuristic (backward compat, 3.5).
func TestResponseErrorLegacyFallback(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = readReq(conn)
		_ = writeResp(conn, map[string]any{
			"type":    "error",
			"message": "unknown service 'redis'",
		})
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second}
	_, err = c.Status("redis")
	if !errors.Is(err, client.ErrNotFound) {
		t.Fatalf("expected ErrNotFound via legacy message, got %v", err)
	}
}

// TestFollowLogsReadDeadline verifies ReadFrame enforces a per-frame idle
// deadline so a silent server is detected instead of blocking forever (3.6).
func TestFollowLogsReadDeadline(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = readReq(conn)
		// Never respond. Keep the connection open.
		time.Sleep(3 * time.Second)
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second, ReadTimeout: 200 * time.Millisecond}
	conn, err := c.FollowLogs("redis", 0, true)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	start := time.Now()
	_, err = c.ReadFrame(conn)
	elapsed := time.Since(start)
	if err == nil {
		t.Fatal("expected deadline error, got nil")
	}
	// Should fire around the 200ms deadline, well under the 2s dial timeout.
	if elapsed > 1500*time.Millisecond {
		t.Fatalf("read deadline did not fire in time: %v", elapsed)
	}
}

// TestReadFrameStreamingDecode verifies a normal multi-KB frame decodes
// correctly through the streaming decoder (3.4 regression guard).
func TestReadFrameStreamingDecode(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = readReq(conn)
		services := make([]map[string]any, 256)
		for i := range services {
			services[i] = map[string]any{
				"name": "svc-" + strings.Repeat("x", 60), "state": "running",
				"pid": 1, "restarts": 0, "enabled": true,
			}
		}
		_ = writeResp(conn, map[string]any{"type": "list", "services": services})
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second}
	list, err := c.List()
	if err != nil {
		t.Fatal(err)
	}
	if len(list) != 256 {
		t.Fatalf("expected 256 services, got %d", len(list))
	}
}

func TestWatchSendsLabelKeys(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	got := make(chan fakeReq, 1)
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		req, err := readReq(conn)
		if err != nil {
			return
		}
		got <- req
		_ = writeResp(conn, map[string]any{
			"type": "list",
			"services": []map[string]any{
				{"name": "web", "state": "running", "pid": 1, "restarts": 0, "enabled": true},
			},
		})
	}()

	c := &client.Client{Socket: sock, Timeout: 2 * time.Second}
	conn, err := c.Watch([]string{"microdns-port"})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	select {
	case req := <-got:
		if req.Type != "watch" {
			t.Fatalf("type=%q", req.Type)
		}
		if len(req.LabelKeys) != 1 || req.LabelKeys[0] != "microdns-port" {
			t.Fatalf("label_keys=%v", req.LabelKeys)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timeout waiting for watch request")
	}

	resp, err := c.ReadFrame(conn)
	if err != nil {
		t.Fatal(err)
	}
	if resp.Type != "list" || len(resp.Services) != 1 || resp.Services[0].Name != "web" {
		t.Fatalf("unexpected watch snapshot: %+v", resp)
	}
}

// Ensure the unused io import in this file is referenced (writeResp/readReq use it).
var _ = io.EOF
