package config_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/dcc-bigfred/microinit/go/config"
)

func TestWriteDropinAndListGroup(t *testing.T) {
	dir := t.TempDir()
	svc := config.ServiceDef{
		Name:     "redis",
		Enabled:  config.BoolPtr(true),
		StartCmd: "exec redis-server",
	}
	if err := config.WriteDropin(dir, "infra", "redis", svc); err != nil {
		t.Fatal(err)
	}
	if !config.DropinExists(dir, "infra", "redis") {
		t.Fatal("expected drop-in")
	}
	got, err := config.ListGroup(dir, "infra")
	if err != nil {
		t.Fatal(err)
	}
	if got["redis"].StartCmd != "exec redis-server" {
		t.Fatalf("%+v", got["redis"])
	}
	if err := config.SyncGroup(dir, "infra", map[string]config.ServiceDef{}); err != nil {
		t.Fatal(err)
	}
	if config.DropinExists(dir, "infra", "redis") {
		t.Fatal("expected removed")
	}
}

func TestBaseConfigServiceNames(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "microinit.json")
	if err := os.WriteFile(path, []byte(`{
  "services": [
    {"name": "redis", "cmd": "/etc/init.d/redis"},
    {"name": "alloy", "cmd": "/etc/init.d/alloy"},
    {"name": ""}
  ]
}`), 0o644); err != nil {
		t.Fatal(err)
	}
	names, err := config.BaseConfigServiceNames(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := names["redis"]; !ok {
		t.Fatal("expected redis")
	}
	if _, ok := names["alloy"]; !ok {
		t.Fatal("expected alloy")
	}
	if len(names) != 2 {
		t.Fatalf("len=%d want 2", len(names))
	}
}

func TestBaseConfigServiceNamesMissingFile(t *testing.T) {
	names, err := config.BaseConfigServiceNames(filepath.Join(t.TempDir(), "missing.json"))
	if err != nil {
		t.Fatal(err)
	}
	if len(names) != 0 {
		t.Fatalf("len=%d", len(names))
	}
}
