package screenshare

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pion/ice/v4"
	"github.com/pion/interceptor"
	"github.com/pion/rtcp"
	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"
)

type Hooks struct {
	Allowed func(session, channel uint64) bool
	// Changed must enqueue notifications without blocking and recheck recipients.
	Changed func(channel uint64, reason string)
}
type Manager struct {
	snapshotSequence atomic.Uint64
	mu               sync.RWMutex
	policy           Policy
	api              *webrtc.API
	udpMux           ice.UDPMux
	config           Config
	hooks            Hooks
	rooms            map[uint64]*room
	done             chan struct{}
	once             sync.Once
}
type room struct {
	paused               bool
	id                   string
	channel, host        uint64
	settings             Settings
	publisher            *endpoint
	viewers              map[uint64]*endpoint
	videoSSRC            atomic.Uint32
	videoSeen, audioSeen atomic.Bool
	lastPLI              atomic.Int64
	created              time.Time
}
type mediaPacket struct {
	packet *rtp.Packet
	video  bool
	at     time.Time
}
type endpoint struct {
	pc           *webrtc.PeerConnection
	api          *webrtc.API
	video, audio *webrtc.TrackLocalStaticRTP
	queue        chan mediaPacket
	done         chan struct{}
	once         sync.Once
}

func (e *endpoint) close() { e.once.Do(func() { close(e.done); go e.pc.Close() }) }
func New(cfg Config, hooks Hooks) (*Manager, error) {
	if err := cfg.Validate(); err != nil {
		return nil, err
	}
	if hooks.Allowed == nil {
		return nil, errors.New("screen sharing requires membership authorization")
	}
	m := &Manager{config: cfg, policy: cfg.Policy, hooks: hooks, rooms: map[uint64]*room{}, done: make(chan struct{})}
	if cfg.Enabled {
		api, mux, err := openTransport(cfg)
		if err != nil {
			return nil, err
		}
		m.api, m.udpMux = api, mux
	}
	go m.maintain()
	return m, nil
}

// Every peer connection shares one fixed port. The mux owns its sockets;
// closing an individual peer only removes its ICE ufrag.
func openTransport(cfg Config) (*webrtc.API, ice.UDPMux, error) {
	media := &webrtc.MediaEngine{}
	feedback := []webrtc.RTCPFeedback{{Type: "nack"}, {Type: "nack", Parameter: "pli"}, {Type: "ccm", Parameter: "fir"}, {Type: "goog-remb"}}
	if err := media.RegisterCodec(webrtc.RTPCodecParameters{RTPCodecCapability: webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000, RTCPFeedback: feedback}, PayloadType: 96}, webrtc.RTPCodecTypeVideo); err != nil {
		return nil, nil, err
	}
	if err := media.RegisterCodec(webrtc.RTPCodecParameters{RTPCodecCapability: webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeOpus, ClockRate: 48000, Channels: 2, SDPFmtpLine: "minptime=10;useinbandfec=1;stereo=1"}, PayloadType: 111}, webrtc.RTPCodecTypeAudio); err != nil {
		return nil, nil, err
	}
	registry := &interceptor.Registry{}
	if err := webrtc.RegisterDefaultInterceptors(media, registry); err != nil {
		return nil, nil, err
	}
	settings := webrtc.SettingEngine{}
	mux, err := listenFixedUDPMux(cfg)
	if err != nil {
		return nil, nil, err
	}
	settings.SetICEUDPMux(mux)
	settings.SetNetworkTypes([]webrtc.NetworkType{webrtc.NetworkTypeUDP4, webrtc.NetworkTypeUDP6})
	settings.SetICETimeouts(5*time.Second, 10*time.Second, time.Second)
	return webrtc.NewAPI(webrtc.WithMediaEngine(media), webrtc.WithInterceptorRegistry(registry), webrtc.WithSettingEngine(settings)), mux, nil
}
func (m *Manager) notify(channel uint64, reason string) {
	if m.hooks.Changed != nil {
		m.hooks.Changed(channel, reason)
	}
}
func (m *Manager) Policy() Policy { m.mu.RLock(); defer m.mu.RUnlock(); return m.policy }
func (m *Manager) Status(channel uint64) State {
	m.mu.RLock()
	defer m.mu.RUnlock()
	s := State{ChannelID: channel, Policy: m.policy, Revision: m.snapshotSequence.Add(1)}
	if r := m.rooms[channel]; r != nil {
		s.ShareID = r.id
		s.HostID = r.host
		s.Settings = r.settings
		s.Viewers = len(r.viewers)
		s.Ready = r.videoSeen.Load()
		s.Paused = r.paused
	}
	return s
}

