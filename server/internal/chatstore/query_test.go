package chatstore

import (
	"edenvoice/server/internal/protocol"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func TestQueryPaginationSearchAndAppendIsolation(t *testing.T) {
	s, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for id := uint64(1); id <= 250; id++ {
		text := fmt.Sprint("message ", id)
		if id%7 == 0 {
			text = "older needle"
		}
		if err := s.Append(protocol.TextMessage{ID: id, ChannelID: 1, Text: text, SenderName: "Alice"}); err != nil {
			t.Fatal(err)
		}
	}
	page, cursor, err := s.Query(1, "", "", 151)
	if err != nil || len(page) != 20 || page[0].ID != 131 || page[19].ID != 150 || cursor == "" {
		t.Fatalf("bad first page %v %q %v", page, cursor, err)
	}
	if err := s.Append(protocol.TextMessage{ID: 251, ChannelID: 1, Text: "new"}); err != nil {
		t.Fatal(err)
	}
	next, _, err := s.Query(1, cursor, "")
	if err != nil || len(next) != 20 || next[19].ID != 130 {
		t.Fatalf("page moved after append: %v %v", next, err)
	}
	seen := map[uint64]bool{}
	cursor = ""
	for {
		page, next, err := s.Query(1, cursor, "NEEDLE")
		if err != nil {
			t.Fatal(err)
		}
		for _, m := range page {
			if m.ID%7 != 0 || seen[m.ID] {
				t.Fatalf("wrong or duplicate match %d", m.ID)
			}
			seen[m.ID] = true
		}
		if next == "" {
			break
		}
		if next == cursor {
			t.Fatal("cursor did not advance")
		}
		cursor = next
	}
	if len(seen) != 35 {
		t.Fatalf("missing matches: %d", len(seen))
	}
	for _, bad := range []string{"-1", "999999999999", "not-a-cursor"} {
		if _, _, err := s.Query(1, bad, ""); err == nil {
			t.Fatalf("accepted %q", bad)
		}
	}
}

func TestQueryBoundsEscapedResponses(t *testing.T) {
	s, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for id := uint64(1); id <= 40; id++ {
		if err := s.Append(protocol.TextMessage{ID: id, ChannelID: 1, Text: strings.Repeat("\x00", 2048)}); err != nil {
			t.Fatal(err)
		}
	}
	seen := map[uint64]bool{}
	cursor := ""
	for {
		messages, next, err := s.Query(1, cursor, "")
		if err != nil {
			t.Fatal(err)
		}
		encoded, _ := json.Marshal(messages)
		if len(encoded) > 50*1024 {
			t.Fatalf("response too large: %d", len(encoded))
		}
		for _, m := range messages {
			if seen[m.ID] {
				t.Fatal("duplicate")
			}
			seen[m.ID] = true
		}
		if next == "" {
			break
		}
		cursor = next
	}
	if len(seen) != 40 {
		t.Fatalf("missing records %d", len(seen))
	}
}
