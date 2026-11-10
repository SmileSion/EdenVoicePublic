package control

import (
	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/watchparty"
	"errors"
	"time"
)

type AdminAudit struct {
	At        time.Time `json:"at"`
	Event     string    `json:"event"`
	Operator  string    `json:"operator"`
	Target    string    `json:"target"`
	ChannelID uint64    `json:"channel_id"`
}

func (s *Server) AdminAudit() []AdminAudit {
	s.auditMu.Lock()
	defer s.auditMu.Unlock()
	return append([]AdminAudit{}, s.auditEvents...)
}
func (s *Server) WebAudit(event, actor, target string) { s.audit(event, actor, target, 0) }

// A web login never creates a voice session and cannot claim voice membership.
func (s *Server) WebRole(clientID string) (string, error) {
	if s.administration == nil {
		return "", administration.ErrForbidden
	}
	u, ok := s.administration.User(clientID)
	if !ok || (u.ServerRole != administration.RoleMaster && u.ServerRole != administration.RoleAdmin) {
		return "", administration.ErrForbidden
	}
	for _, b := range s.administration.Bans(time.Now()) {
		if b.ClientID == clientID {
			return "", administration.ErrForbidden
		}
	}
	return string(u.ServerRole), nil
}
func (s *Server) WebCommand(clientID string, envelope protocol.Envelope) error {
	if _, err := s.WebRole(clientID); err != nil {
		return err
	}
	if envelope.Type == protocol.TypeBan {
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if len(request.Reason) > 256 {
			return errors.New("封禁原因不能超过 256 字节")
		}
		if request.ExpiresAt != nil && !request.ExpiresAt.After(time.Now()) {
			return errors.New("封禁到期时间必须晚于当前时间")
		}
	}
	switch envelope.Type {
	case protocol.TypeCreateChannel, protocol.TypeEditChannel, protocol.TypeDeleteChannel, protocol.TypeSetChannelAdmin, protocol.TypeSetServerRole, protocol.TypeAdminMute, protocol.TypeMoveUser, protocol.TypeKickChannel, protocol.TypeKickServer, protocol.TypeBan, protocol.TypeUnban, protocol.TypeSetRoomNetworkPolicy:
		return s.dispatchAdministration(0, clientID, envelope, nil)
	default:
		return errors.New("unsupported web action")
	}
}
func (s *Server) WebCloseWatch(clientID string, channel, epoch uint64) error {
	if _, err := s.WebRole(clientID); err != nil {
		return err
	}
	if err := s.authorize(clientID, administration.ActionWatchManage, administration.Resource{ChannelID: channel}); err != nil {
		return err
	}
	s.watchMu.Lock()
	defer s.watchMu.Unlock()
	snapshot, err := s.watch.Close(channel, 0, epoch, time.Now(), true)
	if err != nil {
		return err
	}
	s.broadcastWatchLocked(protocol.WatchState{Snapshot: snapshot, Action: "closed"})
	for _, m := range snapshot.Members {
		delete(s.watchMembers, m.SessionID)
		delete(s.watchLastRequest, m.SessionID)
	}
	s.audit("watch room closed via web", clientID, "", channel)
	return nil
}

type AdminPeer struct {
	protocol.NetworkReport
	At time.Time `json:"at"`
}
type AdminConnection struct {
	ClientVersion  string      `json:"client_version"`
	ClientPlatform string      `json:"client_platform"`
	SessionID      uint64      `json:"session_id"`
	ClientID       string      `json:"client_id"`
	Nickname       string      `json:"nickname"`
	ChannelID      uint64      `json:"channel_id"`
	TCPAddress     string      `json:"tcp_address"`
	UDPAddress     string      `json:"udp_address"`
	ConnectedAt    time.Time   `json:"connected_at"`
	LastHeartbeat  time.Time   `json:"last_heartbeat"`
	TransportMode  string      `json:"transport_mode"`
	Encrypted      bool        `json:"encrypted"`
	Reports        []AdminPeer `json:"reports"`
}
type AdminData struct {
	Endpoints     map[string]string     `json:"endpoints"`
	State         protocol.ServerState  `json:"state"`
	Profiles      []administration.User `json:"profiles"`
	Bans          []administration.Ban  `json:"bans"`
	Connections   []AdminConnection     `json:"connections"`
	Watches       []watchparty.Snapshot `json:"watches"`
	Audit         []AdminAudit          `json:"audit"`
	ChannelAdmins map[uint64][]string   `json:"channel_admins"`
}

func (s *Server) WebSnapshot(clientID string) (AdminData, error) {
	if _, err := s.WebRole(clientID); err != nil {
		return AdminData{}, err
	}
	data := AdminData{State: s.snapshot(), Profiles: s.administration.Profiles(), Bans: s.administration.Bans(time.Now()), Connections: []AdminConnection{}, Watches: []watchparty.Snapshot{}, Audit: s.AdminAudit(), ChannelAdmins: map[uint64][]string{}}
	data.Endpoints = map[string]string{"control": s.addr, "voice": s.voiceAddr, "fingerprint": ""}
	if s.identity != nil {
		data.Endpoints["fingerprint"] = s.identity.Fingerprint()
	}
	for _, u := range data.State.Users {
		x, ok := s.state.SessionByID(u.SessionID)
		if !ok {
			continue
		}
		c := AdminConnection{ClientVersion: x.ClientVersion, ClientPlatform: x.ClientPlatform, SessionID: x.ID, ClientID: x.ClientID, Nickname: x.Nickname, ChannelID: x.ChannelID, ConnectedAt: x.ConnectedAt, LastHeartbeat: x.LastHeartbeat, TransportMode: x.TransportMode, Encrypted: x.MediaInbound != nil, Reports: []AdminPeer{}}
		if sender, ok := x.Sender.(*connSender); ok {
			c.TCPAddress = sender.conn.RemoteAddr().String()
		}
		if x.UDPAddr != nil {
			c.UDPAddress = x.UDPAddr.String()
		}
		for id, report := range x.Reports {
			c.Reports = append(c.Reports, AdminPeer{NetworkReport: report, At: x.ReportTimes[id]})
		}
		data.Connections = append(data.Connections, c)
	}
	for _, ch := range data.State.Channels {
		w := s.watch.Status(ch.ID, time.Now())
		if w.Epoch != 0 {
			data.Watches = append(data.Watches, w)
		}
		data.ChannelAdmins[ch.ID] = []string{}
		for _, u := range data.Profiles {
			if s.administration.ChannelRole(ch.ID, u.ClientID) == administration.RoleChannelAdmin {
				data.ChannelAdmins[ch.ID] = append(data.ChannelAdmins[ch.ID], u.ClientID)
			}
		}
	}
	return data, nil
}

func (s *Server) WebAvatar(clientID, reference string) (string, bool) {
	if _, err := s.WebRole(clientID); err != nil {
		return "", false
	}
	return s.administration.AvatarData(reference)
}
