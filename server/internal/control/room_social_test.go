package control

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"image"
	"image/jpeg"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"testing"
	"time"

	"edenvoice/server/internal/chatstore"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/roomimage"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
)

func socialJPEG(t *testing.T) []byte {
	t.Helper()
	var b bytes.Buffer
	if err := jpeg.Encode(&b, image.NewRGBA(image.Rect(0, 0, 48, 32)), nil); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}
func socialServer(t *testing.T) (*Server, string) {
	t.Helper()
	state := serverstate.New(config.Default())
	dir := t.TempDir()
	store, err := chatstore.Open(filepath.Join(dir, "chat"))
	if err != nil {
		t.Fatal(err)
	}
	if err = state.RestoreChat(store); err != nil {
		t.Fatal(err)
	}
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	if err = s.ConfigureRoomImages(filepath.Join(dir, "images")); err != nil {
		t.Fatal(err)
	}
	return s, dir
}

type socialRecordingSender struct {
	mu       sync.Mutex
	messages []protocol.Envelope
}

func (s *socialRecordingSender) Send(message protocol.Envelope) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.messages = append(s.messages, message)
	return nil
}
func (s *socialRecordingSender) snapshot() []protocol.Envelope {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]protocol.Envelope(nil), s.messages...)
}
func waitRoomImageJobs(t *testing.T, s *Server) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		s.social.mu.Lock()
		done := len(s.social.diskJobs) == 0
		s.social.mu.Unlock()
		if done {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("image jobs did not finish")
}
func socialMember(t *testing.T, s *Server, channel uint64) (uint64, *socialRecordingSender) {
	t.Helper()
	out := &socialRecordingSender{}
	a := s.state.AddAuthenticatedSession("member", "authenticated", "", "", out, nil, nil)
	if err := s.state.Join(a.ID, channel); err != nil {
		t.Fatal(err)
	}
	return a.ID, out
}
func callSocial(t *testing.T, s *Server, id uint64, out *socialRecordingSender, req protocol.RoomSocialRequest) protocol.RoomSocialEvent {
	t.Helper()
	if req.RequestID == "" {
		req.RequestID = "test"
	}
	if req.ChannelID == 0 {
		req.ChannelID = 1
	}
	before := len(out.snapshot())
	e, _ := protocol.NewEnvelope(protocol.TypeRoomSocialRequest, req)
	if err := s.dispatch(id, e, out); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		for _, envelope := range out.snapshot()[before:] {
			if envelope.Type == protocol.TypeRoomSocialEvent {
				var event protocol.RoomSocialEvent
				if err := envelope.Decode(&event); err != nil {
					t.Fatal(err)
				}
				if event.RequestID != req.RequestID {
					continue
				}
				if req.Action == "image_commit" || req.Action == "image_get" {
					waitRoomImageJobs(t, s)
				}
				return event
			}
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("missing social response")
	return protocol.RoomSocialEvent{}
}
func beginSocial(t *testing.T, s *Server, id uint64, out *socialRecordingSender, data []byte) string {
	t.Helper()
	e := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_begin", Width: 48, Height: 32, Bytes: len(data)})
	if e.Kind != "image_upload" || e.UploadID == "" {
		t.Fatalf("begin: %+v", e)
	}
	return e.UploadID
}
func uploadSocial(t *testing.T, s *Server, id uint64, out *socialRecordingSender, data []byte) protocol.RoomSocialEvent {
	t.Helper()
	upload := beginSocial(t, s, id, out, data)
	e := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_chunk", UploadID: upload, Data: base64.StdEncoding.EncodeToString(data)})
	if e.Kind != "image_upload" || e.Offset != len(data) {
		t.Fatalf("chunk: %+v", e)
	}
	return callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_commit", UploadID: upload})
}
func TestRoomSocialImagePersistenceAndNoBase64History(t *testing.T) {
	s, dir := socialServer(t)
	id, out := socialMember(t, s, 1)
	data := socialJPEG(t)
	if !slices.Contains(s.serverFeatures(), "room_social_v1") || !slices.Contains(s.serverFeatures(), "room_images_v1") {
		t.Fatal("features missing")
	}
	event := uploadSocial(t, s, id, out, data)
	if event.Kind != "image_committed" || event.Image == nil {
		t.Fatalf("commit: %+v", event)
	}
	history := s.state.ChatHistory(1)
	if len(history) != 1 || history[0].Image == nil || history[0].Text != "[图片]" {
		t.Fatalf("history: %+v", history)
	}
	raw, err := os.ReadFile(filepath.Join(dir, "chat", "1.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(raw, []byte(base64.StdEncoding.EncodeToString(data))) || bytes.Contains(raw, []byte(`"data"`)) {
		t.Fatal("binary image entered chat log")
	}
	reopened, err := chatstore.Open(filepath.Join(dir, "chat"))
	if err != nil {
		t.Fatal(err)
	}
	restored, err := reopened.Recent(1, 100)
	if err != nil || len(restored) != 1 || restored[0].Image.Reference != event.Image.Reference {
		t.Fatal("image metadata did not survive restart")
	}
	images, err := roomimage.Open(filepath.Join(dir, "images"))
	if err != nil {
		t.Fatal(err)
	}
	_, imageData, _, err := images.Get(1, event.Image.Reference, 0, time.Now())
	if err != nil || !bytes.Equal(data, imageData) {
		t.Fatal("image did not survive restart")
	}
	other, otherOut := socialMember(t, s, 2)
	got := callSocial(t, s, other, otherOut, protocol.RoomSocialRequest{Action: "image_get", ChannelID: 1, Reference: event.Image.Reference})
	if got.Code != "forbidden" {
		t.Fatalf("spoofed room read: %+v", got)
	}
	got = callSocial(t, s, other, otherOut, protocol.RoomSocialRequest{Action: "image_get", ChannelID: 2, Reference: event.Image.Reference})
	if got.Code != "image_missing" {
		t.Fatalf("reference crossed rooms: %+v", got)
	}
	s.social.mu.Lock()
	s.social.outbound = socialBucket{}
	s.social.mu.Unlock()
	got = callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_get", Reference: event.Image.Reference})
	if got.Kind != "image_data" || !got.EOF || got.Data != base64.StdEncoding.EncodeToString(data) {
		t.Fatalf("get: %+v", got)
	}
	got = callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_get", Reference: event.Image.Reference})
	if got.Code != "busy" || got.RetryAfterMS == 0 {
		t.Fatal("global/session budget not applied")
	}
}
func TestImageRejectsMalformedDataAndPersistenceFailure(t *testing.T) {
	s, dir := socialServer(t)
	id, out := socialMember(t, s, 1)
	for _, req := range []protocol.RoomSocialRequest{{Action: "image_begin", Width: 1601, Height: 1, Bytes: 64}, {Action: "image_begin", Width: 1, Height: 1, Bytes: protocol.MaxRoomImageBytes + 1}, {Action: "image_begin", Width: -1, Height: 1, Bytes: 64}} {
		if e := callSocial(t, s, id, out, req); e.Code != "invalid_image" {
			t.Fatalf("invalid header: %+v", e)
		}
	}
	data := socialJPEG(t)
	upload := beginSocial(t, s, id, out, data)
	for _, req := range []protocol.RoomSocialRequest{{Action: "image_chunk", UploadID: upload, Data: "not base64"}, {Action: "image_chunk", UploadID: upload, Data: base64.StdEncoding.EncodeToString(data), Offset: 1}, {Action: "image_chunk", UploadID: upload, Data: base64.StdEncoding.EncodeToString(make([]byte, 8193))}} {
		if e := callSocial(t, s, id, out, req); e.Code != "invalid_chunk" {
			t.Fatalf("invalid chunk: %+v", e)
		}
	}
	if e := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_commit", UploadID: upload}); e.Code != "incomplete_image" {
		t.Fatal("incomplete image accepted")
	}
	callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_cancel", UploadID: upload})
	if e := uploadSocial(t, s, id, out, []byte("not jpeg")); e.Code != "invalid_image" {
		t.Fatalf("malformed JPEG: %+v", e)
	}
	originalImages := s.social.images
	for _, test := range []struct {
		err  error
		code string
	}{
		{roomimage.ErrFull, "storage_full"},
		{errors.New("open /private/internal/images/blob: permission denied"), "unavailable"},
	} {
		s.social.images = failingRoomImages{roomImageStore: originalImages, err: test.err}
		s.social.mu.Lock()
		s.social.sessions[id].requests = socialBucket{}
		s.social.mu.Unlock()
		e := uploadSocial(t, s, id, out, data)
		if e.Code != test.code || bytes.Contains([]byte(e.Message), []byte("/private/")) {
			t.Fatalf("unsafe persistence error: %+v", e)
		}
	}
	s.social.images = originalImages
	_ = s.state.RestoreChat(nil)
	if e := uploadSocial(t, s, id, out, data); e.Code != "message_failed" {
		t.Fatalf("unsaved message accepted: %+v", e)
	}
	if len(s.state.ChatHistory(1)) != 0 {
		t.Fatal("failed image message appeared")
	}
	files, _ := os.ReadDir(filepath.Join(dir, "images"))
	if len(files) != 0 {
		t.Fatal("failed image left a published attachment")
	}
}
func TestImageUploadsBoundedExpireAndCancelOnRoomChange(t *testing.T) {
	s, _ := socialServer(t)
	var ids []uint64
	var outs []*socialRecordingSender
	for i := 0; i < 5; i++ {
		id, out := socialMember(t, s, 1)
		ids = append(ids, id)
		outs = append(outs, out)
		event := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_begin", Width: 1600, Height: 1600, Bytes: protocol.MaxRoomImageBytes})
		if i < 4 && event.Kind != "image_upload" {
			t.Fatalf("upload %d: %+v", i, event)
		}
		if i == 4 && event.Code != "busy" {
			t.Fatal("global upload memory exceeded")
		}
	}
	if s.social.pendingBytes != maxPendingImageBytes {
		t.Fatal("upload reservation accounting")
	}
	oldUpload := s.social.sessions[ids[0]].upload.id
	_ = s.state.Join(ids[0], 2)
	_ = s.state.Join(ids[0], 1)
	event := callSocial(t, s, ids[0], outs[0], protocol.RoomSocialRequest{Action: "image_commit", UploadID: oldUpload})
	if event.Code != "upload_missing" {
		t.Fatal("room re-entry reused old upload")
	}
	s.cleanupRoomSocial(time.Now().Add(imageUploadTTL + time.Second))
	if s.social.pendingBytes != 0 || s.social.activeUploads != 0 {
		t.Fatal("expired uploads not released")
	}
	id, out := ids[0], outs[0]
	upload := beginSocial(t, s, id, out, socialJPEG(t))
	callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_cancel", UploadID: upload})
	if s.social.pendingBytes != 0 {
		t.Fatal("cancel retained bytes")
	}
	beginSocial(t, s, id, out, socialJPEG(t))
	s.disconnect(id, false)
	if s.social.pendingBytes != 0 {
		t.Fatal("disconnect retained upload")
	}
}
func TestRoomReactionAggregateLimit(t *testing.T) {
	s, _ := socialServer(t)
	success, busy := 0, 0
	for i := 0; i < 9; i++ {
		id, out := socialMember(t, s, 1)
		event := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "reaction", Emoji: "🎉"})
		if event.Kind == "reaction" {
			success++
		}
		if event.Code == "busy" {
			busy++
		}
	}
	if success != 6 || busy != 3 {
		t.Fatalf("room burst bound: success=%d busy=%d", success, busy)
	}
}
func TestRoomSocialOverAuthenticatedTLSAndLegacyCompatibility(t *testing.T) {
	s, _ := socialServer(t)
	identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "identity.json"))
	if err != nil {
		t.Fatal(err)
	}
	tlsConfig, err := identity.TLSConfig("test")
	if err != nil {
		t.Fatal(err)
	}
	s.identity = identity
	s.tlsConfig = tlsConfig
	connect := func(name string) *testClient {
		_, priv, _ := ed25519.GenerateKey(rand.Reader)
		c := newSecureTestClient(t, s, tlsConfig, name, priv)
		t.Cleanup(func() { _ = c.conn.Close() })
		return c
	}
	a, legacy, b := connect("a"), connect("legacy"), connect("b")
	for i, c := range []*testClient{a, legacy, b} {
		room := uint64(1)
		if i == 2 {
			room = 2
		}
		c.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: room})
		c.receiveUntil(t, protocol.TypeChatHistory)
	}
	request := func(c *testClient, req protocol.RoomSocialRequest) protocol.RoomSocialEvent {
		t.Helper()
		if req.RequestID == "" {
			req.RequestID = fmt.Sprintf("%s-%d", req.Action, time.Now().UnixNano())
		}
		c.send(t, protocol.TypeRoomSocialRequest, req)
		for i := 0; i < 64; i++ {
			envelope := c.receive(t)
			if envelope.Type == protocol.TypeRoomSocialEvent {
				var event protocol.RoomSocialEvent
				_ = envelope.Decode(&event)
				if event.RequestID == req.RequestID {
					return event
				}
			}
		}
		t.Fatal("missing correlated TLS response")
		return protocol.RoomSocialEvent{}
	}
	if e := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "away", Active: socialActive(true)}); e.Kind != "away" || !e.Active || e.SessionID != a.sessionID {
		t.Fatalf("TLS away: %+v", e)
	}
	a.send(t, protocol.TypeSetMute, protocol.StateRequest{Enabled: false})
	var returned protocol.RoomSocialEvent
	if err := a.receiveUntil(t, protocol.TypeRoomSocialEvent).Decode(&returned); err != nil {
		t.Fatal(err)
	}
	if returned.Kind != "away" || returned.Active || returned.RequestID != "" {
		t.Fatalf("TLS unmute did not clear away: %+v", returned)
	}
	var unmuted protocol.UserStateChanged
	if err := a.receiveUntil(t, protocol.TypeUserStateChanged).Decode(&unmuted); err != nil || unmuted.Muted {
		t.Fatal("TLS unmute state missing")
	}
	if e := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "reaction", Emoji: "👍"}); e.Kind != "reaction" {
		t.Fatalf("TLS reaction: %+v", e)
	}
	if e := request(b, protocol.RoomSocialRequest{ChannelID: 1, Action: "reaction", Emoji: "👍"}); e.Code != "forbidden" {
		t.Fatal("TLS cross room reaction allowed")
	}
	data := socialJPEG(t)
	begin := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "image_begin", Width: 48, Height: 32, Bytes: len(data)})
	if begin.Kind != "image_upload" {
		t.Fatalf("TLS begin: %+v", begin)
	}
	chunk := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "image_chunk", UploadID: begin.UploadID, Data: base64.StdEncoding.EncodeToString(data)})
	if chunk.Offset != len(data) {
		t.Fatal("TLS chunk")
	}
	committed := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "image_commit", UploadID: begin.UploadID})
	if committed.Kind != "image_committed" || committed.Image == nil {
		t.Fatalf("TLS commit: %+v", committed)
	}
	if e := request(a, protocol.RoomSocialRequest{ChannelID: 1, Action: "image_get", Reference: committed.Image.Reference}); e.Data != base64.StdEncoding.EncodeToString(data) {
		t.Fatalf("TLS image fetch: %+v", e)
	}
	s.social.mu.Lock()
	s.social.outbound = socialBucket{}
	s.social.mu.Unlock()
	if e := request(b, protocol.RoomSocialRequest{ChannelID: 2, Action: "image_get", Reference: committed.Image.Reference}); e.Code != "image_missing" {
		t.Fatalf("TLS reference crossed rooms: %+v", e)
	}
	legacy.send(t, protocol.TypePing, protocol.Ping{Nonce: 7})
	sawImage, sawAwayMute := false, false
	for i := 0; i < 64; i++ {
		e := legacy.receive(t)
		if e.Type == protocol.TypeRoomSocialEvent {
			t.Fatal("legacy received unknown social envelope")
		}
		if e.Type == protocol.TypeUserStateChanged {
			var voice protocol.UserStateChanged
			_ = e.Decode(&voice)
			if voice.SessionID == a.sessionID && voice.Muted {
				sawAwayMute = true
			}
		}
		if e.Type == protocol.TypeTextMessage {
			var msg protocol.TextMessage
			_ = e.Decode(&msg)
			sawImage = msg.Text == "[图片]" && msg.Image != nil
		}
		if e.Type == protocol.TypePong {
			break
		}
	}
	if !sawImage || !sawAwayMute {
		t.Fatal("legacy did not receive compatible image placeholder and away mute")
	}
	b.send(t, protocol.TypePing, protocol.Ping{Nonce: 8})
	for i := 0; i < 64; i++ {
		e := b.receive(t)
		if e.Type == protocol.TypeTextMessage {
			t.Fatal("image message crossed room")
		}
		if e.Type == protocol.TypePong {
			break
		}
	}
}

