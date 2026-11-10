package screenshare

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
)

type PolicyDocument struct {
	Revision uint64 `json:"revision"`
	Policy   Policy `json:"policy"`
}

func LoadPolicy(path string, fallback Policy) (PolicyDocument, error) {
	d := PolicyDocument{Revision: 1, Policy: fallback}
	if path == "" {
		return d, nil
	}
	b, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return d, nil
	}
	if err != nil {
		return d, err
	}
	if err = json.Unmarshal(b, &d); err != nil {
		return d, err
	}
	if d.Revision == 0 {
		return d, errors.New("invalid screen policy revision")
	}
	return d, d.Policy.Validate()
}

// Persist before applying runtime changes. A failed write leaves active shares alone.
func SavePolicy(path string, d PolicyDocument) error {
	if err := d.Policy.Validate(); err != nil {
		return err
	}
	if path == "" {
		return nil
	}
	b, err := json.MarshalIndent(d, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".screen-policy-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(b); err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	return os.Rename(f.Name(), path)
}
