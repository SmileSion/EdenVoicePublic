package announcements

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

func TestLatestPublicationPersistenceConflictAndWithdrawal(t *testing.T) {
	path := filepath.Join(t.TempDir(), "announcements.json")
	s, e := Open(path)
	if e != nil {
		t.Fatal(e)
	}
	first := Content{"欢迎", "今晚一起玩。"}
	d, e := s.Update("save", 1, first)
	if e != nil || d.Published != nil {
		t.Fatal(d, e)
	}
	d, e = s.Update("publish", d.Revision, Content{})
	if e != nil || d.Published.Content != first {
		t.Fatal(d, e)
	}
	id := d.Published.Revision
	same, e := s.Update("publish", d.Revision, Content{})
	if e != nil || same.Revision != d.Revision {
		t.Fatal("repeat publication must be idempotent")
	}
	if _, e = s.Update("save", 1, first); e == nil {
		t.Fatal("stale write allowed")
	}
	d, e = s.Update("save", d.Revision, Content{"新公告", "明天见"})
	if e != nil || d.Published.Revision != id {
		t.Fatal("draft changed live publication")
	}
	d, e = s.Update("publish", d.Revision, Content{})
	if e != nil || d.Published.Revision <= id || d.Published.Title != "新公告" {
		t.Fatal(d, e)
	}
	reopened, e := Open(path)
	if e != nil || reopened.Snapshot().Published.Revision != d.Published.Revision {
		t.Fatal("restart lost state", e)
	}
	d, e = s.Update("withdraw", d.Revision, Content{})
	if e != nil || d.Published != nil {
		t.Fatal(d, e)
	}
	again, e := Open(path)
	if e != nil || again.Snapshot().Published != nil {
		t.Fatal("withdraw not persisted")
	}
}
func TestInvalidOrFailedSaveDoesNotChangeState(t *testing.T) {
	path := filepath.Join(t.TempDir(), "document")
	s, _ := Open(path)
	for _, c := range []Content{{"", "x"}, {"x", " "}, {strings.Repeat("字", 81), "x"}, {"x", strings.Repeat("字", 3001)}} {
		if _, e := s.Update("save", 1, c); e == nil {
			t.Fatal("invalid content accepted")
		}
	}
	if e := os.Mkdir(path, 0700); e != nil {
		t.Fatal(e)
	}
	if _, e := s.Update("save", 1, Content{"标题", "正文"}); e == nil {
		t.Fatal("expected disk failure")
	}
	if s.Snapshot().Revision != 1 {
		t.Fatal("disk failure changed memory")
	}
}

func TestConcurrentEditorsCannotOverwriteEachOther(t *testing.T) {
	s := New()
	var wg sync.WaitGroup
	var accepted atomic.Int32
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, err := s.Update("save", 1, Content{"Title", "Body"}); err == nil {
				accepted.Add(1)
			}
			_ = s.Snapshot()
		}()
	}
	wg.Wait()
	if accepted.Load() != 1 || s.Snapshot().Revision != 2 {
		t.Fatal("concurrent editors overwrote revision")
	}
}