type blockedRoomImages struct {
	roomImageStore
	entered, release chan struct{}
	blockGet         bool
}

func (b *blockedRoomImages) Put(channel uint64, data []byte, w, h int, now time.Time) (protocol.ChatImage, error) {
	if !b.blockGet {
		close(b.entered)
		<-b.release
	}
	return b.roomImageStore.Put(channel, data, w, h, now)
}
func (b *blockedRoomImages) Get(channel uint64, ref string, offset int, now time.Time) (protocol.ChatImage, []byte, bool, error) {
	if b.blockGet {
		close(b.entered)
		<-b.release
	}
	return b.roomImageStore.Get(channel, ref, offset, now)
}
func TestImageDiskWaitDoesNotBlockRoomMoveOrSocialAndCannotLeakAfterMove(t *testing.T) {
	for _, get := range []bool{false, true} {
		t.Run(fmt.Sprintf("get=%t", get), func(t *testing.T) {
			s, dir := socialServer(t)
			id, out := socialMember(t, s, 1)
			other, otherOut := socialMember(t, s, 1)
			data := socialJPEG(t)
			req := protocol.RoomSocialRequest{RequestID: "blocked", ChannelID: 1}
			if get {
				image, err := s.social.images.Put(1, data, 48, 32, time.Now())
				if err != nil {
					t.Fatal(err)
				}
				req.Action = "image_get"
				req.Reference = image.Reference
			} else {
				upload := beginSocial(t, s, id, out, data)
				callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "image_chunk", UploadID: upload, Data: base64.StdEncoding.EncodeToString(data)})
				req.Action = "image_commit"
				req.UploadID = upload
			}
			blocker := &blockedRoomImages{roomImageStore: s.social.images, entered: make(chan struct{}), release: make(chan struct{}), blockGet: get}
			s.social.images = blocker
			env, _ := protocol.NewEnvelope(protocol.TypeRoomSocialRequest, req)
			finished := make(chan struct{})
			go func() { _ = s.dispatch(id, env, out); close(finished) }()
			select {
			case <-blocker.entered:
			case <-time.After(time.Second):
				t.Fatal("disk operation not reached")
			}
			moved := make(chan struct{})
			go func() { _ = s.state.Join(id, 2); s.broadcastMove(id, 2); close(moved) }()
			select {
			case <-moved:
			case <-time.After(time.Second):
				close(blocker.release)
				t.Fatal("disk wait blocked room move")
			}
			status := make(chan protocol.RoomSocialEvent, 1)
			go func() { status <- callSocial(t, s, other, otherOut, protocol.RoomSocialRequest{Action: "status"}) }()
			select {
			case event := <-status:
				if event.Kind != "status" {
					t.Fatalf("status during disk wait: %+v", event)
				}
			case <-time.After(time.Second):
				close(blocker.release)
				t.Fatal("disk wait blocked another member status")
			}
			if !get && s.social.pendingBytes != len(data) {
				t.Fatal("in-flight disk task lost its memory reservation on move")
			}
			close(blocker.release)
			select {
			case <-finished:
			case <-time.After(time.Second):
				t.Fatal("disk task did not finish")
			}
			waitRoomImageJobs(t, s)
			for _, event := range out.snapshot() {
				if event.Type == protocol.TypeRoomSocialEvent {
					var e protocol.RoomSocialEvent
					_ = event.Decode(&e)
					if e.RequestID == "blocked" && (e.Kind == "image_data" || e.Kind == "image_committed") {
						t.Fatal("private image data delivered after room change")
					}
				}
			}
			if !get {
				if s.social.pendingBytes != 0 || len(s.state.ChatHistory(1)) != 0 {
					t.Fatal("moved upload published or leaked reservation")
				}
				files, _ := os.ReadDir(filepath.Join(dir, "images"))
				if len(files) != 0 {
					t.Fatal("cancelled commit left attachment files")
				}
			}
		})
	}
}

