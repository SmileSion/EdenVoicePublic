package control

import "edenvoice/server/internal/clientupdates"

func (s *Server) SetClientUpdates(store *clientupdates.Store) { s.updates = store }
func (s *Server) WebClientUpdates(actor, action string, revision uint64, r clientupdates.Release) (clientupdates.Document, error) {
	if _, e := s.WebRole(actor); e != nil {
		return clientupdates.Document{}, e
	}
	if action == "" {
		return s.updates.Snapshot(), nil
	}
	d, e := s.updates.Update(action, revision, r)
	if e == nil {
		if action != "save" {
			s.broadcastSnapshot()
		}
		s.WebAudit("client update "+action, actor, r.Platform+" "+r.Version)
	}
	return d, e
}
