package control

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/roomimage"
	"edenvoice/server/internal/serverstate"
)

const maxPendingImageBytes = 1024 * 1024
const maxPendingImageUploads = 8
const imageUploadTTL = time.Minute

type socialBucket struct {
	tokens float64
	at     time.Time
}

func (b *socialBucket) allow(cost, rate, burst float64, now time.Time) bool {
	if b.at.IsZero() {
		b.tokens = burst
	} else {
		b.tokens = min(burst, b.tokens+max(0, now.Sub(b.at).Seconds())*rate)
	}
	b.at = now
	if b.tokens < cost {
		return false
	}
	b.tokens -= cost
	return true
}

type imageUpload struct {
	id                    string
	commitRequestID       string
	channel, generation   uint64
	width, height, size   int
	data                  []byte
	expires               time.Time
	cancelled, publishing bool // guarded by roomSocial.mu
}
type socialSession struct {
	committing                  *imageUpload
	reading                     bool
	requests, inbound, outbound socialBucket
	upload                      *imageUpload
}
type roomImageStore interface {
	Put(uint64, []byte, int, int, time.Time) (protocol.ChatImage, error)
	Get(uint64, string, int, time.Time) (protocol.ChatImage, []byte, bool, error)
	Remove(string)
	Cleanup(time.Time)
}

type roomSocial struct {
	diskJobs          chan struct{}
	activeUploads     int
	mu                sync.Mutex
	sessions          map[uint64]*socialSession
	rooms             map[uint64]*socialBucket
	inbound, outbound socialBucket
	pendingBytes      int
	images            roomImageStore
}

func newRoomSocial() *roomSocial {
	return &roomSocial{diskJobs: make(chan struct{}, 2), sessions: map[uint64]*socialSession{}, rooms: map[uint64]*socialBucket{}}
}

