package serverstate

import "regexp"

var clientVersionPattern = regexp.MustCompile(`^[0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6}([-+][A-Za-z0-9.-]{1,32})?$`)

// Build metadata is self-reported, never an authentication or capability check.
// Invalid metadata must not prevent an otherwise valid voice connection.
func (s *State) SetClientBuild(id uint64, version, platform string) {
	if len(version) > 64 || !clientVersionPattern.MatchString(version) {
		version = ""
	}
	switch platform {
	case "windows", "macos", "linux":
	default:
		platform = ""
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if session := s.sessions[id]; session != nil {
		session.ClientVersion = version
		session.ClientPlatform = platform
	}
}
