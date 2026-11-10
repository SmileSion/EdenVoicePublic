// Package chatstore retains all room messages in append-only JSONL files.
// Recent reads seek backwards: startup never decodes the entire chat archive.
package chatstore

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"

	"edenvoice/server/internal/privatefs"
	"edenvoice/server/internal/protocol"
)

const RecentLimit = 100
const maxRecordBytes = 64 * 1024

type Store struct {
	mu     sync.Mutex
	dir    string
	lastID uint64
	failed error
}

func Open(dir string) (*Store, error) {
	if err := privatefs.PrepareDirectory(filepath.Join(dir, "room.jsonl")); err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	s := &Store{dir: dir}
	// Include archived/deleted rooms when recovering the global message ID.
	for _, entry := range entries {
		if !strings.HasSuffix(entry.Name(), ".jsonl") {
			continue
		}
		id, err := strconv.ParseUint(strings.TrimSuffix(entry.Name(), ".jsonl"), 10, 64)
		if err != nil || id == 0 || entry.Name() != strconv.FormatUint(id, 10)+".jsonl" {
			return nil, fmt.Errorf("invalid chat filename: %s", entry.Name())
		}
		messages, err := s.Recent(id, 1)
		if err != nil {
			return nil, err
		}
		if len(messages) != 0 && messages[0].ID > s.lastID {
			s.lastID = messages[0].ID
		}
	}
	return s, nil
}

func (s *Store) LastID() uint64 { s.mu.Lock(); defer s.mu.Unlock(); return s.lastID }
func (s *Store) path(channel uint64) string {
	return filepath.Join(s.dir, strconv.FormatUint(channel, 10)+".jsonl")
}

func (s *Store) Recent(channel uint64, limit int) ([]protocol.TextMessage, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if channel == 0 || limit < 1 || limit > RecentLimit {
		return nil, errors.New("invalid chat history request")
	}
	path := s.path(channel)
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return []protocol.TextMessage{}, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	if err := privatefs.Ensure(path); err != nil {
		return nil, err
	}
	info, err := f.Stat()
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() {
		return nil, errors.New("chat history is not a regular file")
	}
	data, err := readTail(f, info.Size(), limit)
	if err != nil {
		return nil, fmt.Errorf("read room %d history: %w", channel, err)
	}
	messages := make([]protocol.TextMessage, 0, limit)
	if len(data) == 0 {
		return messages, nil
	}
	for _, line := range bytes.Split(bytes.TrimSuffix(data, []byte{'\n'}), []byte{'\n'}) {
		var message protocol.TextMessage
		if err := json.Unmarshal(line, &message); err != nil {
			return nil, fmt.Errorf("decode room %d history: %w", channel, err)
		}
		if message.ID == 0 || message.ChannelID != channel || (len(messages) > 0 && message.ID <= messages[len(messages)-1].ID) {
			return nil, errors.New("invalid chat record identity or ordering")
		}
		messages = append(messages, message)
	}
	return messages, nil
}

func readTail(r io.ReaderAt, size int64, limit int) ([]byte, error) {
	var tail []byte
	position := size
	for position > 0 {
		length := int64(4096)
		if position < length {
			length = position
		}
		position -= length
		chunk := make([]byte, length)
		if _, err := r.ReadAt(chunk, position); err != nil {
			return nil, err
		}
		tail = append(chunk, tail...)
		if tail[len(tail)-1] != '\n' {
			return nil, errors.New("incomplete chat record; original file preserved")
		}
		if len(tail) > (limit+1)*maxRecordBytes {
			return nil, errors.New("chat record exceeds read bound")
		}
		// The extra separator ensures the first retained record is complete.
		if bytes.Count(tail, []byte{'\n'}) > limit {
			break
		}
	}
	end := len(tail)
	for count := 0; end > 0 && count < limit; count++ {
		start := bytes.LastIndexByte(tail[:end-1], '\n') + 1
		if end-start > maxRecordBytes {
			return nil, errors.New("chat record too large")
		}
		end = start
	}
	return tail[end:], nil
}

func (s *Store) Append(message protocol.TextMessage) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.failed != nil {
		return s.failed
	}
	if message.ID <= s.lastID || message.ChannelID == 0 {
		return errors.New("invalid chat message ID")
	}
	data, err := json.Marshal(message)
	if err != nil {
		return err
	}
	if len(data)+1 > maxRecordBytes {
		return errors.New("chat record too large")
	}
	f, err := os.OpenFile(s.path(message.ChannelID), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	if err = f.Chmod(0o600); err == nil {
		_, err = f.Write(append(data, '\n'))
		if err == nil {
			err = f.Sync()
		}
	}
	closeErr := f.Close()
	if err == nil {
		err = closeErr
	}
	if err != nil {
		// Never append after an uncertain/partial write or overwrite old history.
		s.failed = err
		return err
	}
	s.lastID = message.ID
	return nil
}
