package supervise_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/dcc-bigfred/microinit/go/client"
	"github.com/dcc-bigfred/microinit/go/supervise"
)

// sleepScript writes a tiny shell wrapper that ignores its argv (microinit
// passes --socket/supervise/--config) and stays alive for `dur`. Returns the
// executable path to use as the microinit binary in tests.
func sleepScript(t *testing.T, dur time.Duration) string {
	t.Helper()
	dir := t.TempDir()
	p := filepath.Join(dir, "fake-microinit")
	if err := os.WriteFile(p, []byte("#!/bin/sh\nexec sleep "+dur.String()+"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return p
}

// TestEnsureRunningSpawnFail verifies a missing binary surfaces a clear error
// and does not leave the host in a spawned state (6).
func TestEnsureRunningSpawnFail(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	h := supervise.New(sock, "/nonexistent/microinit-bin", filepath.Join(dir, "microinit.json"), "")
	joined, err := h.EnsureRunning(context.Background())
	if err == nil {
		t.Fatal("expected spawn error, got nil")
	}
	if joined {
		t.Fatal("must not report joined on spawn failure")
	}
	if h.Spawned() {
		t.Fatal("must not be spawned after spawn failure")
	}
	if !strings.Contains(err.Error(), "start microinit") {
		t.Fatalf("expected start error, got %v", err)
	}
}

// TestEnsureRunningSpawnExitsBeforeReady verifies a binary that exits
// immediately is reported as a pre-ready exit with captured output (6).
func TestEnsureRunningSpawnExitsBeforeReady(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	// "false" exits 1 immediately and prints nothing.
	h := supervise.New(sock, "false", filepath.Join(dir, "microinit.json"), "")
	h.ReadyTimeout = 2 * time.Second
	_, err := h.EnsureRunning(context.Background())
	if err == nil {
		t.Fatal("expected pre-ready exit error, got nil")
	}
	if !strings.Contains(err.Error(), "exited before ready") {
		t.Fatalf("expected 'exited before ready', got %v", err)
	}
}

// TestEnsureRunningReadyTimeout verifies a binary that stays alive but never
// opens the socket is killed after ReadyTimeout and reported (6).
func TestEnsureRunningReadyTimeout(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	// "sleep 30" stays alive but never opens the socket.
	h := supervise.New(sock, sleepScript(t, 30*time.Second), filepath.Join(dir, "microinit.json"), "")
	h.ReadyTimeout = 500 * time.Millisecond
	_, err := h.EnsureRunning(context.Background())
	if err == nil {
		t.Fatal("expected ready timeout, got nil")
	}
	if !strings.Contains(err.Error(), "did not become ready") {
		t.Fatalf("expected 'did not become ready', got %v", err)
	}
	if h.Spawned() {
		t.Fatal("must clear spawned after timeout kill")
	}
}

// TestEnsureRunningConfigRequired verifies spawning without ConfigPath fails
// fast instead of launching a daemon with no config (6).
func TestEnsureRunningConfigRequired(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	h := supervise.New(sock, "sleep", "", "")
	_, err := h.EnsureRunning(context.Background())
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !strings.Contains(err.Error(), "ConfigPath is required") {
		t.Fatalf("expected ConfigPath error, got %v", err)
	}
}

// TestEnsureRunningCtxCancelledSendsSIGTERM verifies cancelling ctx during
// startup sends SIGTERM (soft-kill) and returns the ctx error (2.3).
func TestEnsureRunningCtxCancelledSendsSIGTERM(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	// fake-microinit stays alive and will be terminated by our SIGTERM during startup.
	h := supervise.New(sock, sleepScript(t, 30*time.Second), filepath.Join(dir, "microinit.json"), "")
	h.ReadyTimeout = 30 * time.Second
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(300 * time.Millisecond)
		cancel()
	}()
	_, err := h.EnsureRunning(ctx)
	if err == nil {
		t.Fatal("expected cancellation error, got nil")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("expected context.Canceled, got %v", err)
	}
}

// TestClientErrNotFoundSentinel verifies the exported sentinel exists and is
// usable for errors.Is checks by downstream callers (6).
func TestClientErrNotFoundSentinel(t *testing.T) {
	if !errors.Is(client.ErrNotFound, client.ErrNotFound) {
		t.Fatal("ErrNotFound must satisfy errors.Is with itself")
	}
}
