package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestServerPasswordDefaultsAndMigration(t *testing.T) {
	fresh, err := Load("")
	if err != nil || fresh.ServerPassword != "SmileSion" {
		t.Fatal("new server must use default password", err)
	}
	for _, tt := range []struct{ toml, want string }{{"", ""}, {"server_password = \"\"", ""}, {"server_password = \"custom password\"", "custom password"}} {
		file := filepath.Join(t.TempDir(), "config.toml")
		if err := os.WriteFile(file, []byte(tt.toml), 0600); err != nil {
			t.Fatal(err)
		}
		got, err := Load(file)
		if err != nil || got.ServerPassword != tt.want {
			t.Fatalf("migration: %q %v", got.ServerPassword, err)
		}
	}
}
