package config_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/dcc-bigfred/microinit/go/config"
)

func TestMkdirAllBestEffortParentsCreatesTree(t *testing.T) {
	root := t.TempDir()
	leaf := filepath.Join(root, "a", "b", "c")
	if err := config.MkdirAllBestEffortParents(leaf, 0o755); err != nil {
		t.Fatal(err)
	}
	fi, err := os.Stat(leaf)
	if err != nil {
		t.Fatal(err)
	}
	if !fi.IsDir() {
		t.Fatal("expected directory")
	}
}

func TestMkdirAllBestEffortParentsExistingLeafOK(t *testing.T) {
	root := t.TempDir()
	parent := filepath.Join(root, "services")
	leaf := filepath.Join(parent, "infra")
	if err := os.MkdirAll(leaf, 0o755); err != nil {
		t.Fatal(err)
	}
	// Lock parent against writes (simulate root-owned 0555 parent).
	if err := os.Chmod(parent, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(parent, 0o755) })

	if err := config.MkdirAllBestEffortParents(leaf, 0o755); err != nil {
		t.Fatalf("existing leaf under non-writable parent: %v", err)
	}
}

func TestMkdirAllBestEffortParentsLeafMissingNoWrite(t *testing.T) {
	root := t.TempDir()
	parent := filepath.Join(root, "services")
	if err := os.Mkdir(parent, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(parent, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(parent, 0o755) })

	leaf := filepath.Join(parent, "infra")
	err := config.MkdirAllBestEffortParents(leaf, 0o755)
	if err == nil {
		t.Fatal("expected error creating leaf without parent write")
	}
}

func TestMkdirAllBestEffortParentsIgnoresParentCreateFailure(t *testing.T) {
	root := t.TempDir()
	// Pre-create locked grandparent; intermediate "services" missing would fail
	// for a normal MkdirAll when we cannot create it — but if leaf already
	// exists via a side channel we still succeed. Here: create full tree first,
	// then lock an ancestor and re-ensure leaf.
	grand := filepath.Join(root, "microinit.d")
	services := filepath.Join(grand, "services")
	leaf := filepath.Join(services, "infra")
	if err := os.MkdirAll(leaf, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(grand, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(grand, 0o755) })

	if err := config.MkdirAllBestEffortParents(leaf, 0o755); err != nil {
		t.Fatalf("best-effort parents with existing leaf: %v", err)
	}
}

func TestMkdirAllBestEffortParentsNotADirectory(t *testing.T) {
	root := t.TempDir()
	file := filepath.Join(root, "notdir")
	if err := os.WriteFile(file, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := config.MkdirAllBestEffortParents(file, 0o755)
	if err == nil {
		t.Fatal("expected not a directory")
	}
}
