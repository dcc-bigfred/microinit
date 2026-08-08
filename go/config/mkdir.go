package config

import (
	"fmt"
	"os"
	"path/filepath"
)

// MkdirAllBestEffortParents creates path like os.MkdirAll, but ignores errors
// when creating intermediate directories (they may already exist as root-owned
// and be non-writable to the caller). Only the final leaf must exist afterward
// as a directory — either created here or already present.
func MkdirAllBestEffortParents(path string, perm os.FileMode) error {
	path = filepath.Clean(path)
	if path == "" || path == "." {
		return nil
	}

	// Collect path and ancestors: [leaf, parent, ..., volume-root].
	var chain []string
	for p := path; ; {
		chain = append(chain, p)
		parent := filepath.Dir(p)
		if parent == p {
			break
		}
		p = parent
	}
	// Create from root toward leaf.
	for i, j := 0, len(chain)-1; i < j; i, j = i+1, j-1 {
		chain[i], chain[j] = chain[j], chain[i]
	}

	for i, dir := range chain {
		leaf := i == len(chain)-1
		err := os.Mkdir(dir, perm)
		if err == nil {
			continue
		}
		fi, statErr := os.Stat(dir)
		if statErr == nil {
			if !fi.IsDir() {
				return fmt.Errorf("mkdir %s: not a directory", dir)
			}
			continue
		}
		if !leaf {
			continue
		}
		if os.IsExist(err) {
			return fmt.Errorf("mkdir %s: %w", dir, statErr)
		}
		return err
	}
	return nil
}
