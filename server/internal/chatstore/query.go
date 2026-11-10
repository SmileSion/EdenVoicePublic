package chatstore

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"strconv"
	"strings"

	"edenvoice/server/internal/privatefs"
	"edenvoice/server/internal/protocol"
)

// Query scans at most 1 MiB per request. The cursor is a byte boundary in the
// append-only room log; concurrent appends cannot move previously read pages.
func (s *Store) Query(channel uint64, cursor, query string, before ...uint64) ([]protocol.TextMessage, string, error) {
	if channel == 0 || len(query) > 256 {
		return nil, "", errors.New("invalid history query")
	}
	path := s.path(channel)
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return []protocol.TextMessage{}, "", nil
	}
	if err != nil {
		return nil, "", err
	}
	defer f.Close()
	if err = privatefs.Ensure(path); err != nil {
		return nil, "", err
	}
	info, err := f.Stat()
	if err != nil {
		return nil, "", err
	}
	if !info.Mode().IsRegular() {
		return nil, "", errors.New("invalid history file")
	}
	end := info.Size()
	if cursor != "" {
		end, err = strconv.ParseInt(cursor, 10, 64)
		if err != nil || end < 0 || end > info.Size() {
			return nil, "", errors.New("invalid history cursor")
		}
	}
	start := end - 1024*1024
	if start < 0 {
		start = 0
	}
	data := make([]byte, end-start)
	if _, err = f.ReadAt(data, start); err != nil && !errors.Is(err, io.EOF) {
		return nil, "", err
	}
	if len(data) > 0 && data[len(data)-1] != '\n' {
		return nil, "", errors.New("history is being written; retry shortly")
	}
	if start > 0 {
		i := bytes.IndexByte(data, '\n')
		if i < 0 {
			return nil, "", errors.New("invalid history record")
		}
		data, start = data[i+1:], start+int64(i+1)
	}
	type match struct {
		message protocol.TextMessage
		offset  int64
		bytes   int
	}
	matches := make([]match, 0, 20)
	offset := start
	totalBytes := 0
	dropped := false
	previousID := uint64(0)
	needle := strings.ToLower(strings.TrimSpace(query))
	lines := bytes.Split(data, []byte{'\n'})
	for index, line := range lines {
		if len(line) == 0 {
			if index != len(lines)-1 {
				return nil, "", errors.New("empty history record")
			}
			continue
		}
		if len(line) > maxRecordBytes {
			return nil, "", errors.New("history record too large")
		}
		var message protocol.TextMessage
		if json.Unmarshal(line, &message) != nil || message.ChannelID != channel || message.ID <= previousID {
			return nil, "", errors.New("invalid history record")
		}
		previousID = message.ID
		if (len(before) == 0 || before[0] == 0 || message.ID < before[0]) && (needle == "" || strings.Contains(strings.ToLower(message.Text), needle) || strings.Contains(strings.ToLower(message.SenderName), needle)) {
			encoded, err := json.Marshal(message)
			if err != nil {
				return nil, "", err
			}
			if len(encoded) > 48*1024 {
				return nil, "", errors.New("history record exceeds response limit")
			}
			matches = append(matches, match{message, offset, len(encoded)})
			totalBytes += len(encoded)
			for len(matches) > 20 || totalBytes > 48*1024 {
				totalBytes -= matches[0].bytes
				matches = matches[1:]
				dropped = true
			}
		}
		offset += int64(len(line) + 1)
	}
	next := start
	if len(matches) > 0 && (len(matches) == 20 || dropped) {
		next = matches[0].offset
	}
	messages := make([]protocol.TextMessage, 0, len(matches))
	for _, match := range matches {
		messages = append(messages, match.message)
	}
	nextCursor := ""
	if next > 0 {
		nextCursor = strconv.FormatInt(next, 10)
	}
	return messages, nextCursor, nil
}
