package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/dcc-bigfred/microinit/go/client"
)

// WriteDropin writes a single-service drop-in at dir/group/name.json.
func WriteDropin(dir, group, name string, svc ServiceDef) error {
	if err := client.ValidateName(group); err != nil {
		return fmt.Errorf("drop-in group: %w", err)
	}
	if err := client.ValidateName(name); err != nil {
		return fmt.Errorf("drop-in name: %w", err)
	}
	if svc.Name == "" {
		svc.Name = name
	}
	if svc.Name != name {
		return fmt.Errorf("drop-in name %q does not match service %q", name, svc.Name)
	}
	content, err := json.MarshalIndent(DropinFile{Services: []ServiceDef{svc}}, "", "  ")
	if err != nil {
		return err
	}
	return WriteFileAtomically(filepath.Join(dir, group, name+".json"), append(content, '\n'))
}

// RemoveDropin deletes dir/group/name.json (no-op if missing).
func RemoveDropin(dir, group, name string) error {
	if err := client.ValidateName(group); err != nil {
		return err
	}
	if err := client.ValidateName(name); err != nil {
		return err
	}
	err := os.Remove(filepath.Join(dir, group, name+".json"))
	if os.IsNotExist(err) {
		return nil
	}
	return err
}

// SyncGroup makes dir/group contain exactly the services in desired.
func SyncGroup(dir, group string, desired map[string]ServiceDef) error {
	if err := client.ValidateName(group); err != nil {
		return err
	}
	groupDir := filepath.Join(dir, group)
	if err := os.MkdirAll(groupDir, 0o755); err != nil {
		return err
	}
	entries, err := os.ReadDir(groupDir)
	if err != nil {
		return err
	}
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".json" {
			continue
		}
		name := entry.Name()[:len(entry.Name())-len(".json")]
		if _, ok := desired[name]; !ok {
			if err := os.Remove(filepath.Join(groupDir, entry.Name())); err != nil {
				return err
			}
		}
	}
	names := make([]string, 0, len(desired))
	for name := range desired {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		if err := WriteDropin(dir, group, name, desired[name]); err != nil {
			return err
		}
	}
	return nil
}

// ListGroup reads all drop-ins under dir/group.
func ListGroup(dir, group string) (map[string]ServiceDef, error) {
	groupDir := filepath.Join(dir, group)
	entries, err := os.ReadDir(groupDir)
	if os.IsNotExist(err) {
		return map[string]ServiceDef{}, nil
	}
	if err != nil {
		return nil, err
	}
	out := make(map[string]ServiceDef)
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".json" {
			continue
		}
		data, err := os.ReadFile(filepath.Join(groupDir, entry.Name()))
		if err != nil {
			return nil, err
		}
		var dropin DropinFile
		if err := json.Unmarshal(data, &dropin); err != nil {
			return nil, err
		}
		for _, svc := range dropin.Services {
			out[svc.Name] = svc
		}
	}
	return out, nil
}

// DropinExists reports whether a drop-in file is present for group/name.
func DropinExists(dir, group, name string) bool {
	if err := client.ValidateName(group); err != nil {
		return false
	}
	if err := client.ValidateName(name); err != nil {
		return false
	}
	_, err := os.Stat(filepath.Join(dir, group, name+".json"))
	return err == nil
}

// WriteFileAtomically creates parent dirs and renames into place.
func WriteFileAtomically(path string, content []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".tmp-*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)
	if err := tmp.Chmod(0o600); err != nil {
		_ = tmp.Close()
		return err
	}
	if _, err := tmp.Write(content); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmpName, path)
}
