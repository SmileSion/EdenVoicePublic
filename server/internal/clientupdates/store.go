package clientupdates

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
)

type Release struct {
	Platform  string `json:"platform"`
	Version   string `json:"version"`
	URL       string `json:"url"`
	Signature string `json:"signature"`
	Notes     string `json:"notes"`
}
type Entry struct {
	Draft     Release  `json:"draft"`
	Published *Release `json:"published"`
	Paused    bool     `json:"paused"`
}
type Document struct {
	Revision uint64           `json:"revision"`
	Entries  map[string]Entry `json:"entries"`
}
type Store struct {
	mu   sync.Mutex
	path string
	doc  Document
}

var Platforms = map[string]bool{"windows-x86_64": true, "darwin-aarch64": true, "darwin-x86_64": true, "linux-x86_64": true, "linux-aarch64": true}
var version = regexp.MustCompile(`^(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})\.(0|[1-9][0-9]{0,5})$`)

func Validate(r Release) error {
	u, e := url.Parse(r.URL)
	sig, se := base64.StdEncoding.DecodeString(r.Signature)
	if !Platforms[r.Platform] || !version.MatchString(r.Version) || len(r.Notes) > 2048 || len(r.URL) > 2048 || len(r.Signature) > 2048 || e != nil || u.Scheme != "https" || u.Hostname() == "" || u.User != nil || u.Fragment != "" || u.RawQuery != "" || se != nil || !signatureFormat(sig) {
		return errors.New("请填写有效的平台、正式版本号、HTTPS 固定下载地址和完整签名；不支持临时链接")
	}
	return nil
}
func signatureFormat(sig []byte) bool {
	lines := strings.Split(strings.TrimSpace(string(sig)), "\n")
	if len(lines) != 4 || !strings.HasPrefix(lines[0], "untrusted comment:") || !strings.HasPrefix(lines[2], "trusted comment:") {
		return false
	}
	payload, e := base64.StdEncoding.DecodeString(strings.TrimSpace(lines[1]))
	global, g := base64.StdEncoding.DecodeString(strings.TrimSpace(lines[3]))
	return e == nil && g == nil && len(payload) == 74 && len(global) == 64
}
func New() *Store { return &Store{doc: Document{Revision: 1, Entries: map[string]Entry{}}} }
func Open(path string) (*Store, error) {
	s := &Store{path: path, doc: Document{Revision: 1, Entries: map[string]Entry{}}}
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
	if s.doc.Revision == 0 || s.doc.Entries == nil {
		return nil, errors.New("invalid update document")
	}
	for p, v := range s.doc.Entries {
		if p != v.Draft.Platform {
			return nil, errors.New("invalid platform")
		}
		if e = Validate(v.Draft); e != nil {
			return nil, e
		}
		if v.Published != nil {
			if e = Validate(*v.Published); e != nil || v.Published.Platform != p {
				return nil, errors.New("invalid published update")
			}
		}
	}
	return s, nil
}
func clone(d Document) Document {
	b, _ := json.Marshal(d)
	var o Document
	_ = json.Unmarshal(b, &o)
	return o
}
func (s *Store) Snapshot() Document { s.mu.Lock(); defer s.mu.Unlock(); return clone(s.doc) }
func (s *Store) Published() []Release {
	d := s.Snapshot()
	out := []Release{}
	for _, v := range d.Entries {
		if !v.Paused && v.Published != nil {
			out = append(out, *v.Published)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Platform < out[j].Platform })
	return out
}
func (s *Store) Update(action string, revision uint64, r Release) (Document, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if revision != s.doc.Revision {
		return Document{}, errors.New("配置已变化，请刷新后重试")
	}
	if !Platforms[r.Platform] {
		return Document{}, errors.New("未知平台")
	}
	d := clone(s.doc)
	v, exists := d.Entries[r.Platform]
	switch action {
	case "save":
		if e := Validate(r); e != nil {
			return Document{}, e
		}
		v.Draft = r
	case "publish":
		if !exists {
			return Document{}, errors.New("请先保存草稿")
		}
		p := v.Draft
		v.Published = &p
		v.Paused = false
	case "pause":
		if !exists || v.Published == nil {
			return Document{}, errors.New("没有已发布版本")
		}
		v.Paused = true
	default:
		return Document{}, errors.New("未知更新操作")
	}
	d.Entries[r.Platform] = v
	d.Revision++
	if s.path != "" {
		if e := os.MkdirAll(filepath.Dir(s.path), 0700); e != nil {
			return Document{}, e
		}
		f, e := os.CreateTemp(filepath.Dir(s.path), ".updates-*")
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