// Changing limits closes existing shares so reductions take effect immediately.
func (m *Manager) SetPolicy(p Policy) error {
	return m.UpdatePolicy(p, nil)
}

// UpdatePolicy binds before saving an enabled policy, then commits the runtime
// change only after persistence succeeds. persist must not call back into m.
// Disabled sharing releases the port; a disabled startup never binds it.
func (m *Manager) UpdatePolicy(p Policy, persist func() error) error {
	if err := p.Validate(); err != nil {
		return err
	}
	m.mu.Lock()
	select {
	case <-m.done:
		m.mu.Unlock()
		return errors.New("screen sharing stopped")
	default:
	}
	if m.policy == p {
		m.mu.Unlock()
		return nil
	}
	api, mux := m.api, m.udpMux
	if p.Enabled && mux == nil {
		var err error
		api, mux, err = openTransport(m.config)
		if err != nil {
			m.mu.Unlock()
			return err
		}
	}
	if persist != nil {
		if err := persist(); err != nil {
			if mux != m.udpMux && mux != nil {
				_ = mux.Close()
			}
			m.mu.Unlock()
			return err
		}
	}
	m.policy = p
	m.api, m.udpMux = api, mux
	old := m.rooms
	m.rooms = map[uint64]*room{}
	for _, r := range old {
		closeRoom(r)
	}
	if !p.Enabled {
		m.closeTransportLocked()
	}
	m.mu.Unlock()
	for _, r := range old {
		m.notify(r.channel, "服务器共享限制已更新，请重新开始共享")
	}
	return nil
}
func (m *Manager) newEndpoint() (*endpoint, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	select {
	case <-m.done:
		return nil, errors.New("screen sharing stopped")
	default:
	}
	if !m.policy.Enabled || m.api == nil {
		return nil, errors.New("服务器未开启屏幕共享")
	}
	pc, err := m.api.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		return nil, err
	}
	e := &endpoint{pc: pc, api: m.api, queue: make(chan mediaPacket, 128), done: make(chan struct{})}
	pc.OnDataChannel(func(c *webrtc.DataChannel) { _ = c.Close() })
	return e, nil
}
func (m *Manager) Publish(ctx context.Context, session, channel uint64, settings Settings, offer string) (State, string, error) {
	if channel == 0 || !m.hooks.Allowed(session, channel) {
		return State{}, "", errors.New("请先进入语音房间")
	}
	e, err := m.newEndpoint()
	if err != nil {
		return State{}, "", err
	}
	var token [16]byte
	if _, err = rand.Read(token[:]); err != nil {
		e.close()
		return State{}, "", err
	}
	r := &room{id: hex.EncodeToString(token[:]), channel: channel, host: session, settings: settings, publisher: e, viewers: map[uint64]*endpoint{}, created: time.Now()}
	m.mu.Lock()
	err = settings.Validate(m.policy)
	if err == nil && e.api != m.api {
		err = errors.New("共享网络设置已改变，请重试")
	}
	if err == nil && m.rooms[channel] != nil {
		err = errors.New("房间已经有人在共享屏幕")
	}
	if err == nil && len(m.rooms) >= m.policy.MaxRooms {
		err = errors.New("服务器同时共享的房间数已达上限")
	}
	select {
	case <-m.done:
		err = errors.New("screen sharing stopped")
	default:
	}
	if err == nil {
		m.rooms[channel] = r
	}
	m.mu.Unlock()
	if err != nil {
		e.close()
		return State{}, "", err
	}
	e.pc.OnTrack(func(track *webrtc.TrackRemote, _ *webrtc.RTPReceiver) { m.receive(r, track) })
	e.pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		if s == webrtc.PeerConnectionStateFailed || s == webrtc.PeerConnectionStateClosed {
			m.end(r, "共享连接已断开")
		}
	})
	answer, err := answerOffer(ctx, e, offer)
	if err == nil {
		err = ctx.Err()
	}
	if err == nil && !m.hooks.Allowed(session, channel) {
		err = errors.New("语音房间已改变")
	}
	m.mu.RLock()
	current := m.rooms[channel] == r
	m.mu.RUnlock()
	if err == nil && !current {
		err = errors.New("共享已结束")
	}
	if err != nil {
		m.end(r, "")
		return State{}, "", err
	}
	m.notify(channel, "")
	return m.Status(channel), answer, nil
}
func answerOffer(ctx context.Context, e *endpoint, offer string) (string, error) {
	if len(offer) == 0 || len(offer) > 48*1024 {
		return "", errors.New("invalid screen share SDP")
	}
	if err := e.pc.SetRemoteDescription(webrtc.SessionDescription{Type: webrtc.SDPTypeOffer, SDP: offer}); err != nil {
		return "", err
	}
	answer, err := e.pc.CreateAnswer(nil)
	if err != nil {
		return "", err
	}
	complete := webrtc.GatheringCompletePromise(e.pc)
	if err = e.pc.SetLocalDescription(answer); err != nil {
		return "", err
	}
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()
	select {
	case <-complete:
	case <-ctx.Done():
		return "", ctx.Err()
	case <-timer.C:
		return "", errors.New("共享网络协商超时")
	case <-e.done:
		return "", errors.New("共享已结束")
	}
	return e.pc.LocalDescription().SDP, nil
}
func (m *Manager) Subscribe(ctx context.Context, session, channel uint64, id, offer string) (State, string, error) {
	if !m.hooks.Allowed(session, channel) {
		return State{}, "", errors.New("请先进入同一个语音房间")
	}
	e, err := m.newEndpoint()
	if err != nil {
		return State{}, "", err
	}
	m.mu.Lock()
	r := m.rooms[channel]
	if e.api != m.api || !m.policy.Enabled {
		err = errors.New("共享网络设置已改变，请重试")
	} else if r == nil || r.id != id || !r.videoSeen.Load() {
		err = errors.New("共享尚未准备好或已结束")
	} else if session == r.host {
		err = errors.New("分享者无需订阅自己")
	} else if r.viewers[session] != nil {
		err = errors.New("已经在观看，请先停止观看")
	} else if len(r.viewers) >= m.policy.MaxViewers {
		err = errors.New("观看人数已达服务器上限")
	}
	if err == nil {
		r.viewers[session] = e
	}
	m.mu.Unlock()
	if err != nil {
		e.close()
		return State{}, "", err
	}
	e.pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		if s == webrtc.PeerConnectionStateFailed || s == webrtc.PeerConnectionStateClosed {
			m.removeViewer(r, session, e)
		}
	})
	e.pc.OnTrack(func(_ *webrtc.TrackRemote, _ *webrtc.RTPReceiver) { m.removeViewer(r, session, e) })
	e.video, err = webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8, ClockRate: 90000}, "screen-video", r.id)
	if err == nil {
		err = m.addTrack(r, e, e.video)
	}
	if err == nil && r.settings.Audio {
		e.audio, err = webrtc.NewTrackLocalStaticRTP(webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeOpus, ClockRate: 48000, Channels: 2}, "screen-audio", r.id)
		if err == nil {
			err = m.addTrack(r, e, e.audio)
		}
	}
	var answer string
	if err == nil {
		answer, err = answerOffer(ctx, e, offer)
		if err == nil {
			err = ctx.Err()
		}
	}
	if err == nil && !m.hooks.Allowed(session, channel) {
		err = errors.New("语音房间已改变")
	}
	m.mu.RLock()
	current := m.rooms[channel] == r && r.viewers[session] == e
	m.mu.RUnlock()
	if err == nil && !current {
		err = errors.New("共享已结束")
	}
	if err != nil {
		m.removeViewer(r, session, e)
		return State{}, "", err
	}
	go m.writeViewer(r, session, e)
	m.requestKeyframe(r)
	m.notify(channel, "")
	return m.Status(channel), answer, nil
}
func (m *Manager) addTrack(r *room, e *endpoint, track *webrtc.TrackLocalStaticRTP) error {
	sender, err := e.pc.AddTrack(track)
	if err != nil {
		return err
	}
	go func() {
		for {
			packets, _, err := sender.ReadRTCP()
			if err != nil {
				return
			}
			for _, p := range packets {
				switch p.(type) {
				case *rtcp.PictureLossIndication, *rtcp.FullIntraRequest:
					m.requestKeyframe(r)
				}
			}
		}
	}()
	return nil
}
func (m *Manager) requestKeyframe(r *room) {
	ssrc := r.videoSSRC.Load()
	if ssrc == 0 {
		return
	}
	now := time.Now().UnixNano()
	last := r.lastPLI.Load()
	if now-last < int64(400*time.Millisecond) || !r.lastPLI.CompareAndSwap(last, now) {
		return
	}
	_ = r.publisher.pc.WriteRTCP([]rtcp.Packet{&rtcp.PictureLossIndication{MediaSSRC: ssrc}})
}
func (m *Manager) receive(r *room, track *webrtc.TrackRemote) {
	video := track.Kind() == webrtc.RTPCodecTypeVideo
	if video {
		if track.Codec().MimeType != webrtc.MimeTypeVP8 || r.videoSeen.Swap(true) {
			m.end(r, "共享视频格式无效")
			return
		}
		r.videoSSRC.Store(uint32(track.SSRC()))
		m.notify(r.channel, "")
	} else if !r.settings.Audio || track.Codec().MimeType != webrtc.MimeTypeOpus || r.audioSeen.Swap(true) {
		m.end(r, "共享音轨不符合设置")
		return
	}
	guard := newMediaGuard(r.settings, video, time.Now())
	for {
		p, _, err := track.ReadRTP()
		if err != nil {
			return
		}
		now := time.Now()
		if !guard.accept(p, now) {
			m.end(r, "共享媒体超过约定的码率、分辨率或帧率限制")
			return
		}
		// Padding probes belong to the publisher transport, not the viewers'
		// video stream. Do not forward them or request decoding/keyframes.
		if len(p.Payload) == 0 {
			continue
		}
		if !m.hooks.Allowed(r.host, r.channel) {
			m.end(r, "")
			return
		}
		dropped := false
		m.mu.RLock()
		if m.rooms[r.channel] != r {
			m.mu.RUnlock()
			return
		}
		for id, e := range r.viewers {
			if !m.hooks.Allowed(id, r.channel) {
				continue
			}
			select {
			case e.queue <- mediaPacket{packet: p.Clone(), video: video, at: now}:
			default:
				dropped = true
			}
		}
		m.mu.RUnlock()
		if dropped {
			m.requestKeyframe(r)
		}
	}
}
func (m *Manager) writeViewer(r *room, session uint64, e *endpoint) {
	for {
		select {
		case <-e.done:
			return
		case p := <-e.queue:
			if !m.hooks.Allowed(session, r.channel) {
				m.removeViewer(r, session, e)
				return
			}
			if time.Since(p.at) > 150*time.Millisecond {
				m.requestKeyframe(r)
				continue
			}
			track := e.audio
			if p.video {
				track = e.video
			}
			if track == nil {
				continue
			}
			p.packet.Header.Extension = false
			p.packet.Header.Extensions = nil
			if err := track.WriteRTP(p.packet); err != nil {
				m.removeViewer(r, session, e)
				return
			}
		}
	}
}

