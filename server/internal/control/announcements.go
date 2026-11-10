package control

import "edenvoice/server/internal/announcements"

func (s *Server) SetAnnouncements(store *announcements.Store) { s.announcements = store }
func (s *Server) WebAnnouncements(actor, action string, revision uint64, content announcements.Content) (announcements.Document, error) {
	if _, e := s.WebRole(actor); e != nil {
		return announcements.Document{}, e
	}
	if action == "" {
		return s.announcements.Snapshot(), nil
	}
	d, e := s.announcements.Update(action, revision, content)
	if e == nil {
		if action != "save" {
			s.broadcastSnapshot()
		}
		s.WebAudit("announcement "+action, actor, "")
	}
	return d, e
}
