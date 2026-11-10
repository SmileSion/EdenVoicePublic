package main

import (
	"bytes"
	"encoding/json"
	"log/slog"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/serverstate"
	"errors"
)

func TestMetricsPortConflictDoesNotBecomeFatal(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	var logs bytes.Buffer
	runOptionalMetrics(func() error {
		conflict, err := net.Listen("tcp", listener.Addr().String())
		if conflict != nil {
			conflict.Close()
		}
		return err
	}, slog.New(slog.NewTextHandler(&logs, nil)))
	if !strings.Contains(logs.String(), "observability stopped; voice continues") {
		t.Fatalf("missing nonfatal metrics error: %s", logs.String())
	}
}

func TestOptionalArchiveFailureDoesNotBlockVoiceStartup(t *testing.T) {
	for _, name := range []string{"corrupt-state", "corrupt-rules", "inaccessible-directory"} {
		t.Run(name, func(t *testing.T) {
			cfg := config.Default()
			cfg.Security.DataDir = t.TempDir()
			path := filepath.Join(cfg.Security.DataDir, "eden-archive.json")
			if name == "corrupt-rules" {
				path = filepath.Join(cfg.Security.DataDir, "eden-archive-rules.json")
			}
			if name == "inaccessible-directory" {
				path = filepath.Join(cfg.Security.DataDir, "not-a-directory")
				cfg.Security.DataDir = path
			}
			original := []byte("invalid archive data must survive startup")
			if err := os.WriteFile(path, original, 0600); err != nil {
				t.Fatal(err)
			}
			var logs bytes.Buffer
			collector := startOptionalArchive(cfg, slog.New(slog.NewTextHandler(&logs, nil)))
			if collector != nil {
				collector.Close()
				t.Fatal("failed archive should be disabled")
			}
			if !strings.Contains(logs.String(), "voice service will continue") {
				t.Fatalf("missing degradation warning: %s", logs.String())
			}
			data, err := os.ReadFile(path)
			if err != nil || !bytes.Equal(data, original) {
				t.Fatal("archive failure modified recovery data")
			}
		})
	}
}

func TestOptionalArchiveHealthyStartupStillEnablesCollector(t *testing.T) {
	cfg := config.Default()
	cfg.Security.DataDir = t.TempDir()
	var logs bytes.Buffer
	collector := startOptionalArchive(cfg, slog.New(slog.NewTextHandler(&logs, nil)))
	if collector == nil {
		t.Fatalf("healthy archive unexpectedly disabled: %s", logs.String())
	}
	if err := collector.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestArchiveDisabledIsHardOff(t *testing.T) {
	cfg := config.Default()
	cfg.Security.DataDir = t.TempDir()
	cfg.Archive.Enabled = false
	cfg.Archive.RulesFile = filepath.Join(cfg.Security.DataDir, "rules.json")
	if err := os.WriteFile(cfg.Archive.RulesFile, []byte("invalid rules must not be read"), 0600); err != nil {
		t.Fatal(err)
	}

	collector, err := loadArchiveCollector(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if collector != nil {
		t.Fatal("disabled archive created a collector")
	}
	if _, err := os.Stat(filepath.Join(cfg.Security.DataDir, "eden-archive.json")); !os.IsNotExist(err) {
		t.Fatalf("disabled archive touched persistence: %v", err)
	}
	data, err := os.ReadFile(cfg.Archive.RulesFile)
	if err != nil || string(data) != "invalid rules must not be read" {
		t.Fatal("hard-off touched rules file")
	}
}

func TestSecurityAuditProducesStructuredEventWithoutProvisionToken(t *testing.T) {
	var output bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&output, nil))

	securityAudit(logger, slog.LevelWarn, "master_provision_token_generated")

	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatalf("decode security audit: %v", err)
	}
	if record["level"] != "WARN" || record["msg"] != "security audit" || record["event"] != "master_provision_token_generated" {
		t.Fatalf("unexpected security audit record: %#v", record)
	}
	if _, exists := record["token"]; exists {
		t.Fatalf("security audit unexpectedly contains a token field: %s", output.String())
	}
}

func TestSecurityAuditPreservesIdentityChangeAttributes(t *testing.T) {
	var output bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&output, nil))

	securityAudit(logger, slog.LevelInfo, "server_identity_created", "fingerprint", "test-fingerprint")

	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatalf("decode security audit: %v", err)
	}
	if record["level"] != "INFO" || record["event"] != "server_identity_created" || record["fingerprint"] != "test-fingerprint" {
		t.Fatalf("unexpected security audit record: %#v", record)
	}
}

func TestRoomLimitSurvivesInitializationEditAndRestart(t *testing.T) {
	cfg := config.Default()
	cfg.Channels = []config.Channel{{ID: 1, Name: "Two", UserLimit: 2, NetworkPolicy: "auto", AudioQualityPolicy: "auto"}}
	path := filepath.Join(t.TempDir(), "administration.json")
	store, _, err := administration.LoadOrCreateWithToken(path, configuredChannels(cfg.Channels), "")
	if err != nil {
		t.Fatal(err)
	}
	for restart := 0; restart < 2; restart++ {
		store, _, err = administration.LoadOrCreateWithToken(path, nil, "")
		if err != nil {
			t.Fatal(err)
		}
		cfg.Channels = restoredChannels(store.Channels())
		state := serverstate.New(cfg)
		limit := 2 + restart
		if cfg.Channels[0].UserLimit != limit {
			t.Fatalf("restart %d: lost limit: %+v", restart, cfg.Channels)
		}
		for i := 0; i < limit; i++ {
			u := state.AddSession("member", nil)
			if err := state.Join(u.ID, 1); err != nil {
				t.Fatal(err)
			}
		}
		extra := state.AddSession("extra", nil)
		if err := state.Join(extra.ID, 1); !errors.Is(err, serverstate.ErrChannelFull) {
			t.Fatalf("over-capacity join: %v", err)
		}
		if err := state.Move(extra.ID, 1); !errors.Is(err, serverstate.ErrChannelFull) {
			t.Fatalf("over-capacity move: %v", err)
		}
		ch := store.Channels()[0]
		ch.UserLimit = 3
		if err := store.EditChannel(ch); err != nil {
			t.Fatal(err)
		}
	}
}
