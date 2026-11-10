package privatefs

import (
	"fmt"
	"os"
	"path/filepath"
)

// Ensure tightens an existing security file and its containing directory.
// A failure is fatal to the caller so sensitive state is never loaded from a
// location that the process could not protect.
func Ensure(path string) error {
	if err := os.Chmod(filepath.Dir(path), 0o700); err != nil {
		return fmt.Errorf("secure directory permissions: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return fmt.Errorf("secure file permissions: %w", err)
	}
	return nil
}

// PrepareDirectory creates and tightens the directory for a security file.
func PrepareDirectory(path string) error {
	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create security directory: %w", err)
	}
	if err := os.Chmod(directory, 0o700); err != nil {
		return fmt.Errorf("secure directory permissions: %w", err)
	}
	return nil
}