// ConfigureRoomImages runs before accepting connections. Failure leaves voice,
// text and ephemeral social features available without advertising images.
func (s *Server) ConfigureRoomImages(dir string) error {
	store, err := roomimage.Open(dir)
	if err != nil {
		return err
	}
	s.social.images = store
	return nil
}
func (s *Server) socialError(sender serverstate.Sender, req protocol.RoomSocialRequest, code, message string, retry int64) {
	s.send(sender, protocol.TypeRoomSocialEvent, protocol.RoomSocialEvent{RequestID: req.RequestID, ChannelID: req.ChannelID, Kind: "error", Code: code, Message: message, RetryAfterMS: retry})
}
func (s *Server) dispatchRoomSocial(id uint64, envelope protocol.Envelope, sender serverstate.Sender) error {
	var req protocol.RoomSocialRequest
	if err := envelope.Decode(&req); err != nil {
		s.socialError(sender, req, "invalid_request", "invalid room social request", 0)
		return nil
	}
	if len(req.RequestID) == 0 || len(req.RequestID) > 64 || strings.IndexFunc(req.RequestID, func(r rune) bool { return r < 33 || r > 126 }) >= 0 {
		req.RequestID = ""
		s.socialError(sender, req, "invalid_request", "invalid request ID", 0)
		return nil
	}
	if len(req.UploadID) > 32 || len(req.Reference) > 43 {
		s.socialError(sender, req, "invalid_request", "invalid image identifier", 0)
		return nil
	}
	generation, allowed := s.state.RoomSocialMember(id, req.ChannelID)
	if !allowed {
		s.socialError(sender, req, "forbidden", serverstate.ErrRoomSocialAccess.Error(), 0)
		return nil
	}
	s.state.EnableRoomSocial(id)
	now := time.Now()
	social := s.social
	social.mu.Lock()
	locked := true
	defer func() {
		if locked {
			social.mu.Unlock()
		}
	}()
	unlock := func() { social.mu.Unlock(); locked = false }
	session := social.sessions[id]
	if session == nil {
		session = &socialSession{}
		social.sessions[id] = session
	}
	if session.upload != nil && (session.upload.channel != req.ChannelID || session.upload.generation != generation || !session.upload.expires.After(now)) {
		social.dropUpload(session)
	}
	if !session.requests.allow(1, 20, 20, now) {
		s.socialError(sender, req, "busy", "room controls are busy; retry shortly", 250)
		return nil
	}
	event := protocol.RoomSocialEvent{RequestID: req.RequestID, ChannelID: req.ChannelID}
	fail := func(code string, err error) { s.socialError(sender, req, code, err.Error(), 0) }
	busy := func() { s.socialError(sender, req, "busy", "image transfer is busy; retry this request", 400) }
	switch req.Action {
	case "status":
		away, err := s.state.RoomSocialStatus(id, req.ChannelID)
		if err != nil {
			fail("forbidden", err)
			return nil
		}
		event.Kind = "status"
		event.Away = away
	case "away", "reaction":
		if req.Action == "reaction" {
			bucket := social.rooms[req.ChannelID]
			if bucket == nil {
				bucket = &socialBucket{}
				social.rooms[req.ChannelID] = bucket
			}
			if !bucket.allow(1, 4, 6, now) {
				s.socialError(sender, req, "busy", "room reactions are busy", 500)
				return nil
			}
		}
		if err := s.state.ApplyRoomSocial(id, req, now); err != nil {
			if errors.Is(err, serverstate.ErrReactionCooldown) {
				s.socialError(sender, req, "busy", err.Error(), 2000)
			} else {
				fail("invalid_request", err)
			}
		}
		return nil
	case "image_begin":
		if social.images == nil {
			fail("unavailable", roomimage.ErrUnavailable)
			return nil
		}
		if !protocol.ValidRoomImageDimensions(req.Width, req.Height, req.Bytes) {
			fail("invalid_image", errors.New("image must be JPEG, at most 256 KiB and 1600 pixels per edge"))
			return nil
		}
		if session.upload != nil || session.committing != nil {
			fail("upload_exists", errors.New("cancel the current upload first"))
			return nil
		}
		if social.pendingBytes+req.Bytes > maxPendingImageBytes || social.activeUploads >= maxPendingImageUploads {
			busy()
			return nil
		}
		raw := make([]byte, 16)
		if _, err := rand.Read(raw); err != nil {
			fail("unavailable", roomimage.ErrUnavailable)
			return nil
		}
		upload := &imageUpload{id: hex.EncodeToString(raw), channel: req.ChannelID, generation: generation, width: req.Width, height: req.Height, size: req.Bytes, data: make([]byte, 0, req.Bytes), expires: now.Add(imageUploadTTL)}
		session.upload = upload
		social.pendingBytes += req.Bytes
		social.activeUploads++
		event.Kind = "image_upload"
		event.UploadID = upload.id
	case "image_chunk":
		upload := session.upload
		if upload == nil || upload.id != req.UploadID {
			fail("upload_missing", errors.New("image upload expired or was cancelled"))
			return nil
		}
		if len(req.Data) > base64.StdEncoding.EncodedLen(protocol.MaxRoomImageChunkBytes) {
			fail("invalid_chunk", errors.New("image chunk exceeds 8192 bytes"))
			return nil
		}
		data, err := base64.StdEncoding.Strict().DecodeString(req.Data)
		if err != nil || len(data) == 0 || len(data) > protocol.MaxRoomImageChunkBytes || req.Offset != len(upload.data) || len(upload.data)+len(data) > upload.size {
			fail("invalid_chunk", errors.New("invalid image chunk or offset"))
			return nil
		}
		if !session.inbound.allow(float64(len(req.Data)), 32*1024, 16*1024, now) || !social.inbound.allow(float64(len(req.Data)), 64*1024, 16*1024, now) {
			busy()
			return nil
		}
		upload.data = append(upload.data, data...)
		event.Kind = "image_upload"
		event.UploadID = upload.id
		event.Offset = len(upload.data)
	case "image_commit":
		upload := session.upload
		if upload == nil || upload.id != req.UploadID {
			fail("upload_missing", errors.New("image upload expired or was cancelled"))
			return nil
		}
		if len(upload.data) != upload.size {
			fail("incomplete_image", errors.New("image upload is incomplete"))
			return nil
		}
		if session.reading || session.committing != nil {
			busy()
			return nil
		}
		select {
		case social.diskJobs <- struct{}{}:
		default:
			busy()
			return nil
		}
		// Keep the reservation until persistence finishes, even if a move or
		// disconnect removes this session. No disk work holds the social lock.
		session.upload = nil
		session.committing = upload
		upload.commitRequestID = req.RequestID
		images := social.images
		unlock()
		go s.commitRoomImage(id, req, session, upload, images, now)
		return nil
	case "image_cancel":
		if session.upload != nil && session.upload.id == req.UploadID {
			social.dropUpload(session)
		}
		if upload := session.committing; upload != nil && upload.id == req.UploadID {
			// Once admitted to the chat log the message cannot be withdrawn.
			// Never acknowledge cancellation of a publication already in progress.
			if upload.publishing {
				busy()
				return nil
			}
			if !upload.cancelled {
				upload.cancelled = true
				// Finish the original RPC immediately, while its reserved memory
				// and disk slot remain held until the background task returns.
				s.sendRoomImageResult(id, upload.generation, protocol.RoomSocialRequest{RequestID: upload.commitRequestID, ChannelID: upload.channel}, protocol.RoomSocialEvent{Kind: "error", Code: "upload_missing", Message: "image upload was cancelled"})
			}
		}
		event.Kind = "image_cancelled"
		event.UploadID = req.UploadID
	case "image_get":
		if social.images == nil {
			fail("unavailable", roomimage.ErrUnavailable)
			return nil
		}
		if session.reading || session.committing != nil {
			busy()
			return nil
		}
		select {
		case social.diskJobs <- struct{}{}:
		default:
			busy()
			return nil
		}
		// Budget counts the encoded payload plus bounded envelope overhead.
		const cost = 12 * 1024
		if !session.outbound.allow(cost, 32*1024, 16*1024, now) || !social.outbound.allow(cost, 64*1024, 16*1024, now) {
			<-social.diskJobs
			busy()
			return nil
		}
		session.reading = true
		images := social.images
		unlock()
		go s.readRoomImage(id, generation, req, session, images, now)
		return nil
	default:
		fail("invalid_request", errors.New("unsupported room social action"))
		return nil
	}
	response, _ := protocol.NewEnvelope(protocol.TypeRoomSocialEvent, event)
	s.state.SendRoomSocial(id, req.ChannelID, generation, response)
	return nil
}

