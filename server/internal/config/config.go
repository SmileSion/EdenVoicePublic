package config

import (
	"errors"
	"fmt"
	"net"
	"strconv"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"

	"github.com/BurntSushi/toml"
)

const MaxChannelNameBytes = 64

type Channel struct {
	ID                 uint64 `toml:"id"`
	ParentID           uint64 `toml:"parent_id"`
	Name               string `toml:"name"`
	SortOrder          int    `toml:"sort_order"`
	UserLimit          int    `toml:"user_limit"`
	NetworkPolicy      string `toml:"network_policy"`
	AudioQualityPolicy string `toml:"audio_quality_policy"`
}

type Config struct {
	ScreenShare    screenshare.Config `toml:"screen_share"`
	WebAdmin       WebAdmin           `toml:"web_admin"`
	ServerPassword string             `toml:"server_password"`
	ServerName     string             `toml:"server_name"`
	ControlAddr    string             `toml:"control_addr"`
	VoiceAddr      string             `toml:"voice_addr"`
	Voice          Voice              `toml:"voice"`
	Observability  Observability      `toml:"observability"`
	Channels       []Channel          `toml:"channels"`
	Network        Network            `toml:"network"`
	Security       Security           `toml:"security"`
	Archive        Archive            `toml:"archive"`
}

type Archive struct {
	Enabled   bool   `toml:"enabled"`
	RulesFile string `toml:"rules_file"`
}

type Observability struct {
	MetricsAddr string `toml:"metrics_addr"`
}

type Voice struct {
	WorkerCount     int `toml:"worker_count"`
	WorkerQueueSize int `toml:"worker_queue_size"`
	MaxQueueDelayMS int `toml:"max_queue_delay_ms"`
	WriterQueueSize int `toml:"writer_queue_size"`
	WriteBatchSize  int `toml:"write_batch_size"`
}

type Security struct {
	DataDir              string `toml:"data_dir"`
	AllowLegacyInsecure  bool   `toml:"allow_legacy_insecure"`
	MasterProvisionToken string `toml:"master_provision_token"`
}

type Network struct {
	P2PEnabled               bool    `toml:"p2p_enabled"`
	SFUEnabled               bool    `toml:"sfu_enabled"`
	RelayEnabled             bool    `toml:"relay_enabled"`
	P2PMaxRoomSize           int     `toml:"p2p_max_room_size"`
	HybridMaxRoomSize        int     `toml:"hybrid_max_room_size"`
	QualificationDurationMS  int     `toml:"qualification_duration_ms"`
	QualificationRTTMS       int     `toml:"qualification_rtt_ms"`
	QualificationLossPercent float64 `toml:"qualification_loss_percent"`
	QualificationJitterMS    int     `toml:"qualification_jitter_ms"`
	MinimumDwellTimeMS       int     `toml:"minimum_dwell_time_ms"`
	MigrationCooldownMS      int     `toml:"migration_cooldown_ms"`
	DegradeDurationMS        int     `toml:"degrade_duration_ms"`
	RouteDegradeLossPercent  float64 `toml:"route_degrade_loss_percent"`
	RouteDegradeRTTMS        int     `toml:"route_degrade_rtt_ms"`
	RouteDegradeJitterMS     int     `toml:"route_degrade_jitter_ms"`
	MediaQueueSize           int     `toml:"media_queue_size"`
}

func Default() Config {
	return Config{
		ScreenShare: screenshare.DefaultConfig(), ServerPassword: "SmileSion",
		WebAdmin: WebAdmin{ListenAddr: "127.0.0.1:9101"}, ServerName: "EdenVoice",
		ControlAddr:   "0.0.0.0:9000",
		VoiceAddr:     "0.0.0.0:9001",
		Voice:         Voice{WorkerQueueSize: 64, MaxQueueDelayMS: 40, WriterQueueSize: 256, WriteBatchSize: 64},
		Observability: Observability{MetricsAddr: "127.0.0.1:9100"},
		Security:      Security{DataDir: "data"},
		Archive:       Archive{Enabled: true},
		Network: Network{
			P2PEnabled: true, SFUEnabled: true, RelayEnabled: true,
			P2PMaxRoomSize: 6, HybridMaxRoomSize: 12,
			QualificationDurationMS: 1_500, MinimumDwellTimeMS: 1_500,
			QualificationRTTMS: 80, QualificationLossPercent: 2, QualificationJitterMS: 15,
			MigrationCooldownMS: 10_000, DegradeDurationMS: 1_500,
			RouteDegradeLossPercent: 5, RouteDegradeRTTMS: 250,
			RouteDegradeJitterMS: 40, MediaQueueSize: 256,
		},
		Channels: []Channel{
			{ID: 1, Name: "Lobby", SortOrder: 0},
			{ID: 2, Name: "Gaming", SortOrder: 10},
			{ID: 3, Name: "AFK", SortOrder: 20},
		},
	}
}

