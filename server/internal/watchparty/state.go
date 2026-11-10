// Package watchparty owns playback state independently of the voice media path.
package watchparty

import (
	"errors"
	"math"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

const maxDuration = 7 * 24 * 60 * 60

type File struct {
	Name     string  `json:"name"`
	Size     uint64  `json:"size"`
	Duration float64 `json:"duration"`
}

type Member struct {
	SessionID uint64 `json:"session_id"`
	File      *File  `json:"file,omitempty"`
	Ready     bool   `json:"ready"`
}

type Snapshot struct {
	ChannelID uint64   `json:"channel_id"`
	HostID    uint64   `json:"host_id"`
	Epoch     uint64   `json:"epoch"`
	Revision  uint64   `json:"revision"`
	Position  float64  `json:"position"`
	Paused    bool     `json:"paused"`
	Members   []Member `json:"members"`
}

type room struct {
	epoch, revision uint64
	host            uint64
	position        float64
	paused          bool
	updated         time.Time
	members         map[uint64]Member
}

// Manager has no goroutine, timer, disk IO or dependency on voice state.
// The control layer must authorize room membership before calling it.
type Manager struct {
	mu        sync.Mutex
	rooms     map[uint64]*room
	nextEpoch uint64
}

func New() *Manager { return &Manager{rooms: make(map[uint64]*room)} }

func (m *Manager) Join(channel, session uint64, now time.Time) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if channel == 0 || session == 0 {
		return Snapshot{}, errors.New("watching requires a voice room")
	}
	r := m.rooms[channel]
	if r == nil {
		m.nextEpoch++
		r = &room{epoch: m.nextEpoch, host: session, paused: true, updated: now, members: make(map[uint64]Member)}
		m.rooms[channel] = r
	}
	if _, exists := r.members[session]; !exists {
		// A bounded snapshot fits comfortably in the control message limit.
		if len(r.members) >= 64 {
			return Snapshot{}, errors.New("watch room is full (64 viewers)")
		}
		r.members[session] = Member{SessionID: session}
		r.revision++
	}
	return snapshot(channel, r, now), nil
}

func (m *Manager) Leave(channel, session uint64, now time.Time) (Snapshot, bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r := m.rooms[channel]
	if r == nil {
		return Snapshot{}, false
	}
	if _, exists := r.members[session]; !exists {
		return Snapshot{}, false
	}
	delete(r.members, session)
	if r.host == session {
		r.host = 0
		for id := range r.members {
			if r.host == 0 || id < r.host {
				r.host = id
			}
		}
	}
	r.revision++
	s := snapshot(channel, r, now)
	if len(r.members) == 0 {
		delete(m.rooms, channel)
	}
	return s, true
}

func validFile(f *File) bool {
	return f != nil && f.Name != "" && len(f.Name) <= 128 && utf8.ValidString(f.Name) &&
		!strings.ContainsAny(f.Name, "/\\\x00\r\n") && finitePosition(f.Duration) && f.Duration > 0
}

func finitePosition(p float64) bool {
	return !math.IsNaN(p) && !math.IsInf(p, 0) && p >= 0 && p <= maxDuration
}

// UpdateFile copies the metadata: callers cannot mutate shared room state.
func (m *Manager) UpdateFile(channel, session, epoch uint64, f *File, ready bool, now time.Time) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, err := m.member(channel, session, epoch)
	if err != nil {
		return Snapshot{}, err
	}
	if !validFile(f) {
		return Snapshot{}, errors.New("invalid watch file metadata")
	}
	copyFile := *f
	r.members[session] = Member{SessionID: session, File: &copyFile, Ready: ready}
	r.revision++
	return snapshot(channel, r, now), nil
}

