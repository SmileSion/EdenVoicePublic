package security

import (
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"sync"

	"edenvoice/server/internal/protocol"

	"golang.org/x/crypto/chacha20poly1305"
	"golang.org/x/crypto/curve25519"
	"golang.org/x/crypto/hkdf"
)

const (
	ProtocolVersion  = 5
	InitialKeyEpoch  = uint16(1)
	ReplayWindowSize = 64
)

var transcriptDomain = []byte("EdenVoice/v0.5/identity-auth\x00")

func NewChallenge() ([]byte, error) {
	challenge := make([]byte, 32)
	if _, err := cryptorand.Read(challenge); err != nil {
		return nil, fmt.Errorf("generate challenge: %w", err)
	}
	return challenge, nil
}

type EphemeralKey struct {
	private   [curve25519.ScalarSize]byte
	public    [curve25519.PointSize]byte
	destroyed bool
}

func NewEphemeralKey() (*EphemeralKey, error) {
	key := &EphemeralKey{}
	if _, err := cryptorand.Read(key.private[:]); err != nil {
		return nil, err
	}
	public, err := curve25519.X25519(key.private[:], curve25519.Basepoint)
	if err != nil {
		key.Destroy()
		return nil, err
	}
	copy(key.public[:], public)
	clear(public)
	return key, nil
}

func (k *EphemeralKey) PublicKey() []byte {
	if k == nil || k.destroyed {
		return nil
	}
	return append([]byte(nil), k.public[:]...)
}

func (k *EphemeralKey) Destroy() {
	if k == nil {
		return
	}
	clear(k.private[:])
	clear(k.public[:])
	k.destroyed = true
}

func AuthTranscript(challenge, serverIdentity, clientEphemeral, serverEphemeral []byte) []byte {
	hash := sha256.New()
	hash.Write(transcriptDomain)
	var version [2]byte
	binary.BigEndian.PutUint16(version[:], ProtocolVersion)
	hash.Write(version[:])
	writeTranscriptField(hash, challenge)
	writeTranscriptField(hash, serverIdentity)
	writeTranscriptField(hash, clientEphemeral)
	writeTranscriptField(hash, serverEphemeral)
	return hash.Sum(nil)
}

func VerifyClient(publicKey ed25519.PublicKey, claimedClientID string, transcript, signature []byte) error {
	if len(publicKey) != ed25519.PublicKeySize || subtle.ConstantTimeCompare([]byte(ClientID(publicKey)), []byte(claimedClientID)) != 1 {
		return errors.New("client identity does not match claimed client ID")
	}
	if !ed25519.Verify(publicKey, transcript, signature) {
		return errors.New("invalid client identity signature")
	}
	return nil
}

func writeTranscriptField(writer io.Writer, value []byte) {
	var length [4]byte
	binary.BigEndian.PutUint32(length[:], uint32(len(value)))
	_, _ = writer.Write(length[:])
	_, _ = writer.Write(value)
}

type SessionSecrets struct {
	ClientToServerKey [chacha20poly1305.KeySize]byte
	ServerToClientKey [chacha20poly1305.KeySize]byte
	ClientNoncePrefix [6]byte
	ServerNoncePrefix [6]byte
}

func (s *SessionSecrets) Destroy() {
	clear(s.ClientToServerKey[:])
	clear(s.ServerToClientKey[:])
	clear(s.ClientNoncePrefix[:])
	clear(s.ServerNoncePrefix[:])
}

func DeriveSessionSecrets(privateKey *EphemeralKey, peerPublicKey, transcript []byte) (SessionSecrets, error) {
	if privateKey == nil || privateKey.destroyed {
		return SessionSecrets{}, errors.New("ephemeral key was destroyed")
	}
	if len(peerPublicKey) != curve25519.PointSize {
		return SessionSecrets{}, errors.New("invalid peer ephemeral key")
	}
	sharedSecret, err := curve25519.X25519(privateKey.private[:], peerPublicKey)
	if err != nil {
		return SessionSecrets{}, fmt.Errorf("x25519 agreement: %w", err)
	}
	defer clear(sharedSecret)
	reader := hkdf.New(sha256.New, sharedSecret, transcript, []byte("EdenVoice/v0.5/session-secrets"))
	var secrets SessionSecrets
	material := make([]byte, 76)
	defer clear(material)
	if _, err := io.ReadFull(reader, material); err != nil {
		return SessionSecrets{}, fmt.Errorf("derive session secrets: %w", err)
	}
	copy(secrets.ClientToServerKey[:], material[:32])
	copy(secrets.ServerToClientKey[:], material[32:64])
	copy(secrets.ClientNoncePrefix[:], material[64:70])
	copy(secrets.ServerNoncePrefix[:], material[70:76])
	return secrets, nil
}

