package supervise

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestSpawnEnvCopiesBigfredDataDir(t *testing.T) {
	root := t.TempDir()
	t.Setenv("BIGFRED_DATA_DIR", root)
	t.Setenv("DATA_DIR", "")
	_ = os.Unsetenv("DATA_DIR")

	env := spawnEnv(&Host{})
	found := ""
	for _, e := range env {
		if strings.HasPrefix(e, "DATA_DIR=") {
			found = strings.TrimPrefix(e, "DATA_DIR=")
			break
		}
	}
	if found != root {
		t.Fatalf("DATA_DIR=%q, want %q", found, root)
	}
}

func TestSpawnEnvKeepsExistingDataDir(t *testing.T) {
	want := filepath.Join(t.TempDir(), "data")
	t.Setenv("DATA_DIR", want)
	t.Setenv("BIGFRED_DATA_DIR", t.TempDir())

	env := spawnEnv(&Host{})
	count := 0
	for _, e := range env {
		if strings.HasPrefix(e, "DATA_DIR=") {
			count++
			if strings.TrimPrefix(e, "DATA_DIR=") != want {
				t.Fatalf("DATA_DIR overwritten: %s", e)
			}
		}
	}
	if count != 1 {
		t.Fatalf("expected one DATA_DIR entry, got %d", count)
	}
}

func TestSpawnEnvExtraEnvOverridesParent(t *testing.T) {
	t.Setenv("ENABLE_TELEMETRY", "false")
	env := spawnEnv(&Host{ExtraEnv: []string{"ENABLE_TELEMETRY=true"}})
	found := ""
	for _, e := range env {
		if strings.HasPrefix(e, "ENABLE_TELEMETRY=") {
			found = strings.TrimPrefix(e, "ENABLE_TELEMETRY=")
		}
	}
	if found != "true" {
		t.Fatalf("ENABLE_TELEMETRY=%q, want true", found)
	}
}
