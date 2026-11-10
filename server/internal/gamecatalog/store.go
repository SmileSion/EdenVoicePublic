package gamecatalog

import (
	"edenvoice/server/internal/protocol"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
)

type Document struct {
	Revision  uint64               `json:"revision"`
	Published protocol.GameCatalog `json:"published"`
	Draft     []protocol.Game      `json:"draft"`
	Previous  []protocol.Game      `json:"previous"`
}
type Store struct {
	mu   sync.Mutex
	path string
	doc  Document
}

func New() *Store {
	g := protocol.DefaultGames()
	return &Store{doc: Document{Revision: 1, Published: protocol.GameCatalog{Version: 1, Games: g}, Draft: g}}
}
func Open(path string) (*Store, error) {
	s := New()
	s.path = path
	b, e := os.ReadFile(path)
	if os.IsNotExist(e) {
		return s, nil
	}
	if e != nil {
		return nil, e
	}
	if e = json.Unmarshal(b, &s.doc); e != nil {
		return nil, e
	}
	if s.doc.Revision == 0 || s.doc.Published.Version == 0 {
		return nil, errors.New("invalid catalog version")
	}
	for _, g := range [][]protocol.Game{s.doc.Draft, s.doc.Published.Games} {
		if e = protocol.ValidateGames(g); e != nil {
			return nil, e
		}
	}
	if s.doc.Previous != nil {
		if e = protocol.ValidateGames(s.doc.Previous); e != nil {
			return nil, e
		}
	}
	return s, nil
}
func clone(d Document) Document {
	b, _ := json.Marshal(d)
	var out Document
	_ = json.Unmarshal(b, &out)
	return out
}
func (s *Store) Snapshot() Document { s.mu.Lock(); defer s.mu.Unlock(); return clone(s.doc) }
func (s *Store) Update(action string, revision uint64, games []protocol.Game) (Document, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if revision != s.doc.Revision {
		return Document{}, errors.New("目录已被其他管理员修改，请刷新后重试")
	}
	d := clone(s.doc)
	switch action {
	case "save":
		if e := protocol.ValidateGames(games); e != nil {
			return Document{}, e
		}
		d.Draft = games
	case "publish":
		d.Previous = d.Published.Games
		d.Published = protocol.GameCatalog{Version: d.Published.Version + 1, Games: d.Draft}
	case "rollback":
		if d.Previous == nil {
			return Document{}, errors.New("没有可回退版本")
		}
		prev := d.Previous
		d.Previous = d.Published.Games
		d.Published = protocol.GameCatalog{Version: d.Published.Version + 1, Games: prev}
		d.Draft = prev
	default:
		return Document{}, errors.New("未知目录操作")
	}
	d.Revision++
	if s.path != "" {
		if e := os.MkdirAll(filepath.Dir(s.path), 0700); e != nil {
			return Document{}, e
		}
		f, e := os.CreateTemp(filepath.Dir(s.path), ".games-*")
		if e != nil {
			return Document{}, e
		}
		defer os.Remove(f.Name())
		b, e := json.MarshalIndent(d, "", "  ")
		if e == nil {
			_, e = f.Write(b)
		}
		if e == nil {
			e = f.Sync()
		}
		ce := f.Close()
		if e == nil {
			e = ce
		}
		if e == nil {
			e = os.Rename(f.Name(), s.path)
		}
		if e != nil {
			return Document{}, e
		}
	}
	s.doc = clone(d)
	return clone(d), nil
}
