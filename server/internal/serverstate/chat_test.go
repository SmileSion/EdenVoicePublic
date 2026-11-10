package serverstate

import (
	"errors"
	"testing"
	"time"

	"edenvoice/server/internal/chatstore"
	"edenvoice/server/internal/protocol"
)

func TestPersistentRoomHistorySurvivesStateRestart(t *testing.T) {
	dir := t.TempDir()
	store, err := chatstore.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	state := newTestState()
	if err := state.RestoreChat(store); err != nil {
		t.Fatal(err)
	}
	alice := state.AddAuthenticatedSession("Alice", "stable-client", "USER", "", nil, nil, nil)
	if err := state.Join(alice.ID, 1); err != nil {
		t.Fatal(err)
	}
	for id := 0; id < 110; id++ {
		if _, err := state.RecordTextMessage(alice.ID, "saved", time.Unix(int64(id+1), 0)); err != nil {
			t.Fatal(err)
		}
	}
	reopened, err := chatstore.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	restarted := newTestState()
	if err := restarted.RestoreChat(reopened); err != nil {
		t.Fatal(err)
	}
	history := restarted.ChatHistory(1)
	if len(history) != 100 || history[0].ID != 11 || history[99].ID != 110 || history[0].SenderClientID != "stable-client" {
		t.Fatal("history or stable sender identity not restored")
	}
	if len(restarted.ChatHistory(2)) != 0 {
		t.Fatal("history leaked to another room")
	}
	bob := restarted.AddSession("Bob", nil)
	_ = restarted.Join(bob.ID, 2)
	message, err := restarted.RecordTextMessage(bob.ID, "new", time.Now())
	if err != nil || message.ID != 111 {
		t.Fatalf("message IDs reused: %+v %v", message, err)
	}
}

type blockedChatStore struct {
	entered, release chan struct{}
	err              error
}

func (s *blockedChatStore) LastID() uint64 { return 0 }
func (s *blockedChatStore) Recent(uint64, int) ([]protocol.TextMessage, error) {
	return []protocol.TextMessage{}, nil
}
func (s *blockedChatStore) Append(protocol.TextMessage) error {
	close(s.entered)
	<-s.release
	return s.err
}

func TestChatDiskWaitDoesNotHoldVoiceStateLockOrPublishUnsavedMessage(t *testing.T) {
	store := &blockedChatStore{entered: make(chan struct{}), release: make(chan struct{}), err: errors.New("disk full")}
	defer close(store.release)
	state := newTestState()
	if err := state.RestoreChat(store); err != nil {
		t.Fatal(err)
	}
	session := state.AddSession("listener", nil)
	_ = state.Join(session.ID, 1)
	done := make(chan error, 1)
	go func() { _, err := state.RecordTextMessage(session.ID, "unsaved", time.Now()); done <- err }()
	select {
	case <-store.entered:
	case <-time.After(time.Second):
		t.Fatal("append not reached")
	}
	read := make(chan bool, 1)
	go func() { _, ok := state.SessionByID(session.ID); read <- ok }()
	select {
	case ok := <-read:
		if !ok {
			t.Fatal("missing session")
		}
	case <-time.After(time.Second):
		t.Fatal("disk I/O blocked shared voice state lock")
	}
	if len(state.ChatHistory(1)) != 0 {
		t.Fatal("unsaved message published")
	}
	// Let the write fail, checking the sender receives an explicit error.
	store.release <- struct{}{}
	if err := <-done; !errors.Is(err, ErrChatUnavailable) {
		t.Fatalf("missing persistence error: %v", err)
	}
	if len(state.ChatHistory(1)) != 0 {
		t.Fatal("failed message cached")
	}
}

func TestUnavailableChatStoreDoesNotDisableVoiceState(t *testing.T) {
	state := newTestState()
	if err := state.RestoreChat(nil); err == nil {
		t.Fatal("missing store accepted")
	}
	session := state.AddSession("voice still available", nil)
	if err := state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	if _, err := state.RecordTextMessage(session.ID, "not durable", time.Now()); !errors.Is(err, ErrChatUnavailable) {
		t.Fatal("silently accepted volatile message")
	}
}
