package config

import (
	"encoding/json"
	"fmt"
	"os"
)

// baseConfigFile is the subset of microinit.json needed to list declared services.
type baseConfigFile struct {
	Services []struct {
		Name string `json:"name"`
	} `json:"services"`
}

// BaseConfigServiceNames returns service names declared in the main
// microinit.json (configPath). Callers often treat these as system-owned and
// refuse to overwrite them with drop-ins.
func BaseConfigServiceNames(configPath string) (map[string]struct{}, error) {
	data, err := os.ReadFile(configPath)
	if err != nil {
		if os.IsNotExist(err) {
			return map[string]struct{}{}, nil
		}
		return nil, fmt.Errorf("read microinit config %s: %w", configPath, err)
	}
	var cfg baseConfigFile
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("parse microinit config %s: %w", configPath, err)
	}
	out := make(map[string]struct{}, len(cfg.Services))
	for _, svc := range cfg.Services {
		if svc.Name == "" {
			continue
		}
		out[svc.Name] = struct{}{}
	}
	return out, nil
}
