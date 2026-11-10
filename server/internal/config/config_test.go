package config

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestValidateRequiresWiderRouteDegradationThresholds(t *testing.T) {
	cfg := Default()
	cfg.Network.RouteDegradeRTTMS = cfg.Network.QualificationRTTMS
	if err := cfg.Validate(); err == nil {
		t.Fatal("equal qualification and degradation thresholds should be rejected")
	}
	cfg = Default()
	if err := cfg.Validate(); err != nil {
		t.Fatalf("default hysteresis thresholds are invalid: %v", err)
	}
}

func TestValidateRejectsInvalidChannelNetworkPolicy(t *testing.T) {
	cfg := Default()
	cfg.Channels[0].NetworkPolicy = "sometimes"
	if err := cfg.Validate(); err == nil {
		t.Fatal("invalid channel network policy should be rejected")
	}
}

func TestValidateAcceptsRoomAudioQualityLevelsOneThroughTen(t *testing.T) {
	for level := 1; level <= 10; level++ {
		cfg := Default()
		cfg.Channels[0].AudioQualityPolicy = fmt.Sprintf("quality_%d", level)
		if err := cfg.Validate(); err != nil {
			t.Fatalf("quality level %d should be valid: %v", level, err)
		}
	}
}

func TestValidateRejectsRoomAudioQualityOutsideRange(t *testing.T) {
	cfg := Default()
	cfg.Channels[0].AudioQualityPolicy = "quality_11"
	if err := cfg.Validate(); err == nil {
		t.Fatal("quality level 11 should be rejected")
	}
}

func TestValidateRejectsNegativeChannelUserLimit(t *testing.T) {
	cfg := Default()
	cfg.Channels[0].UserLimit = -1
	if err := cfg.Validate(); err == nil {
		t.Fatal("negative channel user limit should be rejected")
	}
}

func TestValidateVoiceWorkerConfiguration(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(*Config)
	}{
		{name: "negative worker count", mutate: func(cfg *Config) { cfg.Voice.WorkerCount = -1 }},
		{name: "too many workers", mutate: func(cfg *Config) { cfg.Voice.WorkerCount = 65 }},
		{name: "empty queue", mutate: func(cfg *Config) { cfg.Voice.WorkerQueueSize = 0 }},
		{name: "oversized queue", mutate: func(cfg *Config) { cfg.Voice.WorkerQueueSize = 4097 }},
		{name: "empty delay", mutate: func(cfg *Config) { cfg.Voice.MaxQueueDelayMS = 0 }},
		{name: "oversized delay", mutate: func(cfg *Config) { cfg.Voice.MaxQueueDelayMS = 1001 }},
		{name: "empty writer queue", mutate: func(cfg *Config) { cfg.Voice.WriterQueueSize = 0 }},
		{name: "oversized writer queue", mutate: func(cfg *Config) { cfg.Voice.WriterQueueSize = 4097 }},
		{name: "empty write batch", mutate: func(cfg *Config) { cfg.Voice.WriteBatchSize = 0 }},
		{name: "oversized write batch", mutate: func(cfg *Config) { cfg.Voice.WriteBatchSize = 1025 }},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			cfg := Default()
			test.mutate(&cfg)
			if err := cfg.Validate(); err == nil {
				t.Fatal("invalid voice worker configuration was accepted")
			}
		})
	}
}

func TestDefaultVoiceWorkersUseAutomaticBoundedPool(t *testing.T) {
	cfg := Default()
	if cfg.Voice.WorkerCount != 0 || cfg.Voice.WorkerQueueSize != 64 || cfg.Voice.MaxQueueDelayMS != 40 || cfg.Voice.WriterQueueSize != 256 || cfg.Voice.WriteBatchSize != 64 {
		t.Fatalf("unexpected voice worker defaults: %+v", cfg.Voice)
	}
	if err := cfg.Validate(); err != nil {
		t.Fatalf("default voice worker configuration is invalid: %v", err)
	}
}

func TestObservabilityDefaultsToLoopbackAndRejectsMalformedAddress(t *testing.T) {
	cfg := Default()
	if cfg.Observability.MetricsAddr != "127.0.0.1:9100" {
		t.Fatalf("metrics should default to loopback: %q", cfg.Observability.MetricsAddr)
	}
	cfg.Observability.MetricsAddr = "missing-port"
	if err := cfg.Validate(); err == nil {
		t.Fatal("malformed metrics address should be rejected")
	}
	cfg.Observability.MetricsAddr = ""
	if err := cfg.Validate(); err != nil {
		t.Fatalf("empty metrics address should disable the endpoint: %v", err)
	}
}

func TestArchiveDefaultsEnabledForBackwardCompatibleConfigs(t *testing.T) {
	if !Default().Archive.Enabled {
		t.Fatal("Eden Archive should default to enabled")
	}
}

func TestScreenSharingTOMLLimits(t *testing.T) {
	path := filepath.Join(t.TempDir(), "screen.toml")
	if err := os.WriteFile(path, []byte("[screen_share]\nenabled=true\nmax_bitrate_kbps=1500\nmax_fps=15\nallow_audio=false\n"), 0600); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if !cfg.ScreenShare.Enabled || cfg.ScreenShare.MaxBitrateKbps != 1500 || cfg.ScreenShare.MaxFPS != 15 || cfg.ScreenShare.AllowAudio {
		t.Fatalf("limits not decoded: %+v", cfg.ScreenShare)
	}
	if cfg.ScreenShare.MaxWidth != 1920 {
		t.Fatal("omitted fields must keep defaults")
	}
}

func TestScreenSharingFixedPortMigration(t *testing.T) {
	for _, tt := range []struct {
		name, body string
		want       uint16
		invalid    bool
	}{
		{name: "default", want: 7010},
		{name: "explicit", body: "udp_port=7450", want: 7450},
		{name: "legacy range", body: "udp_port_min=7400\nudp_port_max=7499", want: 7400},
		{name: "legacy lower only", body: "udp_port_min=7400", want: 7400},
		{name: "new wins", body: "udp_port=7550\nudp_port_min=7400\nudp_port_max=7399", want: 7550},
		{name: "zero", body: "udp_port=0", invalid: true},
		{name: "negative", body: "udp_port=-1", invalid: true},
		{name: "too large", body: "udp_port=65536", invalid: true},
		{name: "legacy zero", body: "udp_port_min=0\nudp_port_max=7099", invalid: true},
		{name: "legacy reversed", body: "udp_port_min=7500\nudp_port_max=7499", invalid: true},
	} {
		t.Run(tt.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "config.toml")
			if err := os.WriteFile(path, []byte("[screen_share]\n"+tt.body+"\n"), 0600); err != nil {
				t.Fatal(err)
			}
			cfg, err := Load(path)
			if tt.invalid {
				if err == nil {
					t.Fatal("invalid port accepted")
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if cfg.ScreenShare.UDPPort != tt.want || cfg.ScreenShare.UDPPortMin != 0 || cfg.ScreenShare.UDPPortMax != 0 {
				t.Fatalf("expected one fixed port %d, got %+v", tt.want, cfg.ScreenShare)
			}
		})
	}
}

func TestScreenSharingPortCannotOverlapVoice(t *testing.T) {
	cfg := Default()
	cfg.VoiceAddr = "0.0.0.0:7010"
	if err := cfg.Validate(); err == nil {
		t.Fatal("screen sharing port overlaps the independent voice listener")
	}
}
