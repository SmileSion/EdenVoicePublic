package administration

import (
	"errors"
	"golang.org/x/crypto/bcrypt"
	"sort"
)

// Only hashes are persisted; the public snapshot includes locked room IDs only.
func (s *Store) SetChannelPassword(id uint64, password string) error {
	if len(password) > 72 {
		return errors.New("房间密码不能超过 72 字节")
	}
	var hash string
	if password != "" {
		data, err := bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)
		if err != nil {
			return err
		}
		hash = string(data)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.state.Channels[id]; !ok {
		return errors.New("channel not found")
	}
	if s.state.ChannelPasswords == nil {
		s.state.ChannelPasswords = make(map[uint64]string)
	}
	previous := s.state.ChannelPasswords[id]
	if hash == "" {
		delete(s.state.ChannelPasswords, id)
	} else {
		s.state.ChannelPasswords[id] = hash
	}
	if err := s.persistLocked(); err != nil {
		if previous == "" {
			delete(s.state.ChannelPasswords, id)
		} else {
			s.state.ChannelPasswords[id] = previous
		}
		return err
	}
	return nil
}
func (s *Store) CheckChannelPassword(id uint64, password string) error {
	s.mu.RLock()
	hash := s.state.ChannelPasswords[id]
	s.mu.RUnlock()
	if hash == "" {
		return nil
	}
	if len(password) > 72 || bcrypt.CompareHashAndPassword([]byte(hash), []byte(password)) != nil {
		return errors.New("房间密码错误或未填写，请输入密码后重试")
	}
	return nil
}
func (s *Store) LockedChannelIDs() []uint64 {
	s.mu.RLock()
	defer s.mu.RUnlock()
	ids := []uint64{}
	for id, hash := range s.state.ChannelPasswords {
		if _, ok := s.state.Channels[id]; ok && hash != "" {
			ids = append(ids, id)
		}
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return ids
}

func hashChannelPassword(password string) (string, error) {
	if len(password) > 72 {
		return "", errors.New("房间密码不能超过 72 字节")
	}
	if password == "" {
		return "", nil
	}
	hash, err := bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)
	return string(hash), err
}
