package control

import (
	"crypto/sha256"
	"crypto/subtle"
	"errors"
	"net"
	"sync"
	"time"
)

var errServerPasswordRequired = errors.New("server password required")
var errServerPasswordInvalid = errors.New("server password invalid")

// SetServerPassword configures admission before ListenAndServe starts.
func (s *Server) SetServerPassword(password string) { s.serverPassword = password }
func checkServerPassword(expected, supplied string) error {
	if expected == "" {
		return nil
	}
	if supplied == "" {
		return errServerPasswordRequired
	}
	if len(supplied) > 256 {
		return errServerPasswordInvalid
	}
	a, b := sha256.Sum256([]byte(expected)), sha256.Sum256([]byte(supplied))
	if subtle.ConstantTimeCompare(a[:], b[:]) != 1 {
		return errServerPasswordInvalid
	}
	return nil
}

// Bound work and memory before identity verification on password-protected servers.
// Twelve admissions per minute per source IP; successful reconnects count too.
type passwordAttempt struct {
	start time.Time
	count int
}
type passwordAttemptLimiter struct {
	mu      sync.Mutex
	entries map[string]passwordAttempt
}

func (l *passwordAttemptLimiter) allow(address string, now time.Time) bool {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		host = address
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.entries == nil {
		l.entries = make(map[string]passwordAttempt)
	}
	if len(l.entries) >= 4096 {
		for key, a := range l.entries {
			if now.Sub(a.start) >= time.Minute {
				delete(l.entries, key)
			}
		}
	}
	a, exists := l.entries[host]
	if !exists && len(l.entries) >= 4096 {
		return false
	}
	if !exists || now.Sub(a.start) >= time.Minute {
		a = passwordAttempt{start: now}
	}
	if a.count >= 12 {
		return false
	}
	a.count++
	l.entries[host] = a
	return true
}