// Control applies absolute positions, not deltas, so duplicated seek messages
// cannot accumulate movement. Epoch rejects messages from a destroyed session.
func (m *Manager) Control(channel, session, epoch uint64, action string, position float64, now time.Time, moderator bool) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, err := m.member(channel, session, epoch)
	if err != nil {
		return Snapshot{}, err
	}
	if !finitePosition(position) {
		return Snapshot{}, errors.New("invalid watch position")
	}
	if action != "play" && action != "pause" && action != "seek" {
		return Snapshot{}, errors.New("invalid watch action")
	}
	if !r.members[session].Ready {
		return Snapshot{}, errors.New("player is not ready")
	}
	if r.host != session && !moderator {
		return Snapshot{}, errors.New("only the watch host or a room administrator can control playback")
	}
	r.position, r.updated = position, now
	if action == "play" {
		r.paused = false
	}
	if action == "pause" {
		r.paused = true
	}
	r.revision++
	return snapshot(channel, r, now), nil
}

func (m *Manager) Read(channel, session, epoch uint64, now time.Time) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, err := m.member(channel, session, epoch)
	if err != nil {
		return Snapshot{}, err
	}
	return snapshot(channel, r, now), nil
}

func (m *Manager) member(channel, session, epoch uint64) (*room, error) {
	r := m.rooms[channel]
	if r == nil || r.epoch != epoch {
		return nil, errors.New("watch session expired")
	}
	if _, ok := r.members[session]; !ok {
		return nil, errors.New("not a watch participant")
	}
	return r, nil
}

func snapshot(channel uint64, r *room, now time.Time) Snapshot {
	p := r.position
	if !r.paused {
		p += math.Max(0, now.Sub(r.updated).Seconds())
	}
	s := Snapshot{ChannelID: channel, HostID: r.host, Epoch: r.epoch, Revision: r.revision, Position: math.Min(p, maxDuration), Paused: r.paused, Members: make([]Member, 0, len(r.members))}
	for _, member := range r.members {
		if member.File != nil {
			f := *member.File
			member.File = &f
		}
		s.Members = append(s.Members, member)
	}
	sort.Slice(s.Members, func(i, j int) bool { return s.Members[i].SessionID < s.Members[j].SessionID })
	return s
}

// Status is authorized by the control layer for members of the voice room.
func (m *Manager) Status(channel uint64, now time.Time) Snapshot {
	m.mu.Lock()
	defer m.mu.Unlock()
	if r := m.rooms[channel]; r != nil {
		return snapshot(channel, r, now)
	}
	return Snapshot{ChannelID: channel, Paused: true}
}

func (m *Manager) Close(channel, session, epoch uint64, now time.Time, moderator bool) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r := m.rooms[channel]
	if r == nil || r.epoch != epoch {
		return Snapshot{}, errors.New("watch session expired")
	}
	if r.host != session && !moderator {
		return Snapshot{}, errors.New("only the watch host or a room administrator can close the room")
	}
	r.revision++
	result := snapshot(channel, r, now)
	delete(m.rooms, channel)
	return result, nil
}

// Moderation is authorized by the control layer against the current channel.
// A moderator need not join watching or launch a player to manage the room.
func (m *Manager) TransferHost(channel, session, epoch, target uint64, now time.Time, moderator bool) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r := m.rooms[channel]
	if r == nil || r.epoch != epoch {
		return Snapshot{}, errors.New("watch session expired")
	}
	if r.host != session && !moderator {
		return Snapshot{}, errors.New("only the watch host or a room administrator can transfer hosting")
	}
	if _, exists := r.members[target]; !exists || target == 0 {
		return Snapshot{}, errors.New("new host must be a current watch participant")
	}
	if r.host != target {
		r.host = target
		r.revision++
	}
	return snapshot(channel, r, now), nil
}

func (m *Manager) Unready(channel, session, epoch uint64, now time.Time) (Snapshot, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	r, err := m.member(channel, session, epoch)
	if err != nil {
		return Snapshot{}, err
	}
	r.members[session] = Member{SessionID: session}
	r.revision++
	return snapshot(channel, r, now), nil
}
