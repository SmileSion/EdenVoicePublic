package control

import (
	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"errors"
	"strings"
	"time"
)

// Both authenticated voice clients and the web console use this implementation.
func (s *Server) dispatchAdministration(id uint64, actorClientID string, envelope protocol.Envelope, sender serverstate.Sender) error {
	switch envelope.Type {
	case protocol.TypeSetRoomNetworkPolicy:
		var request protocol.RoomNetworkPolicyRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		channelID := request.ChannelID
		if channelID == 0 {
			channelID = s.state.ChannelOf(id)
		}
		if s.administration != nil {
			if err := s.administration.Authorize(actorClientID, administration.ActionNetworkPolicySet, administration.Resource{ChannelID: channelID}); err != nil {
				return err
			}
		}
		previousPolicy := s.state.RoomNetworkPolicy(channelID)
		if err := s.state.SetRoomNetworkPolicyForChannel(channelID, request.Policy); err != nil {
			return err
		}
		if s.administration != nil {
			if err := s.administration.SetChannelNetworkPolicy(channelID, request.Policy); err != nil {
				_ = s.state.SetRoomNetworkPolicyForChannel(channelID, previousPolicy)
				return err
			}
		}
		s.audit("room network policy changed", actorClientID, "", channelID, "policy", request.Policy)
		s.broadcastSnapshot()
		s.broadcastRoute(channelID)
		s.broadcastRoomCandidates(channelID)
	case protocol.TypeCreateChannel:
		var request protocol.ChannelMutationRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionChannelCreate, administration.Resource{ChannelID: request.Channel.ParentID}); err != nil {
			return err
		}
		request.Channel.Name = strings.TrimSpace(request.Channel.Name)
		if id == 0 && request.Channel.Temporary {
			return errors.New("temporary rooms must be created by a voice participant")
		}
		if err := validateNewChannel(request.Channel); err != nil {
			return err
		}
		channel, err := s.administration.CreateChannelWithPassword(request.Channel, request.Password)
		if err != nil {
			return err
		}
		if err := s.state.CreateChannel(channel); err != nil {
			return err
		}
		if s.archive != nil {
			s.archive.Observe(archive.Activity{Kind: archive.Created, RoomID: channel.ID, RoomName: channel.Name, At: time.Now()})
		}
		s.audit("channel created", actorClientID, "", channel.ID)
		if channel.Temporary {
			oldChannel := s.state.ChannelOf(id)
			if err := s.state.Join(id, channel.ID); err != nil {
				return err
			}
			s.observeArchiveMove(actorClientID, oldChannel, channel.ID)
			s.broadcastMove(id, channel.ID)
			s.broadcastRoute(oldChannel)
			s.broadcastRoomCandidates(oldChannel)
			s.broadcastRoute(channel.ID)
			s.sendPeerCandidates(id)
			s.broadcastCandidate(id)
			s.sendCapabilities(id, sender, channel.ID)
			s.send(sender, protocol.TypeChatHistory, protocol.ChatHistory{ChannelID: channel.ID, Messages: []protocol.TextMessage{}})
			s.deleteEmptyTemporaryChannel(oldChannel)
		}
		s.broadcastSnapshot()
		s.broadcastCapabilities()
	case protocol.TypeEditChannel:
		var request protocol.ChannelMutationRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionChannelEdit, administration.Resource{ChannelID: request.Channel.ID}); err != nil {
			return err
		}
		request.Channel.Name = strings.TrimSpace(request.Channel.Name)
		if err := validateChannel(request.Channel); err != nil {
			return err
		}
		if err := s.administration.EditChannel(request.Channel); err != nil {
			return err
		}
		if err := s.state.EditChannel(request.Channel); err != nil {
			return err
		}
		s.audit("channel edited", actorClientID, "", request.Channel.ID)
		s.broadcastSnapshot()
		s.deleteEmptyTemporaryChannel(request.Channel.ID)
	case protocol.TypeDeleteChannel:
		var request protocol.ChannelRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionChannelDelete, administration.Resource{ChannelID: request.ChannelID}); err != nil {
			return err
		}
		fallback := s.fallbackChannel(request.ChannelID)
		if fallback == 0 {
			return errors.New("no safe fallback channel")
		}
		if err := s.administration.DeleteChannel(request.ChannelID); err != nil {
			return err
		}
		moved, err := s.state.DeleteChannel(request.ChannelID, fallback)
		if err != nil {
			return err
		}
		for _, sessionID := range moved {
			s.broadcastMove(sessionID, fallback)
			s.sendPeerCandidates(sessionID)
			if movedSession, ok := s.state.SessionByID(sessionID); ok {
				s.observeArchiveMove(movedSession.ClientID, request.ChannelID, fallback)
				s.sendCapabilities(sessionID, movedSession.Sender, fallback)
			}
		}
		s.audit("channel deleted", actorClientID, "", request.ChannelID)
		s.broadcastSnapshot()
		s.broadcastCapabilities()
		s.broadcastRoute(fallback)
		s.broadcastRoomCandidates(fallback)
	case protocol.TypeSetChannelAdmin:
		var request protocol.ChannelRoleRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionChannelRoleSet, administration.Resource{ChannelID: request.ChannelID, TargetClientID: request.ClientID}); err != nil {
			return err
		}
		role := administration.ChannelRole("")
		if request.Enabled {
			role = administration.RoleChannelAdmin
		}
		if err := s.administration.SetChannelRole(request.ChannelID, request.ClientID, role); err != nil {
			return err
		}
		s.audit("channel role changed", actorClientID, request.ClientID, request.ChannelID)
		for _, target := range s.state.SessionsByClientID(request.ClientID) {
			s.sendCapabilities(target.ID, target.Sender, target.ChannelID)
		}
		s.broadcastSnapshot()
	case protocol.TypeSetServerRole:
		var request protocol.ServerRoleRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionServerRoleSet, administration.Resource{TargetClientID: request.ClientID}); err != nil {
			return err
		}
		if err := s.administration.SetServerRole(request.ClientID, administration.ServerRole(request.Role)); err != nil {
			return err
		}
		s.state.UpdateServerRole(request.ClientID, request.Role)
		for _, target := range s.state.SessionsByClientID(request.ClientID) {
			s.sendCapabilities(target.ID, target.Sender, target.ChannelID)
		}
		s.audit("server role changed", actorClientID, request.ClientID, 0)
		s.broadcastSnapshot()
	case protocol.TypeAdminMute:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		target, err := s.resolveTarget(request)
		if err != nil {
			return err
		}
		resource := administration.Resource{ChannelID: target.ChannelID, TargetClientID: request.ClientID}
		scope, err := s.administration.AuthorizeMute(actorClientID, resource)
		if err != nil {
			return err
		}
		if scope == administration.MuteScopeServer {
			if err := s.state.SetAdminMuted(target.ID, request.Enabled); err != nil {
				return err
			}
			s.audit("server mute changed", actorClientID, request.ClientID, target.ChannelID)
		} else {
			if err := s.state.SetChannelMuted(target.ID, target.ChannelID, request.Enabled); err != nil {
				return err
			}
			s.audit("channel mute changed", actorClientID, request.ClientID, target.ChannelID)
		}
		s.broadcastState(target.ID)
	case protocol.TypeMoveUser:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		target, err := s.resolveTarget(request)
		if err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberMove, administration.Resource{ChannelID: target.ChannelID, TargetClientID: request.ClientID}); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberMove, administration.Resource{ChannelID: request.ChannelID, TargetClientID: request.ClientID}); err != nil {
			return err
		}
		oldChannel := target.ChannelID
		if err := s.state.Move(target.ID, request.ChannelID); err != nil {
			return err
		}
		s.observeArchiveMove(target.ClientID, oldChannel, request.ChannelID)
		s.broadcastMove(target.ID, request.ChannelID)
		s.broadcastState(target.ID)
		s.broadcastRoute(oldChannel)
		s.broadcastRoomCandidates(oldChannel)
		s.broadcastRoute(request.ChannelID)
		s.sendPeerCandidates(target.ID)
		s.broadcastCandidate(target.ID)
		s.sendCapabilities(target.ID, target.Sender, request.ChannelID)
		s.audit("user moved", actorClientID, request.ClientID, request.ChannelID)
		s.deleteEmptyTemporaryChannel(oldChannel)
	case protocol.TypeKickChannel:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if request.ClientID == actorClientID {
			return administration.ErrForbidden
		}
		target, err := s.resolveTarget(request)
		if err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberKickChannel, administration.Resource{ChannelID: target.ChannelID, TargetClientID: request.ClientID}); err != nil {
			return err
		}
		oldChannel := target.ChannelID
		if err := s.state.Leave(target.ID); err != nil {
			return err
		}
		s.observeArchiveMove(target.ClientID, oldChannel, 0)
		s.broadcastMove(target.ID, 0)
		s.broadcastRoute(oldChannel)
		s.sendCapabilities(target.ID, target.Sender, 0)
		s.audit("user kicked from channel", actorClientID, request.ClientID, oldChannel)
		s.deleteEmptyTemporaryChannel(oldChannel)
	case protocol.TypeKickServer:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if request.ClientID == actorClientID {
			return administration.ErrForbidden
		}
		target, err := s.resolveTarget(request)
		if err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberKickServer, administration.Resource{TargetClientID: request.ClientID}); err != nil {
			return err
		}
		s.audit("user kicked from server", actorClientID, request.ClientID, target.ChannelID)
		s.disconnectWithError(target.ID, target.Sender, "kicked_from_server", "kicked by administrator")
	case protocol.TypeBan:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if request.ClientID == actorClientID {
			return administration.ErrForbidden
		}
		if err := s.authorize(actorClientID, administration.ActionMemberBan, administration.Resource{TargetClientID: request.ClientID}); err != nil {
			return err
		}
		if err := s.administration.Ban(request.ClientID, request.Reason, actorClientID, request.ExpiresAt); err != nil {
			return err
		}
		s.audit("identity banned", actorClientID, request.ClientID, 0)
		for _, target := range s.state.SessionsByClientID(request.ClientID) {
			s.disconnectWithError(target.ID, target.Sender, "banned", "this identity is banned")
		}
	case protocol.TypeUnban:
		var request protocol.TargetRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberUnban, administration.Resource{TargetClientID: request.ClientID}); err != nil {
			return err
		}
		if err := s.administration.Unban(request.ClientID); err != nil {
			return err
		}
		s.audit("identity unbanned", actorClientID, request.ClientID, 0)
	case protocol.TypeListBans:
		var request protocol.BanListRequest
		if err := envelope.Decode(&request); err != nil {
			return err
		}
		if err := s.authorize(actorClientID, administration.ActionMemberBanList, administration.Resource{}); err != nil {
			return err
		}
		if request.Offset < 0 {
			return errors.New("invalid ban list offset")
		}
		if request.Limit <= 0 || request.Limit > 50 {
			request.Limit = 8
		}
		s.sendBanList(sender, request.Offset, request.Limit)
	default:
		return errors.New("unsupported administrative action")
	}
	return nil
}