// Pause records the publisher's privacy state. Media keeps flowing so clients
// receive the publisher's safe replacement video instead of freezing a private
// last frame. It never mutates voice state or shared media queues.
func (m *Manager) Pause(session, channel uint64, id string, paused bool) error {
	m.mu.Lock()
	r := m.rooms[channel]
	if r == nil || r.id != id {
		m.mu.Unlock()
		return errors.New("共享已结束")
	}
	if r.host != session || !m.hooks.Allowed(session, channel) {
		m.mu.Unlock()
		return errors.New("仅当前房间的分享者可以切换隐私暂停")
	}
	changed := r.paused != paused
	r.paused = paused
	m.mu.Unlock()
	if changed {
		m.notify(channel, "")
	}
	return nil
}

func (m *Manager) Stop(session, channel uint64, id string, moderator bool) error {
	m.mu.RLock()
	r := m.rooms[channel]
	m.mu.RUnlock()
	if r == nil || r.id != id {
		return errors.New("共享已结束")
	}
	if !m.hooks.Allowed(session, channel) || (r.host != session && !moderator) {
		return errors.New("仅分享者或管理员可以结束共享")
	}
	m.end(r, "")
	return nil
}
func (m *Manager) Leave(session, channel uint64, id string) {
	m.mu.RLock()
	r := m.rooms[channel]
	var e *endpoint
	if r != nil && r.id == id {
		e = r.viewers[session]
	}
	m.mu.RUnlock()
	if e != nil {
		m.removeViewer(r, session, e)
	}
}
func (m *Manager) RemoveSession(session uint64) {
	m.mu.RLock()
	var owned []*room
	type subscription struct {
		r *room
		e *endpoint
	}
	var views []subscription
	for _, r := range m.rooms {
		if r.host == session {
			owned = append(owned, r)
		} else if e := r.viewers[session]; e != nil {
			views = append(views, subscription{r, e})
		}
	}
	m.mu.RUnlock()
	for _, r := range owned {
		m.end(r, "")
	}
	for _, v := range views {
		m.removeViewer(v.r, session, v.e)
	}
}
func (m *Manager) removeViewer(r *room, session uint64, e *endpoint) {
	m.mu.Lock()
	changed := m.rooms[r.channel] == r && r.viewers[session] == e
	if changed {
		delete(r.viewers, session)
	}
	m.mu.Unlock()
	e.close()
	if changed {
		m.notify(r.channel, "")
	}
}

