package supervise_test

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"path/filepath"
	"testing"
	"time"

	"github.com/dcc-bigfred/microinit/go/supervise"
)

func TestEnsureRunningJoinsExisting(t *testing.T) {
	dir := t.TempDir()
	sock := filepath.Join(dir, "microinit.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				if _, err := readReq(c); err != nil {
					return
				}
				_ = writeResp(c, map[string]any{"type": "list", "services": []any{}})
			}(conn)
		}
	}()

	h := supervise.New(sock, "false", filepath.Join(dir, "microinit.json"), filepath.Join(dir, "dropins"))
	joined, err := h.EnsureRunning(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !joined {
		t.Fatal("expected joined")
	}
	if h.Spawned() {
		t.Fatal("must not spawn when joining")
	}
	if err := h.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
}

func TestShutdownNoopWhenNotSpawned(t *testing.T) {
	h := supervise.New(filepath.Join(t.TempDir(), "missing.sock"), "microinit", "", "")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := h.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
}

func readReq(r io.Reader) (map[string]any, error) {
	var hdr [4]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return nil, err
	}
	n := binary.LittleEndian.Uint32(hdr[:])
	buf := make([]byte, n)
	if _, err := io.ReadFull(r, buf); err != nil {
		return nil, err
	}
	var req map[string]any
	err := json.Unmarshal(buf, &req)
	return req, err
}

func writeResp(w io.Writer, msg any) error {
	payload, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	var hdr [4]byte
	binary.LittleEndian.PutUint32(hdr[:], uint32(len(payload)))
	if _, err := w.Write(hdr[:]); err != nil {
		return err
	}
	_, err = w.Write(payload)
	return err
}