// Jobs acquire their bounded slot before spawning, so a slow filesystem cannot
// grow a goroutine queue or stop the same connection's control read loop.
func (s *Server) commitRoomImage(id uint64, req protocol.RoomSocialRequest, session *socialSession, upload *imageUpload, images roomImageStore, now time.Time) {
	social := s.social
	var result *protocol.RoomSocialEvent
	defer func() {
		social.mu.Lock()
		defer social.mu.Unlock()
		social.pendingBytes -= upload.size
		social.activeUploads--
		session.committing = nil
		<-social.diskJobs
		// Serialize completion with cancel. The original RPC has already been
		// ended by a successful cancel, so it must not get a second result.
		if result != nil && !upload.cancelled {
			s.sendRoomImageResult(id, upload.generation, req, *result)
		}
	}()
	image, err := images.Put(upload.channel, upload.data, upload.width, upload.height, now)
	social.mu.Lock()
	cancelled := upload.cancelled
	if !cancelled && err == nil {
		upload.publishing = true
	}
	social.mu.Unlock()
	if cancelled {
		if err == nil {
			images.Remove(image.Reference)
		}
		return
	}
	if err != nil {
		code, publicErr := "unavailable", roomimage.ErrUnavailable
		switch {
		case errors.Is(err, roomimage.ErrInvalidImage):
			code, publicErr = "invalid_image", roomimage.ErrInvalidImage
		case errors.Is(err, roomimage.ErrFull):
			code, publicErr = "storage_full", roomimage.ErrFull
		}
		result = &protocol.RoomSocialEvent{Kind: "error", Code: code, Message: publicErr.Error()}
		return
	}
	// Revalidate membership after persistence, before accepting the message into
	// the durable chat log. Moving or disconnecting never waits for this I/O.
	message, err := s.state.RecordImageMessage(id, upload.channel, upload.generation, image, time.Now())
	if err != nil {
		images.Remove(image.Reference)
		result = &protocol.RoomSocialEvent{Kind: "error", Code: "message_failed", Message: "image message could not be saved"}
		return
	}
	envelope, _ := protocol.NewEnvelope(protocol.TypeTextMessage, message)
	s.state.BroadcastRoom(message.ChannelID, envelope, 0)
	s.broadcastChatHeads(message)
	result = &protocol.RoomSocialEvent{Kind: "image_committed", UploadID: req.UploadID, Image: &image}
}