// Caller holds the manager lock. Close cannot block or invoke callbacks inline.
func closeRoom(r *room) {
	r.publisher.close()
	for _, e := range r.viewers {
		e.close()
	}
}
func (m *Manager) end(r *room, reason string) {
	m.mu.Lock()
	current := m.rooms[r.channel] == r
	if current {
		delete(m.rooms, r.channel)
		closeRoom(r)
	}
	m.mu.Unlock()
	if current {
		m.notify(r.channel, reason)
	}
}
func (m *Manager) maintain() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-m.done:
			return
		case now := <-ticker.C:
			m.mu.RLock()
			var expired []*room
			var departed []uint64
			for _, r := range m.rooms {
				if !m.hooks.Allowed(r.host, r.channel) || (!r.videoSeen.Load() && now.Sub(r.created) > 20*time.Second) {
					expired = append(expired, r)
				}
				for id := range r.viewers {
					if !m.hooks.Allowed(id, r.channel) {
						departed = append(departed, id)
					}
				}
			}
			m.mu.RUnlock()
			for _, r := range expired {
				m.end(r, "共享连接已结束")
			}
			for _, id := range departed {
				m.RemoveSession(id)
			}
		}
	}
}
func (m *Manager) Close() {
	m.once.Do(func() {
		m.mu.Lock()
		close(m.done)
		for _, r := range m.rooms {
			closeRoom(r)
		}
		m.rooms = map[uint64]*room{}
		m.closeTransportLocked()
		m.mu.Unlock()
	})
}

func (m *Manager) closeTransportLocked() {
	if m.udpMux != nil {
		_ = m.udpMux.Close()
		m.udpMux = nil
	}
	m.api = nil
}
