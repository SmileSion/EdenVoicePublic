package control

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
)

type gatedRoomImages struct {
	roomImageStore
	entered chan struct{}
	release chan struct{}
	fail    bool
}

func (g *gatedRoomImages) Put(channel uint64, data []byte, width, height int, now time.Time) (protocol.ChatImage, error) {
	g.entered <- struct{}{}
	<-g.release
	if g.fail {
		return protocol.ChatImage{}, errors.New("private storage path must not escape")
	}
	return g.roomImageStore.Put(channel, data, width, height, now)
}
func (g *gatedRoomImages) Get(channel uint64, reference string, offset int, now time.Time) (protocol.ChatImage, []byte, bool, error) {
	g.entered <- struct{}{}
	<-g.release
	if g.fail {
		return protocol.ChatImage{}, nil, false, errors.New("private storage path must not escape")
	}
	return g.roomImageStore.Get(channel, reference, offset, now)
}

func socialTLSConnector(t *testing.T, s *Server) func() *testClient {
	t.Helper()
	identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "identity.json"))
	if err != nil {
		t.Fatal(err)
	}
	tlsConfig, err := identity.TLSConfig("test")
	if err != nil {
		t.Fatal(err)
	}
	s.identity, s.tlsConfig = identity, tlsConfig
	return func() *testClient {
		t.Helper()
		_, key, err := ed25519.GenerateKey(rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		c := newSecureTestClient(t, s, tlsConfig, "image-client", key)
		t.Cleanup(func() { _ = c.conn.(*tls.Conn).NetConn().Close() })
		c.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 1})
		c.receiveUntil(t, protocol.TypeChatHistory)
		return c
	}
}

func tlsSocialRequest(t *testing.T, c *testClient, req protocol.RoomSocialRequest) protocol.RoomSocialEvent {
	t.Helper()
	if req.ChannelID == 0 {
		req.ChannelID = 1
	}
	if req.RequestID == "" {
		req.RequestID = fmt.Sprintf("%s-%d", req.Action, time.Now().UnixNano())
	}
	c.send(t, protocol.TypeRoomSocialRequest, req)
	for range 64 {
		env := c.receive(t)
		if env.Type != protocol.TypeRoomSocialEvent {
			continue
		}
		var event protocol.RoomSocialEvent
		if err := env.Decode(&event); err != nil {
			t.Fatal(err)
		}
		if event.RequestID == req.RequestID {
			return event
		}
	}
	t.Fatal("missing social TLS reply")
	return protocol.RoomSocialEvent{}
}

func tlsImageUpload(t *testing.T, c *testClient, data []byte) string {
	t.Helper()
	begin := tlsSocialRequest(t, c, protocol.RoomSocialRequest{Action: "image_begin", Width: 48, Height: 32, Bytes: len(data)})
	if begin.Kind != "image_upload" {
		t.Fatalf("begin: %+v", begin)
	}
	chunk := tlsSocialRequest(t, c, protocol.RoomSocialRequest{Action: "image_chunk", UploadID: begin.UploadID, Data: base64.StdEncoding.EncodeToString(data)})
	if chunk.Kind != "image_upload" || chunk.Offset != len(data) {
		t.Fatalf("chunk: %+v", chunk)
	}
	return begin.UploadID
}

func waitImageEntry(t *testing.T, gate *gatedRoomImages) {
	t.Helper()
	select {
	case <-gate.entered:
	case <-time.After(time.Second):
		t.Fatal("background image task did not enter storage")
	}
}