func (s *Server) readRoomImage(id, generation uint64, req protocol.RoomSocialRequest, session *socialSession, images roomImageStore, now time.Time) {
	var result protocol.RoomSocialEvent
	defer func() {
		s.social.mu.Lock()
		defer s.social.mu.Unlock()
		session.reading = false
		<-s.social.diskJobs
		s.sendRoomImageResult(id, generation, req, result)
	}()
	image, data, eof, err := images.Get(req.ChannelID, req.Reference, req.Offset, now)
	if err != nil {
		code := "unavailable"
		if errors.Is(err, roomimage.ErrUnavailable) {
			code = "image_missing"
		}
		result = protocol.RoomSocialEvent{Kind: "error", Code: code, Message: roomimage.ErrUnavailable.Error()}
		return
	}
	result = protocol.RoomSocialEvent{Kind: "image_data", Reference: req.Reference, Offset: req.Offset, Data: base64.StdEncoding.EncodeToString(data), EOF: eof, Image: &image}
}

func (s *Server) sendRoomImageResult(id, generation uint64, req protocol.RoomSocialRequest, event protocol.RoomSocialEvent) {
	event.RequestID, event.ChannelID = req.RequestID, req.ChannelID
	response, _ := protocol.NewEnvelope(protocol.TypeRoomSocialEvent, event)
	s.state.SendRoomSocial(id, req.ChannelID, generation, response)
}
func (r *roomSocial) dropUpload(s *socialSession) {
	if s.upload != nil {
		r.pendingBytes -= s.upload.size
		r.activeUploads--
		s.upload = nil
	}
}
func (s *Server) cleanupRoomSocial(now time.Time) {
	social := s.social
	social.mu.Lock()
	for id, session := range social.sessions {
		_, exists := s.state.User(id)
		if !exists {
			social.dropUpload(session)
			if session.committing != nil {
				session.committing.cancelled = true
			}
			delete(social.sessions, id)
			continue
		}
		if session.upload != nil {
			generation, allowed := s.state.RoomSocialMember(id, session.upload.channel)
			if !allowed || generation != session.upload.generation || !session.upload.expires.After(now) {
				social.dropUpload(session)
			}
		}
	}
	for channel, bucket := range social.rooms {
		if now.Sub(bucket.at) > time.Minute {
			delete(social.rooms, channel)
		}
	}
	images := social.images
	social.mu.Unlock()
	if images != nil {
		images.Cleanup(now)
	}
}
func (s *Server) cancelRoomSocialOnMove(id uint64) {
	social := s.social
	social.mu.Lock()
	defer social.mu.Unlock()
	if session := social.sessions[id]; session != nil && session.upload != nil {
		generation, allowed := s.state.RoomSocialMember(id, session.upload.channel)
		if !allowed || generation != session.upload.generation {
			social.dropUpload(session)
		}
	}
	if session := social.sessions[id]; session != nil && session.committing != nil {
		upload := session.committing
		generation, allowed := s.state.RoomSocialMember(id, upload.channel)
		if !allowed || generation != upload.generation {
			upload.cancelled = true
		}
	}
}
func (s *Server) removeRoomSocial(id uint64) {
	social := s.social
	social.mu.Lock()
	defer social.mu.Unlock()
	if session := social.sessions[id]; session != nil {
		social.dropUpload(session)
		if session.committing != nil {
			session.committing.cancelled = true
		}
		delete(social.sessions, id)
	}
}
