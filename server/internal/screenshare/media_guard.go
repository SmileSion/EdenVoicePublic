package screenshare

import (
	"github.com/pion/rtp"
	"github.com/pion/rtp/codecs"
	"time"
)

// Actual media validation, independent of the client's declared parameters.
// Allow bounded keyframe bursts and RTP overhead, not sustained excess traffic.
type mediaGuard struct {
	settings       Settings
	video          bool
	tokens, rate   float64
	at, frameStart time.Time
	frames         int
}

func newMediaGuard(s Settings, video bool, now time.Time) *mediaGuard {
	rate := float64(s.BitrateKbps) * 1000 / 8 * 1.25
	if !video {
		rate = 192000 / 8
	}
	return &mediaGuard{settings: s, video: video, tokens: rate * 2, rate: rate, at: now, frameStart: now}
}
func (g *mediaGuard) accept(p *rtp.Packet, now time.Time) bool {
	g.tokens = min(g.rate*2, g.tokens+max(0, now.Sub(g.at).Seconds())*g.rate) - float64(p.MarshalSize())
	g.at = now
	if g.tokens < 0 {
		return false
	}
	// Browsers send padding-only RTP to probe available bandwidth. It is
	// transport traffic, not an empty/malformed VP8 frame; still charge bytes
	// to the bounded rate budget above.
	if len(p.Payload) == 0 && p.Padding && p.PaddingSize > 0 {
		return true
	}
	if !g.video {
		return len(p.Payload) <= 1500
	}
	var vp8 codecs.VP8Packet
	body, err := vp8.Unmarshal(p.Payload)
	if err != nil {
		return false
	}
	if vp8.S == 1 && vp8.PID == 0 {
		g.frames++
		if now.Sub(g.frameStart) >= 2*time.Second {
			if float64(g.frames) > float64(g.settings.FPS+3)*now.Sub(g.frameStart).Seconds() {
				return false
			}
			g.frames = 0
			g.frameStart = now
		}
		if len(body) > 0 && body[0]&1 == 0 {
			if len(body) < 10 || body[3] != 0x9d || body[4] != 0x01 || body[5] != 0x2a {
				return false
			}
			width := (int(body[6]) | int(body[7])<<8) & 0x3fff
			height := (int(body[8]) | int(body[9])<<8) & 0x3fff
			if width == 0 || height == 0 || width > g.settings.Width || height > g.settings.Height {
				return false
			}
		}
	}
	return true
}