func TestSlowImageStoreKeepsSameTLSConnectionResponsiveAndDropsObsoleteResults(t *testing.T) {
	for _, test := range []struct {
		action, end string
		fail        bool
	}{
		{"image_commit", "leave", false}, {"image_get", "leave", false},
		{"image_commit", "disconnect", false}, {"image_get", "disconnect", false},
		{"image_commit", "cancel", false},
		{"image_commit", "cancel", true},
		{"image_commit", "leave", true}, {"image_get", "leave", true},
	} {
		t.Run(fmt.Sprintf("%s/%s/error=%t", test.action, test.end, test.fail), func(t *testing.T) {
			s, dir := socialServer(t)
			data := socialJPEG(t)
			req := protocol.RoomSocialRequest{RequestID: "blocked-image", ChannelID: 1, Action: test.action}
			if test.action == "image_get" {
				image, err := s.social.images.Put(1, data, 48, 32, time.Now())
				if err != nil {
					t.Fatal(err)
				}
				req.Reference = image.Reference
			}
			gate := &gatedRoomImages{roomImageStore: s.social.images, entered: make(chan struct{}, 2), release: make(chan struct{}), fail: test.fail}
			s.social.images = gate
			defer func() {
				select {
				case <-gate.release:
				default:
					close(gate.release)
				}
				waitRoomImageJobs(t, s)
			}()
			c := socialTLSConnector(t, s)()
			upload := tlsImageUpload(t, c, data)
			if test.action == "image_commit" {
				req.UploadID = upload
			}
			c.send(t, protocol.TypeRoomSocialRequest, req)
			waitImageEntry(t, gate)

			started := time.Now()
			c.send(t, protocol.TypePing, protocol.Ping{Nonce: 51})
			c.receiveUntil(t, protocol.TypePong)
			c.send(t, protocol.TypeSetMute, protocol.StateRequest{Enabled: true})
			var muted protocol.UserStateChanged
			if err := c.receiveUntil(t, protocol.TypeUserStateChanged).Decode(&muted); err != nil || !muted.Muted {
				t.Fatal("mute stalled behind storage")
			}
			cancelID := upload
			if test.action == "image_commit" && test.end != "cancel" {
				cancelID = "unrelated-upload"
			}
			if test.end == "cancel" {
				c.send(t, protocol.TypeRoomSocialRequest, protocol.RoomSocialRequest{RequestID: "cancel", ChannelID: 1, Action: "image_cancel", UploadID: cancelID})
				var endedCommit, acknowledgedCancel bool
				for range 64 {
					env := c.receive(t)
					if env.Type != protocol.TypeRoomSocialEvent {
						continue
					}
					var event protocol.RoomSocialEvent
					_ = env.Decode(&event)
					if event.RequestID == req.RequestID {
						endedCommit = event.Kind == "error" && event.Code == "upload_missing"
					}
					if event.RequestID == "cancel" {
						acknowledgedCancel = event.Kind == "image_cancelled"
					}
					if endedCommit && acknowledgedCancel {
						break
					}
				}
				if !endedCommit || !acknowledgedCancel {
					t.Fatal("cancel did not immediately end both RPCs")
				}
			} else if event := tlsSocialRequest(t, c, protocol.RoomSocialRequest{Action: "image_cancel", UploadID: cancelID}); event.Kind != "image_cancelled" {
				t.Fatalf("cancel: %+v", event)
			}
			if event := tlsSocialRequest(t, c, protocol.RoomSocialRequest{Action: "status"}); event.Kind != "status" {
				t.Fatalf("status: %+v", event)
			}
			if time.Since(started) > time.Second {
				t.Fatal("same-connection controls were delayed by image storage")
			}
			s.social.mu.Lock()
			reserved := s.social.pendingBytes
			s.social.mu.Unlock()
			if test.action == "image_commit" && reserved != len(data) {
				t.Fatal("cancel released in-flight memory too early")
			}
			if test.action == "image_get" && reserved != 0 {
				t.Fatal("pending upload was not cancelled during download")
			}

			if test.end == "disconnect" {
				_ = c.conn.(*tls.Conn).NetConn().Close()
				deadline := time.Now().Add(time.Second)
				for time.Now().Before(deadline) {
					if _, exists := s.state.User(c.sessionID); !exists {
						break
					}
					time.Sleep(time.Millisecond)
				}
				if _, exists := s.state.User(c.sessionID); exists {
					t.Fatal("disconnect waited for image storage")
				}
			} else if test.end == "leave" {
				c.send(t, protocol.TypeLeaveChannel, struct{}{})
				c.receiveUserMovedUntil(t, c.sessionID, 0)
				// Returning to the same room must not restore the old job's authority.
				c.send(t, protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 1})
				c.receiveUntil(t, protocol.TypeChatHistory)
			}
			close(gate.release)
			waitRoomImageJobs(t, s)
			if test.end != "disconnect" {
				c.send(t, protocol.TypePing, protocol.Ping{Nonce: 52})
				for range 64 {
					env := c.receive(t)
					if env.Type == protocol.TypeRoomSocialEvent {
						var event protocol.RoomSocialEvent
						_ = env.Decode(&event)
						if event.RequestID == req.RequestID {
							t.Fatalf("obsolete image job replied after %s: %+v", test.end, event)
						}
					}
					if env.Type == protocol.TypeTextMessage {
						t.Fatal("cancelled image was published")
					}
					if env.Type == protocol.TypePong {
						break
					}
				}
			}
			s.social.mu.Lock()
			reserved, uploads, jobs := s.social.pendingBytes, s.social.activeUploads, len(s.social.diskJobs)
			s.social.mu.Unlock()
			if reserved != 0 || uploads != 0 || jobs != 0 {
				t.Fatalf("resources leaked: bytes=%d uploads=%d jobs=%d", reserved, uploads, jobs)
			}
			if len(s.state.ChatHistory(1)) != 0 {
				t.Fatal("cancelled commit entered history")
			}
			if test.action == "image_commit" {
				files, err := os.ReadDir(filepath.Join(dir, "images"))
				if err != nil || len(files) != 0 {
					t.Fatalf("cancelled image files remain: %v %v", files, err)
				}
			}
		})
	}
}

