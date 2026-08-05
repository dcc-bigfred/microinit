package client

import (
	"bytes"
	"testing"
)

func TestWriteFullShortWrites(t *testing.T) {
	var buf bytes.Buffer
	w := &shortWriter{w: &buf, max: 3}
	if err := writeFrame(w, map[string]string{"type": "ok"}); err != nil {
		t.Fatal(err)
	}
	var resp Response
	if err := readFrame(bytes.NewReader(buf.Bytes()), &resp); err != nil {
		t.Fatal(err)
	}
	if resp.Type != "ok" {
		t.Fatalf("got %+v", resp)
	}
}

type shortWriter struct {
	w   *bytes.Buffer
	max int
}

func (s *shortWriter) Write(p []byte) (int, error) {
	if len(p) > s.max {
		p = p[:s.max]
	}
	return s.w.Write(p)
}

func TestReadFrameExactLength(t *testing.T) {
	var buf bytes.Buffer
	if err := writeFrame(&buf, map[string]any{"type": "ok"}); err != nil {
		t.Fatal(err)
	}
	if err := writeFrame(&buf, map[string]any{"type": "list", "services": []any{}}); err != nil {
		t.Fatal(err)
	}
	r := bytes.NewReader(buf.Bytes())
	var a, b Response
	if err := readFrame(r, &a); err != nil {
		t.Fatal(err)
	}
	if a.Type != "ok" {
		t.Fatalf("first: %+v", a)
	}
	if err := readFrame(r, &b); err != nil {
		t.Fatal(err)
	}
	if b.Type != "list" {
		t.Fatalf("second: %+v", b)
	}
}