type replayWindow struct {
	initialized bool
	highest     uint32
	bitmap      uint64
}

func (w *replayWindow) seen(sequence uint32) bool {
	if !w.initialized || sequence > w.highest {
		return false
	}
	delta := w.highest - sequence
	return delta >= ReplayWindowSize || w.bitmap&(uint64(1)<<delta) != 0
}

func (w *replayWindow) mark(sequence uint32) {
	if !w.initialized {
		w.initialized, w.highest, w.bitmap = true, sequence, 1
		return
	}
	if sequence > w.highest {
		delta := sequence - w.highest
		if delta >= ReplayWindowSize {
			w.bitmap = 1
		} else {
			w.bitmap = w.bitmap<<delta | 1
		}
		w.highest = sequence
		return
	}
	w.bitmap |= uint64(1) << (w.highest - sequence)
}

type epochCipher struct {
	epoch       uint16
	rootKey     [32]byte
	streams     map[uint64]cipherAEAD
	noncePrefix [6]byte
	replay      map[uint64]*replayWindow
}

type cipherAEAD interface {
	Seal(dst, nonce, plaintext, additionalData []byte) []byte
	Open(dst, nonce, ciphertext, additionalData []byte) ([]byte, error)
}

// MediaCipher protects one direction of one authenticated media session.
// It accepts the current and immediately previous epoch only.
type MediaCipher struct {
	mu        sync.Mutex
	current   epochCipher
	previous  *epochCipher
	pending   *epochCipher
	destroyed bool
}

func (c *MediaCipher) Epoch() uint16 {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return 0
	}
	return c.current.epoch
}

func NewMediaCipher(key [32]byte, noncePrefix [6]byte, epoch uint16) (*MediaCipher, error) {
	cipher := &MediaCipher{current: epochCipher{epoch: epoch, rootKey: key, streams: make(map[uint64]cipherAEAD), noncePrefix: noncePrefix, replay: make(map[uint64]*replayWindow)}}
	clear(key[:])
	clear(noncePrefix[:])
	return cipher, nil
}

// SealPacket chooses the epoch, authenticates the corresponding wire header,
// and encrypts under one lock. A concurrent rekey cannot split these steps.
// The caller retains ownership of plaintext; packet is returned by value.
func (c *MediaCipher) SealPacket(packet protocol.VoicePacket, plaintext []byte) (protocol.VoicePacket, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	packet.KeyEpoch = c.current.epoch
	header, err := packet.AuthenticatedHeader(len(plaintext) + chacha20poly1305.Overhead)
	if err != nil {
		return protocol.VoicePacket{}, err
	}
	packet.Payload, _, err = c.sealLocked(header, packet.SessionID, packet.Sequence, plaintext)
	if err != nil {
		return protocol.VoicePacket{}, err
	}
	return packet, nil
}

// Seal is the low-level API for epoch-independent associated data. Media wire
// packets must use SealPacket so their authenticated epoch cannot go stale.
func (c *MediaCipher) Seal(aad []byte, streamID uint64, sequence uint32, plaintext []byte) ([]byte, uint16, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.sealLocked(aad, streamID, sequence, plaintext)
}

func (c *MediaCipher) sealLocked(aad []byte, streamID uint64, sequence uint32, plaintext []byte) ([]byte, uint16, error) {
	if c.destroyed {
		return nil, 0, errors.New("media cipher destroyed")
	}
	nonce := mediaNonce(c.current.noncePrefix, c.current.epoch, sequence)
	aead, err := c.current.streamCipher(streamID)
	if err != nil {
		return nil, 0, err
	}
	return aead.Seal(nil, nonce[:], plaintext, aad), c.current.epoch, nil
}

func (c *MediaCipher) Open(aad []byte, streamID uint64, epoch uint16, sequence uint32, ciphertext []byte) ([]byte, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return nil, errors.New("media cipher destroyed")
	}
	selected := &c.current
	if epoch != selected.epoch {
		if c.previous != nil && c.previous.epoch == epoch {
			selected = c.previous
		} else if c.pending != nil && c.pending.epoch == epoch {
			selected = c.pending
		} else {
			return nil, errors.New("invalid media key epoch")
		}
	}
	window := selected.replay[streamID]
	if window == nil {
		window = &replayWindow{}
		selected.replay[streamID] = window
	}
	if window.seen(sequence) {
		return nil, errors.New("replayed or stale media packet")
	}
	nonce := mediaNonce(selected.noncePrefix, epoch, sequence)
	aead, err := selected.streamCipher(streamID)
	if err != nil {
		return nil, err
	}
	plaintext, err := aead.Open(nil, nonce[:], ciphertext, aad)
	if err != nil {
		return nil, errors.New("media authentication failed")
	}
	window.mark(sequence)
	return plaintext, nil
}