func TestRoomImageBackgroundJobsAreGloballyAndPerSessionBounded(t *testing.T) {
	s, _ := socialServer(t)
	gate := &gatedRoomImages{roomImageStore: s.social.images, entered: make(chan struct{}, 2), release: make(chan struct{})}
	s.social.images = gate
	defer func() {
		select {
		case <-gate.release:
		default:
			close(gate.release)
		}
		waitRoomImageJobs(t, s)
	}()
	connect := socialTLSConnector(t, s)
	clients := []*testClient{connect(), connect(), connect()}
	data := socialJPEG(t)
	uploads := make([]string, 3)
	for i, c := range clients {
		uploads[i] = tlsImageUpload(t, c, data)
	}
	for i := 0; i < 2; i++ {
		clients[i].send(t, protocol.TypeRoomSocialRequest, protocol.RoomSocialRequest{RequestID: "commit", ChannelID: 1, Action: "image_commit", UploadID: uploads[i]})
		waitImageEntry(t, gate)
		if i == 0 {
			if event := tlsSocialRequest(t, clients[0], protocol.RoomSocialRequest{Action: "image_get", Reference: "room-image:00000000000000000000000000000000"}); event.Code != "busy" {
				t.Fatalf("session job bound with a global slot still free: %+v", event)
			}
		}
	}
	if event := tlsSocialRequest(t, clients[2], protocol.RoomSocialRequest{Action: "image_commit", UploadID: uploads[2]}); event.Code != "busy" {
		t.Fatalf("global job bound: %+v", event)
	}
	s.social.mu.Lock()
	jobs, reserved := len(s.social.diskJobs), s.social.pendingBytes
	retained := s.social.sessions[clients[2].sessionID].upload != nil
	s.social.mu.Unlock()
	if jobs != 2 || reserved != 3*len(data) || !retained {
		t.Fatal("busy commit lost upload or exceeded task bound")
	}
	for i, c := range clients {
		if event := tlsSocialRequest(t, c, protocol.RoomSocialRequest{Action: "image_cancel", UploadID: uploads[i]}); event.Kind != "image_cancelled" {
			t.Fatalf("cancel %d: %+v", i, event)
		}
	}
	close(gate.release)
	waitRoomImageJobs(t, s)
	s.social.mu.Lock()
	defer s.social.mu.Unlock()
	if s.social.pendingBytes != 0 || s.social.activeUploads != 0 {
		t.Fatal("cancelled jobs leaked reservations")
	}
}