func Load(path string) (Config, error) {
	cfg := Default()
	if path != "" {
		metadata, err := toml.DecodeFile(path, &cfg)
		if err != nil {
			return Config{}, fmt.Errorf("decode config: %w", err)
		}
		// Existing configurations without the new key remain passwordless.
		if !metadata.IsDefined("server_password") {
			cfg.ServerPassword = ""
		}
		// A legacy range becomes one fixed port at its lower bound. An explicit
		// udp_port wins; no runtime path keeps allocating from the old range.
		if !metadata.IsDefined("screen_share", "udp_port") {
			if metadata.IsDefined("screen_share", "udp_port_min") {
				cfg.ScreenShare.UDPPort = cfg.ScreenShare.UDPPortMin
			}
			if metadata.IsDefined("screen_share", "udp_port_max") && cfg.ScreenShare.UDPPortMax < cfg.ScreenShare.UDPPort {
				return Config{}, errors.New("invalid legacy screen sharing UDP port range")
			}
		}
		cfg.ScreenShare.UDPPortMin, cfg.ScreenShare.UDPPortMax = 0, 0
	}
	if err := cfg.Validate(); err != nil {
		return Config{}, err
	}
	return cfg, nil
}

func (c Config) Validate() error {
	if err := c.ScreenShare.Validate(); err != nil {
		return err
	}
	if _, port, err := net.SplitHostPort(c.VoiceAddr); err == nil && port == strconv.Itoa(int(c.ScreenShare.UDPPort)) {
		return errors.New("screen sharing udp_port must differ from the voice UDP port")
	}
	if len(c.ServerPassword) > 256 {
		return errors.New("server_password must not exceed 256 bytes")
	}
	if err := c.WebAdmin.Validate(); err != nil {
		return err
	}
	if c.ServerName == "" || c.ControlAddr == "" || c.VoiceAddr == "" {
		return errors.New("server_name, control_addr, and voice_addr are required")
	}
	if c.Security.DataDir == "" {
		return errors.New("security.data_dir must be non-empty")
	}
	if c.Observability.MetricsAddr != "" {
		if _, _, err := net.SplitHostPort(c.Observability.MetricsAddr); err != nil {
			return fmt.Errorf("invalid observability.metrics_addr: %w", err)
		}
	}
	if c.Voice.WorkerCount < 0 || c.Voice.WorkerCount > 64 || c.Voice.WorkerQueueSize <= 0 || c.Voice.WorkerQueueSize > 4096 || c.Voice.MaxQueueDelayMS <= 0 || c.Voice.MaxQueueDelayMS > 1000 || c.Voice.WriterQueueSize <= 0 || c.Voice.WriterQueueSize > 4096 || c.Voice.WriteBatchSize <= 0 || c.Voice.WriteBatchSize > 1024 {
		return errors.New("invalid voice worker configuration")
	}
	if len(c.Channels) == 0 {
		return errors.New("at least one channel is required")
	}
	if c.Network.P2PMaxRoomSize < 2 || c.Network.HybridMaxRoomSize < c.Network.P2PMaxRoomSize ||
		c.Network.QualificationDurationMS <= 0 || c.Network.QualificationRTTMS <= 0 || c.Network.QualificationLossPercent <= 0 || c.Network.QualificationJitterMS <= 0 || c.Network.MinimumDwellTimeMS < 0 ||
		c.Network.MigrationCooldownMS < 0 || c.Network.DegradeDurationMS <= 0 ||
		c.Network.RouteDegradeRTTMS <= c.Network.QualificationRTTMS || c.Network.RouteDegradeLossPercent <= c.Network.QualificationLossPercent || c.Network.RouteDegradeJitterMS <= c.Network.QualificationJitterMS ||
		c.Network.MediaQueueSize <= 0 {
		return errors.New("invalid network routing thresholds")
	}
	if !c.Network.SFUEnabled {
		return errors.New("sfu_enabled must be true for the SFU-first architecture")
	}
	seen := make(map[uint64]struct{}, len(c.Channels))
	for _, channel := range c.Channels {
		if channel.ID == 0 || channel.Name == "" || len(channel.Name) > MaxChannelNameBytes || channel.UserLimit < 0 || channel.UserLimit > protocol.MaxChannelUserLimit {
			return fmt.Errorf("invalid channel id=%d name=%q", channel.ID, channel.Name)
		}
		if channel.NetworkPolicy != "" && channel.NetworkPolicy != "auto" && channel.NetworkPolicy != "force_sfu" && channel.NetworkPolicy != "force_p2p" {
			return fmt.Errorf("invalid network policy for channel %d", channel.ID)
		}
		if !validAudioQualityPolicy(channel.AudioQualityPolicy) {
			return fmt.Errorf("invalid audio quality policy for channel %d", channel.ID)
		}
		if _, exists := seen[channel.ID]; exists {
			return fmt.Errorf("duplicate channel id %d", channel.ID)
		}
		seen[channel.ID] = struct{}{}
	}
	for _, channel := range c.Channels {
		if channel.ParentID != 0 {
			if _, exists := seen[channel.ParentID]; !exists {
				return fmt.Errorf("channel %d has unknown parent %d", channel.ID, channel.ParentID)
			}
		}
	}
	return nil
}

func validAudioQualityPolicy(policy string) bool {
	switch policy {
	case "", "auto",
		"quality_1", "quality_2", "quality_3", "quality_4", "quality_5",
		"quality_6", "quality_7", "quality_8", "quality_9", "quality_10",
		"fixed_32k", "fixed_48k", "fixed_64k", "fixed_96k":
		return true
	default:
		return false
	}
}
