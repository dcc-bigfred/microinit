package config_test

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/dcc-bigfred/microinit/go/config"
)

func TestWithCreatedByAndMatchLabels(t *testing.T) {
	svc := config.WithCreatedBy(config.ServiceDef{
		Name:     "redis",
		StartCmd: "exec redis-server",
		Labels:   map[string]string{"env": "prod"},
	}, "bigfred")
	if svc.Labels[config.LabelCreatedBy] != "bigfred" || svc.Labels["env"] != "prod" {
		t.Fatalf("%+v", svc.Labels)
	}
	raw, err := json.Marshal(svc)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(raw), `"created-by":"bigfred"`) {
		t.Fatalf("json: %s", raw)
	}
	if !config.MatchLabels(svc.Labels, map[string]string{config.LabelCreatedBy: "bigfred"}) {
		t.Fatal("expected match")
	}
	if config.MatchLabels(svc.Labels, map[string]string{config.LabelCreatedBy: "other"}) {
		t.Fatal("expected no match")
	}
}
