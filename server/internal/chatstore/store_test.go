package chatstore

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"edenvoice/server/internal/protocol"
)

func TestRestartLoadsOnlyTailAndRetainsAllRecords(t *testing.T) {
	dir := t.TempDir()
	store, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	for id := uint64(1); id <= 205; id++ {
		if err := store.Append(protocol.TextMessage{ID: id, ChannelID: 1, Text: "跨年\n消息😀", SentAtUnixMS: 1704067200000}); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.Append(protocol.TextMessage{ID: 206, ChannelID: 2, Text: "other room"}); err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if reopened.LastID() != 206 {
		t.Fatal("global message ID not recovered")
	}
	history, err := reopened.Recent(1, 100)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 100 || history[0].ID != 106 || history[99].ID != 205 || history[0].Text != "跨年\n消息😀" || history[0].SentAtUnixMS != 1704067200000 {
		t.Fatalf("wrong history: %+v", history)
	}
	data, err := os.ReadFile(filepath.Join(dir, "1.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Count(data, []byte{'\n'}) != 205 {
		t.Fatal("old records were discarded")
	}
	if err := reopened.Append(protocol.TextMessage{ID: 207, ChannelID: 1, Text: "after restart"}); err != nil {
		t.Fatal(err)
	}
	if err := reopened.Append(protocol.TextMessage{ID: 206, ChannelID: 1}); err == nil {
		t.Fatal("accepted reused ID")
	}
}

type measuredReader struct {
	*bytes.Reader
	read int
}

func (r *measuredReader) ReadAt(p []byte, off int64) (int, error) {
	n, err := r.Reader.ReadAt(p, off)
	r.read += n
	return n, err
}

func TestTailReadDoesNotScanFullArchive(t *testing.T) {
	data := bytes.Repeat([]byte("{\"text\":\"old\"}\n"), 100000)
	r := &measuredReader{Reader: bytes.NewReader(data)}
	tail, err := readTail(r, int64(len(data)), 100)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Count(tail, []byte{'\n'}) != 100 || r.read > 8192 {
		t.Fatalf("unbounded tail read: bytes=%d", r.read)
	}
}

func TestTailBoundariesAndCorruptionPreserveOriginal(t *testing.T) {
	for _, count := range []int{0, 1, 99, 100, 101} {
		var data []byte
		for id := 1; id <= count; id++ {
			line, _ := json.Marshal(protocol.TextMessage{ID: uint64(id), ChannelID: 1})
			data = append(data, append(line, '\n')...)
		}
		tail, err := readTail(bytes.NewReader(data), int64(len(data)), 100)
		if err != nil {
			t.Fatal(err)
		}
		if got := bytes.Count(tail, []byte{'\n'}); got != min(count, 100) {
			t.Fatalf("count=%d got=%d", count, got)
		}
	}
	dir := t.TempDir()
	path := filepath.Join(dir, "1.jsonl")
	broken := []byte("{\"id\":1,\"channel_id\":1}\n{\"id\":2")
	if err := os.WriteFile(path, broken, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(dir); err == nil {
		t.Fatal("incomplete record accepted")
	}
	after, _ := os.ReadFile(path)
	if !bytes.Equal(after, broken) {
		t.Fatal("corrupt archive was changed")
	}
	blankTail := []byte("{\"id\":42,\"channel_id\":1}\n\n")
	if err := os.WriteFile(path, blankTail, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(dir); err == nil {
		t.Fatal("blank trailing record silently reset message high-water ID")
	}
}
