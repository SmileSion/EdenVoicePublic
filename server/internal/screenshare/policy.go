// Package screenshare owns the optional video path. It never uses voice queues.
package screenshare

import (
	"errors"
	"fmt"
	"net"
)

type Policy struct {
	Enabled        bool `json:"enabled" toml:"enabled"`
	MaxBitrateKbps int  `json:"max_bitrate_kbps" toml:"max_bitrate_kbps"`
	MaxWidth       int  `json:"max_width" toml:"max_width"`
	MaxHeight      int  `json:"max_height" toml:"max_height"`
	MaxFPS         int  `json:"max_fps" toml:"max_fps"`
	AllowAudio     bool `json:"allow_audio" toml:"allow_audio"`
	MaxViewers     int  `json:"max_viewers" toml:"max_viewers"`
	MaxRooms       int  `json:"max_rooms" toml:"max_rooms"`
}

type Config struct {
	Policy   `toml:",inline"`
	PublicIP string `toml:"public_ip"`
	UDPPort  uint16 `toml:"udp_port"`
	// Deprecated: decoded only so config.Load can migrate old TOML files.
	UDPPortMin uint16 `toml:"udp_port_min"`
	UDPPortMax uint16 `toml:"udp_port_max"`
}

func DefaultConfig() Config {
	// Existing small-bandwidth servers must explicitly enable video.
	return Config{Policy: Policy{MaxBitrateKbps: 4000, MaxWidth: 1920, MaxHeight: 1080, MaxFPS: 30, AllowAudio: true, MaxViewers: 8, MaxRooms: 4}, UDPPort: 7010}
}

func (p Policy) Validate() error {
	if p.MaxBitrateKbps < 128 || p.MaxBitrateKbps > 20000 || p.MaxWidth < 320 || p.MaxWidth > 3840 || p.MaxHeight < 180 || p.MaxHeight > 2160 || p.MaxFPS < 1 || p.MaxFPS > 60 || p.MaxViewers < 1 || p.MaxViewers > 32 || p.MaxRooms < 1 || p.MaxRooms > 32 {
		return errors.New("screen sharing limits out of range: bitrate 128–20000 kbps, width 320–3840, height 180–2160, fps 1–60, viewers/rooms 1–32")
	}
	return nil
}
func (c Config) Validate() error {
	if err := c.Policy.Validate(); err != nil {
		return err
	}
	if c.UDPPort == 0 {
		return errors.New("screen sharing udp_port must be 1–65535")
	}
	if c.PublicIP != "" && net.ParseIP(c.PublicIP) == nil {
		return errors.New("screen sharing public_ip must be an IP address")
	}
	return nil
}

type Settings struct {
	Width       int  `json:"width"`
	Height      int  `json:"height"`
	FPS         int  `json:"fps"`
	BitrateKbps int  `json:"bitrate_kbps"`
	Audio       bool `json:"audio"`
}

func (s Settings) Validate(p Policy) error {
	if !p.Enabled {
		return errors.New("服务器未开启屏幕共享")
	}
	if s.Width < 160 || s.Height < 90 || s.FPS < 1 || s.BitrateKbps < 128 {
		return errors.New("无效的共享画面参数")
	}
	if s.Width > p.MaxWidth || s.Height > p.MaxHeight || s.FPS > p.MaxFPS || s.BitrateKbps > p.MaxBitrateKbps {
		return fmt.Errorf("超出服务器限制：%d×%d / %d fps / %d kbps", p.MaxWidth, p.MaxHeight, p.MaxFPS, p.MaxBitrateKbps)
	}
	if s.Audio && !p.AllowAudio {
		return errors.New("服务器不允许共享声音")
	}
	return nil
}

type Request struct {
	Paused    *bool    `json:"paused,omitempty"`
	RequestID string   `json:"request_id"`
	Action    string   `json:"action"`
	ChannelID uint64   `json:"channel_id"`
	ShareID   string   `json:"share_id"`
	Settings  Settings `json:"settings"`
	SDP       string   `json:"sdp,omitempty"`
}
type State struct {
	Paused    bool     `json:"paused"`
	Revision  uint64   `json:"revision"`
	ChannelID uint64   `json:"channel_id"`
	ShareID   string   `json:"share_id"`
	HostID    uint64   `json:"host_id"`
	Settings  Settings `json:"settings"`
	Viewers   int      `json:"viewers"`
	Ready     bool     `json:"ready"`
	Policy    Policy   `json:"policy"`
}
type Response struct {
	State
	RequestID string `json:"request_id,omitempty"`
	Action    string `json:"action"`
	SDP       string `json:"sdp,omitempty"`
	Error     string `json:"error,omitempty"`
}
