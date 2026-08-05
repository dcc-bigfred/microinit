package supervise

import (
	"bytes"
	"testing"
)

func TestBoundedBufferWriteReturnsInputLength(t *testing.T) {
	b := newBoundedBuffer(8)
	in := bytes.Repeat([]byte("x"), 32)
	n, err := b.Write(in)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(in) {
		t.Fatalf("Write returned %d, want %d (os/exec requires full length)", n, len(in))
	}
	if got := b.String(); len(got) != 8 {
		t.Fatalf("captured len=%d want 8", len(got))
	}
}
