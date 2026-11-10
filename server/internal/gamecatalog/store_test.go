package gamecatalog

import (
	"edenvoice/server/internal/protocol"
	"path/filepath"
	"testing"
)

func TestDraftPublishRollbackPersistenceAndCAS(t *testing.T) {
	path := filepath.Join(t.TempDir(), "games.json")
	s, e := Open(path)
	if e != nil {
		t.Fatal(e)
	}
	d := s.Snapshot()
	g := protocol.DefaultGames()
	g[0].Title = "新名字"
	d, e = s.Update("save", d.Revision, g)
	if e != nil {
		t.Fatal(e)
	}
	if d.Published.Games[0].Title == "新名字" {
		t.Fatal("draft leaked")
	}
	if _, e = s.Update("publish", 1, nil); e == nil {
		t.Fatal("stale edit accepted")
	}
	d, e = s.Update("publish", d.Revision, nil)
	if e != nil || d.Published.Version != 2 || d.Published.Games[0].Title != "新名字" {
		t.Fatal(d, e)
	}
	s, e = Open(path)
	if e != nil {
		t.Fatal(e)
	}
	d, e = s.Update("rollback", d.Revision, nil)
	if e != nil || d.Published.Version != 3 || d.Published.Games[0].Title == "新名字" {
		t.Fatal(d, e)
	}
	d.Draft[0].Title = "mutated"
	if s.Snapshot().Draft[0].Title == "mutated" {
		t.Fatal("mutable snapshot")
	}
	for _, exe := range []string{"javaw.exe", "../cs2.exe", "*.exe", "Steam.exe", "cs2.exe\n"} {
		g := protocol.DefaultGames()
		g[0].Executables = []string{exe}
		if protocol.ValidateGames(g) == nil {
			t.Fatal(exe)
		}
	}
}
