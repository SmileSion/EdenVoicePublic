// Package roomimage stores bounded JPEG attachments separately from chat logs.
package roomimage

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"image/jpeg"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/privatefs"
	"edenvoice/server/internal/protocol"
)

const MaxDiskBytes int64 = 64 * 1024 * 1024
const Retention = 30 * 24 * time.Hour
const maxFiles = 4096
const refPrefix = "room-image:"

var ErrUnavailable = errors.New("image unavailable or expired")
var ErrInvalidImage = errors.New("invalid JPEG image")
var ErrFull = errors.New("image storage is full; retry later")

type entry struct {
	Image     protocol.ChatImage `json:"image"`
	Channel   uint64             `json:"channel"`
	CreatedAt time.Time          `json:"created_at"`
	diskBytes int64
}
type Store struct {
	mu          sync.Mutex
	dir         string
	entries     map[string]entry
	bytes       int64
	lastCleanup time.Time
}

func ValidReference(ref string) bool {
	raw, ok := strings.CutPrefix(ref, refPrefix)
	if !ok || len(raw) != 32 {
		return false
	}
	decoded, err := hex.DecodeString(raw)
	return err == nil && len(decoded) == 16 && strings.ToLower(raw) == raw
}
func (s *Store) path(ref, ext string) string {
	return filepath.Join(s.dir, strings.TrimPrefix(ref, refPrefix)+ext)
}
func Open(dir string) (*Store, error) {
	if err := privatefs.PrepareDirectory(filepath.Join(dir, "image.json")); err != nil {
		return nil, err
	}
	files, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	s := &Store{dir: dir, entries: map[string]entry{}}
	for _, f := range files {
		if !strings.HasSuffix(f.Name(), ".json") {
			continue
		}
		ref := refPrefix + strings.TrimSuffix(f.Name(), ".json")
		if !ValidReference(ref) {
			continue
		}
		info, err := f.Info()
		if err != nil {
			return nil, err
		}
		if !info.Mode().IsRegular() || info.Size() > 1024 {
			return nil, errors.New("invalid image metadata file")
		}
		b, err := os.ReadFile(s.path(ref, ".json"))
		if err != nil {
			return nil, err
		}
		var e entry
		if json.Unmarshal(b, &e) != nil || e.Image.Reference != ref || e.Image.MIME != "image/jpeg" || e.Channel == 0 || e.CreatedAt.IsZero() || !protocol.ValidRoomImageDimensions(e.Image.Width, e.Image.Height, e.Image.Bytes) {
			return nil, errors.New("invalid image metadata")
		}
		jpg, err := os.Lstat(s.path(ref, ".jpg"))
		if errors.Is(err, os.ErrNotExist) {
			_ = os.Remove(s.path(ref, ".json"))
			continue
		}
		if err != nil {
			return nil, err
		}
		if !jpg.Mode().IsRegular() || jpg.Size() != int64(e.Image.Bytes) {
			return nil, errors.New("invalid stored image")
		}
		if err = privatefs.Ensure(s.path(ref, ".jpg")); err != nil {
			return nil, err
		}
		if err = privatefs.Ensure(s.path(ref, ".json")); err != nil {
			return nil, err
		}
		e.diskBytes = info.Size() + jpg.Size()
		s.entries[ref] = e
		s.bytes += e.diskBytes
	}
	// Crash leftovers are never referenced by a published metadata entry.
	for _, f := range files {
		ref := refPrefix + strings.TrimSuffix(f.Name(), ".jpg")
		if strings.HasPrefix(f.Name(), ".image-") || (strings.HasSuffix(f.Name(), ".jpg") && ValidReference(ref) && s.entries[ref].Channel == 0) {
			if err := os.Remove(filepath.Join(dir, f.Name())); err != nil {
				return nil, err
			}
		}
	}
	s.cleanupLocked(time.Now())
	if s.bytes > MaxDiskBytes || len(s.entries) > maxFiles {
		return nil, ErrFull
	}
	return s, nil
}
func Validate(data []byte, width, height int) error {
	if !protocol.ValidRoomImageDimensions(width, height, len(data)) {
		return fmt.Errorf("%w: dimensions or size exceed limits", ErrInvalidImage)
	}
	c, err := jpeg.DecodeConfig(bytes.NewReader(data))
	if err != nil || c.Width != width || c.Height != height {
		return fmt.Errorf("%w: dimensions do not match", ErrInvalidImage)
	}
	img, err := jpeg.Decode(bytes.NewReader(data))
	if err != nil || img.Bounds().Dx() != width || img.Bounds().Dy() != height {
		return fmt.Errorf("%w: pixel data cannot be decoded", ErrInvalidImage)
	}
	return nil
}
func (s *Store) Put(channel uint64, data []byte, width, height int, now time.Time) (protocol.ChatImage, error) {
	if channel == 0 {
		return protocol.ChatImage{}, ErrUnavailable
	}
	if err := Validate(data, width, height); err != nil {
		return protocol.ChatImage{}, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.cleanupLocked(now)
	raw := make([]byte, 16)
	if _, err := rand.Read(raw); err != nil {
		return protocol.ChatImage{}, err
	}
	ref := refPrefix + hex.EncodeToString(raw)
	e := entry{Image: protocol.ChatImage{Reference: ref, MIME: "image/jpeg", Width: width, Height: height, Bytes: len(data)}, Channel: channel, CreatedAt: now}
	meta, err := json.Marshal(e)
	if err != nil {
		return protocol.ChatImage{}, err
	}
	e.diskBytes = int64(len(data) + len(meta))
	if s.bytes+e.diskBytes > MaxDiskBytes || len(s.entries) >= maxFiles {
		return protocol.ChatImage{}, ErrFull
	}
	if err = s.writeAtomic(s.path(ref, ".jpg"), data); err != nil {
		return protocol.ChatImage{}, err
	}
	if err = s.writeAtomic(s.path(ref, ".json"), meta); err != nil {
		_ = os.Remove(s.path(ref, ".jpg"))
		return protocol.ChatImage{}, err
	}
	dir, err := os.Open(s.dir)
	if err == nil {
		err = dir.Sync()
		_ = dir.Close()
	}
	if err != nil {
		_ = os.Remove(s.path(ref, ".jpg"))
		_ = os.Remove(s.path(ref, ".json"))
		return protocol.ChatImage{}, err
	}
	s.entries[ref] = e
	s.bytes += e.diskBytes
	return e.Image, nil
}
func (s *Store) writeAtomic(path string, data []byte) error {
	f, err := os.CreateTemp(s.dir, ".image-")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	_, err = f.Write(data)
	if err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(f.Name(), path)
	}
	return err
}
func (s *Store) Get(channel uint64, ref string, offset int, now time.Time) (protocol.ChatImage, []byte, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.cleanupLocked(now)
	e, ok := s.entries[ref]
	if !ok || !e.CreatedAt.Add(Retention).After(now) || e.Channel != channel || !ValidReference(ref) || offset < 0 || offset >= e.Image.Bytes {
		return protocol.ChatImage{}, nil, false, ErrUnavailable
	}
	f, err := os.Open(s.path(ref, ".jpg"))
	if err != nil {
		return protocol.ChatImage{}, nil, false, ErrUnavailable
	}
	defer f.Close()
	count := min(protocol.MaxRoomImageChunkBytes, e.Image.Bytes-offset)
	data := make([]byte, count)
	if _, err = f.ReadAt(data, int64(offset)); err != nil {
		return protocol.ChatImage{}, nil, false, err
	}
	return e.Image, data, offset+count == e.Image.Bytes, nil
}
func (s *Store) Remove(ref string) { s.mu.Lock(); defer s.mu.Unlock(); s.removeLocked(ref) }
func (s *Store) removeLocked(ref string) {
	e, ok := s.entries[ref]
	if !ok {
		return
	}
	if err := os.Remove(s.path(ref, ".jpg")); err != nil && !errors.Is(err, os.ErrNotExist) {
		return
	}
	if err := os.Remove(s.path(ref, ".json")); err != nil && !errors.Is(err, os.ErrNotExist) {
		return
	}
	delete(s.entries, ref)
	s.bytes -= e.diskBytes
}
func (s *Store) cleanupLocked(now time.Time) {
	if !s.lastCleanup.IsZero() && now.Sub(s.lastCleanup) < time.Minute {
		return
	}
	s.lastCleanup = now
	for ref, e := range s.entries {
		if !e.CreatedAt.Add(Retention).After(now) {
			s.removeLocked(ref)
		}
	}
}
func (s *Store) Cleanup(now time.Time) { s.mu.Lock(); defer s.mu.Unlock(); s.cleanupLocked(now) }