type failingRoomImages struct {
	roomImageStore
	err error
}

func (f failingRoomImages) Put(uint64, []byte, int, int, time.Time) (protocol.ChatImage, error) {
	return protocol.ChatImage{}, f.err
}

func socialActive(active bool) *bool { return &active }
func TestAwayRequiresExplicitBooleanAndUnmuteClearsWithoutTimeout(t *testing.T) {
	s, _ := socialServer(t)
	id, out := socialMember(t, s, 1)
	for _, payload := range []map[string]any{
		{"request_id": "missing", "channel_id": 1, "action": "away"},
		{"request_id": "old-timed", "channel_id": 1, "action": "away", "minutes": 5},
		{"request_id": "null", "channel_id": 1, "action": "away", "active": nil},
		{"request_id": "string", "channel_id": 1, "action": "away", "active": "true"},
	} {
		env, _ := protocol.NewEnvelope(protocol.TypeRoomSocialRequest, payload)
		if err := s.dispatch(id, env, out); err != nil {
			t.Fatal(err)
		}
		var event protocol.RoomSocialEvent
		messages := out.snapshot()
		_ = messages[len(messages)-1].Decode(&event)
		if event.Code != "invalid_request" {
			t.Fatalf("invalid away accepted: %+v", event)
		}
	}
	event := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "away", Active: socialActive(true)})
	if event.Kind != "away" || !event.Active || s.state.CanSpeak(id) {
		t.Fatalf("away did not close microphone: %+v", event)
	}
	s.cleanupRoomSocial(time.Now().Add(48 * time.Hour))
	status := callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "status"})
	if len(status.Away) != 1 {
		t.Fatal("cleanup expired indefinite away")
	}
	for _, enabled := range []bool{true, false} {
		env, _ := protocol.NewEnvelope(protocol.TypeSetDeafen, protocol.StateRequest{Enabled: enabled})
		if err := s.dispatch(id, env, out); err != nil {
			t.Fatal(err)
		}
	}
	status = callSocial(t, s, id, out, protocol.RoomSocialRequest{Action: "status"})
	if len(status.Away) != 1 {
		t.Fatal("deafen changed away")
	}
	start := len(out.snapshot())
	env, _ := protocol.NewEnvelope(protocol.TypeSetMute, protocol.StateRequest{Enabled: false})
	if err := s.dispatch(id, env, out); err != nil {
		t.Fatal(err)
	}
	cleared, unmuted := false, false
	for _, message := range out.snapshot()[start:] {
		switch message.Type {
		case protocol.TypeRoomSocialEvent:
			var event protocol.RoomSocialEvent
			_ = message.Decode(&event)
			cleared = event.Kind == "away" && !event.Active
		case protocol.TypeUserStateChanged:
			var state protocol.UserStateChanged
			_ = message.Decode(&state)
			unmuted = !state.Muted
		}
	}
	if !cleared || !unmuted || !s.state.CanSpeak(id) {
		t.Fatal("normal microphone command did not clear away and notify peers")
	}
}
