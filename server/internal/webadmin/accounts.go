// Package webadmin serves the browser console independently of the voice path.
package webadmin

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"sync"
	"time"

	"golang.org/x/crypto/bcrypt"

	"edenvoice/server/internal/privatefs"
)

var ErrCredentials = errors.New("账号或密码错误")
var usernamePattern = regexp.MustCompile(`^[a-zA-Z0-9_.-]{3,48}$`)

type Account struct {
	Username     string    `json:"username"`
	ClientID     string    `json:"client_id"`
	ReadOnly     bool      `json:"read_only"`
	Disabled     bool      `json:"disabled"`
	CreatedAt    time.Time `json:"created_at"`
	Version      string    `json:"version"`
	PasswordHash string    `json:"password_hash,omitempty"`
}
type Accounts struct {
	mu    sync.RWMutex
	path  string
	users map[string]Account
	dummy []byte
}

func OpenAccounts(path string) (*Accounts, error) {
	a := &Accounts{path: path, users: map[string]Account{}}
	raw, err := os.ReadFile(path)
	if err == nil {
		if err = privatefs.Ensure(path); err != nil {
			return nil, err
		}
		if err = json.Unmarshal(raw, &a.users); err != nil {
			return nil, err
		}
		if a.users == nil {
			return nil, errors.New("invalid web account store")
		}
	} else if !os.IsNotExist(err) {
		return nil, err
	}
	a.dummy, err = bcrypt.GenerateFromPassword([]byte(randomValue()), 12)
	return a, err
}
func randomValue() string {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}
func (a *Accounts) Get(name string) (Account, bool) {
	a.mu.RLock()
	defer a.mu.RUnlock()
	u, ok := a.users[name]
	return u, ok
}
func (a *Accounts) List() []Account {
	a.mu.RLock()
	defer a.mu.RUnlock()
	out := []Account{}
	for _, u := range a.users {
		u.PasswordHash = ""
		u.Version = ""
		out = append(out, u)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Username < out[j].Username })
	return out
}
func (a *Accounts) Authenticate(name, password string) (Account, error) {
	u, ok := a.Get(name)
	hash := a.dummy
	if ok {
		hash = []byte(u.PasswordHash)
	}
	err := bcrypt.CompareHashAndPassword(hash, []byte(password))
	if err != nil || !ok || u.Disabled {
		return Account{}, ErrCredentials
	}
	return u, nil
}
func (a *Accounts) Put(name, clientID, password string, readOnly, disabled bool) error {
	if !usernamePattern.MatchString(name) || clientID == "" || len(clientID) > 128 {
		return errors.New("账号需为 3–48 位字母、数字、点、下划线或短横线，并绑定管理员身份")
	}
	var hash []byte
	var err error
	if password != "" {
		if len(password) < 12 || len(password) > 72 {
			return errors.New("密码需为 12–72 字节")
		}
		hash, err = bcrypt.GenerateFromPassword([]byte(password), 12)
		if err != nil {
			return err
		}
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	old, exists := a.users[name]
	if !exists && password == "" {
		return errors.New("新账号需要设置密码")
	}
	if !exists && len(a.users) >= 100 {
		return errors.New("最多允许 100 个 Web 账号")
	}
	u := Account{Username: name, ClientID: clientID, ReadOnly: readOnly, Disabled: disabled, CreatedAt: old.CreatedAt, Version: randomValue(), PasswordHash: old.PasswordHash}
	if !exists {
		u.CreatedAt = time.Now()
	}
	if hash != nil {
		u.PasswordHash = string(hash)
	}
	next := make(map[string]Account, len(a.users)+1)
	for k, v := range a.users {
		next[k] = v
	}
	next[name] = u
	if err = writePrivateJSON(a.path, next); err != nil {
		return err
	}
	a.users = next
	return nil
}
func writePrivateJSON(path string, value any) error {
	if err := privatefs.PrepareDirectory(path); err != nil {
		return err
	}
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".web-admin-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err = f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}