func TestImageCommitFailureBeforeCancelHasOneTerminalReply(t *testing.T) {
	s, _ := socialServer(t)
	gate := &gatedRoomImages{roomImageStore: s.social.images, entered: make(chan struct{}, 2), release: make(chan struct{}), fail: true}
	s.social.images = gate
	defer func() {
		select {
		case <-gate.release:
		default:
			close(gate.release)
		}
		waitRoomImageJobs(t, s)
	}()
	c := socialTLSConnector(t, s)()
	upload := tlsImageUpload(t, c, socialJPEG(t))
	c.send(t, protocol.TypeRoomSocialRequest, protocol.RoomSocialRequest{RequestID: "failed-commit", ChannelID: 1, Action: "image_commit", UploadID: upload})
	waitImageEntry(t, gate)
	close(gate.release)
	waitRoomImageJobs(t, s)
	// The opposite ordering (cancel wins while the eventual Put result is an
	// error) is covered by the cancel/error TLS case above. Both orders must
	// produce exactly one terminal result for the original commit request.
	c.send(t, protocol.TypeRoomSocialRequest, protocol.RoomSocialRequest{RequestID: "cancel", ChannelID: 1, Action: "image_cancel", UploadID: upload})
	c.send(t, protocol.TypePing, protocol.Ping{Nonce: 90})
	terminal, cancelled := 0, false
	for range 64 {
		env := c.receive(t)
		if env.Type == protocol.TypeRoomSocialEvent {
			var event protocol.RoomSocialEvent
			_ = env.Decode(&event)
			if event.RequestID == "failed-commit" {
				terminal++
				if event.Kind != "error" || event.Code != "unavailable" {
					t.Fatalf("commit result: %+v", event)
				}
			}
			if event.RequestID == "cancel" {
				cancelled = event.Kind == "image_cancelled"
			}
		}
		if env.Type == protocol.TypePong {
			break
		}
	}
	if terminal != 1 || !cancelled {
		t.Fatalf("terminal replies=%d cancel=%t", terminal, cancelled)
	}
}

func TestSlowImageCommitUsesPublicationTimeAfterSameTLSConnectionText(t *testing.T) {
	s, _ := socialServer(t)
	gate := &gatedRoomImages{roomImageStore: s.social.images, entered: make(chan struct{}, 2), release: make(chan struct{})}
	s.social.images = gate
	defer func() {
		select {
		case <-gate.release:
		default:
			close(gate.release)
		}
		waitRoomImageJobs(t, s)
	}()
	c := socialTLSConnector(t, s)()
	upload := tlsImageUpload(t, c, socialJPEG(t))
	c.send(t, protocol.TypeRoomSocialRequest, protocol.RoomSocialRequest{RequestID: "slow-commit", ChannelID: 1, Action: "image_commit", UploadID: upload})
	waitImageEntry(t, gate)
	c.send(t, protocol.TypeSendTextMessage, protocol.SendTextMessageRequest{Text: "still here"})
	var text protocol.TextMessage
	if err := c.receiveUntil(t, protocol.TypeTextMessage).Decode(&text); err != nil || text.Text != "still here" {
		t.Fatal("text could not pass the pending image")
	}
	// Preserve the ordinary 250ms chat limit; image publication must use its
	// completion time rather than the timestamp from before this text message.
	time.Sleep(300 * time.Millisecond)
	close(gate.release)
	var imageMessage protocol.TextMessage
	committed := false
	for range 64 {
		env := c.receive(t)
		if env.Type == protocol.TypeTextMessage {
			_ = env.Decode(&imageMessage)
		}
		if env.Type == protocol.TypeRoomSocialEvent {
			var event protocol.RoomSocialEvent
			_ = env.Decode(&event)
			if event.RequestID == "slow-commit" {
				if event.Kind != "image_committed" {
					t.Fatalf("late commit: %+v", event)
				}
				committed = true
				break
			}
		}
	}
	waitRoomImageJobs(t, s)
	if !committed || imageMessage.Image == nil || imageMessage.SentAtUnixMS < text.SentAtUnixMS+250 || len(s.state.ChatHistory(1)) != 2 {
		t.Fatal("image did not publish with current chat time after text")
	}
}
