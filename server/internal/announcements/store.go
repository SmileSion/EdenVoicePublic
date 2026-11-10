package announcements

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

type Content struct {
	Title string `json:"title"`
	Body  string `json:"body"`
}
type Announcement struct {
	Content
	Revision    uint64 `json:"revision,string"`
	PublishedAt int64  `json:"published_at"`
}
type Document struct {
	Revision  uint64        `json:"revision,string"`
	Draft     Content       `json:"draft"`
	Published *Announcement `json:"published"`
}
type Store struct {
	mu   sync.Mutex
	path string
	doc  Document
}

func New() *Store { return &Store{doc: Document{Revision: 1}} }
func validate(c Content) error {
	if strings.TrimSpace(c.Title) == "" || strings.TrimSpace(c.Body) == "" || utf8.RuneCountInString(c.Title) > 80 || utf8.RuneCountInString(c.Body) > 3000 {
		return errors.New("标题需填写且不超过80字，正文需填写且不超过3000字")
	}
	return nil
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
	if s.doc.Revision == 0 {
		return nil, errors.New("invalid announcement revision")
	}
	if s.doc.Draft != (Content{}) {
		if e = validate(s.doc.Draft); e != nil {
			return nil, e
		}
	}
	if p := s.doc.Published; p != nil {
		if e = validate(p.Content); e != nil {
			return nil, e
		}
		if p.Revision == 0 || p.Revision > s.doc.Revision {
			return nil, errors.New("invalid publication revision")
		}
	}
	return s, nil
}
func clone(d Document) Document {
	if d.Published != nil {
		p := *d.Published
		d.Published = &p
	}
	return d
}
func (s *Store) Snapshot() Document { s.mu.Lock(); defer s.mu.Unlock(); return clone(s.doc) }
func (s *Store) Update(action string, revision uint64, c Content) (Document, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if revision != s.doc.Revision {
		return Document{}, errors.New("公告已被其他管理员修改，请刷新后重试")
	}
	d := clone(s.doc)
	switch action {
	case "save":
		c.Title = strings.TrimSpace(c.Title)
		c.Body = strings.TrimSpace(c.Body)
		if e := validate(c); e != nil {
			return Document{}, e
		}
		if d.Draft == c {
			return d, nil
		}
		d.Draft = c
	case "publish":
		if e := validate(d.Draft); e != nil {
			return Document{}, e
		}
		if d.Published != nil && d.Published.Content == d.Draft {
			return d, nil
		}
		d.Published = &Announcement{Content: d.Draft, Revision: d.Revision + 1, PublishedAt: time.Now().UnixMilli()}
	case "withdraw":
		if d.Published == nil {
			return d, nil
		}
		d.Published = nil
	default:
		return Document{}, errors.New("未知公告操作")
	}
	d.Revision++
	if s.path != "" {
		if e := os.MkdirAll(filepath.Dir(s.path), 0700); e != nil {
			return Document{}, e
		}
		f, e := os.CreateTemp(filepath.Dir(s.path), ".announcement-*")
		if e != nil {
			return Document{}, e
		}
		defer os.Remove(f.Name())
		b, _ := json.MarshalIndent(d, "", "  ")
		_, e = f.Write(b)
		if e == nil {
			e = f.Sync()
		}
		ce := f.Close()
		if e != nil {
			return Document{}, e
		}
		if ce != nil {
			return Document{}, ce
		}
		if e = os.Rename(f.Name(), s.path); e != nil {
			return Document{}, e
		}
	}
	s.doc = d
	return clone(d), nil
}