func (c *MediaCipher) Rekey(key [32]byte, noncePrefix [6]byte, epoch uint16) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return errors.New("media cipher destroyed")
	}
	if epoch == 0 || epoch == c.current.epoch {
		return errors.New("new media epoch must be non-zero and distinct")
	}
	if c.previous != nil {
		destroyEpoch(c.previous)
	}
	previous := c.current
	c.previous = &previous
	c.current = epochCipher{epoch: epoch, rootKey: key, streams: make(map[uint64]cipherAEAD), noncePrefix: noncePrefix, replay: make(map[uint64]*replayWindow)}
	clear(key[:])
	clear(noncePrefix[:])
	return nil
}

func (e *epochCipher) streamCipher(streamID uint64) (cipherAEAD, error) {
	if aead := e.streams[streamID]; aead != nil {
		return aead, nil
	}
	var streamContext [8]byte
	binary.BigEndian.PutUint64(streamContext[:], streamID)
	reader := hkdf.New(sha256.New, e.rootKey[:], nil, append([]byte("EdenVoice/v0.5/media-stream\x00"), streamContext[:]...))
	var key [32]byte
	if _, err := io.ReadFull(reader, key[:]); err != nil {
		return nil, err
	}
	aead, err := chacha20poly1305.New(key[:])
	clear(key[:])
	if err == nil {
		e.streams[streamID] = aead
	}
	return aead, err
}

func (c *MediaCipher) DropPrevious() {
	c.mu.Lock()
	if c.previous != nil {
		destroyEpoch(c.previous)
	}
	c.previous = nil
	c.mu.Unlock()
}

func (c *MediaCipher) DropPreviousIfCurrent(currentEpoch uint16) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed || c.current.epoch != currentEpoch || c.previous == nil {
		return
	}
	destroyEpoch(c.previous)
	c.previous = nil
}

func (c *MediaCipher) PrepareRekey(salt []byte, epoch uint16) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return errors.New("media cipher destroyed")
	}
	if epoch == 0 || epoch == c.current.epoch {
		return errors.New("invalid pending media epoch")
	}
	reader := hkdf.New(sha256.New, c.current.rootKey[:], salt, []byte("EdenVoice/v0.5/media-rekey"))
	var material [38]byte
	if _, err := io.ReadFull(reader, material[:]); err != nil {
		return err
	}
	var key [32]byte
	var prefix [6]byte
	copy(key[:], material[:32])
	copy(prefix[:], material[32:])
	clear(material[:])
	if c.pending != nil {
		destroyEpoch(c.pending)
	}
	pending := epochCipher{epoch: epoch, rootKey: key, streams: make(map[uint64]cipherAEAD), noncePrefix: prefix, replay: make(map[uint64]*replayWindow)}
	clear(key[:])
	clear(prefix[:])
	c.pending = &pending
	return nil
}

func (c *MediaCipher) ActivatePrepared(epoch uint16) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return errors.New("media cipher destroyed")
	}
	if c.pending == nil || c.pending.epoch != epoch {
		return errors.New("media rekey was not prepared")
	}
	if c.previous != nil {
		destroyEpoch(c.previous)
	}
	previous := c.current
	c.previous = &previous
	pending := c.pending
	c.current = *pending
	c.pending = nil
	clear(pending.rootKey[:])
	clear(pending.noncePrefix[:])
	pending.epoch = 0
	pending.streams = nil
	pending.replay = nil
	return nil
}

func (c *MediaCipher) Destroy() {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.destroyed {
		return
	}
	destroyEpoch(&c.current)
	if c.previous != nil {
		destroyEpoch(c.previous)
		c.previous = nil
	}
	if c.pending != nil {
		destroyEpoch(c.pending)
		c.pending = nil
	}
	c.destroyed = true
}

func destroyEpoch(epoch *epochCipher) {
	clear(epoch.rootKey[:])
	clear(epoch.noncePrefix[:])
	clear(epoch.streams)
	clear(epoch.replay)
	epoch.epoch = 0
}

func mediaNonce(prefix [6]byte, epoch uint16, sequence uint32) [chacha20poly1305.NonceSize]byte {
	var nonce [chacha20poly1305.NonceSize]byte
	copy(nonce[:6], prefix[:])
	binary.BigEndian.PutUint16(nonce[6:8], epoch)
	binary.BigEndian.PutUint32(nonce[8:12], sequence)
	return nonce
}
