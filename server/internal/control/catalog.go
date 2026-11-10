package control

import (
	"edenvoice/server/internal/gamecatalog"
	"edenvoice/server/internal/protocol"
)

func (s *Server) SetGameCatalog(store *gamecatalog.Store) {
	s.catalogMu.Lock()
	defer s.catalogMu.Unlock()
	s.catalog = store
	s.state.ApplyGameCatalog(store.Snapshot().Published)
}
func (s *Server) WebGameCatalog(actor, action string, revision uint64, games []protocol.Game) (gamecatalog.Document, error) {
	if _, err := s.WebRole(actor); err != nil {
		return gamecatalog.Document{}, err
	}
	s.catalogMu.Lock()
	defer s.catalogMu.Unlock()
	if s.catalog == nil {
		s.catalog = gamecatalog.New()
	}
	if action == "" {
		return s.catalog.Snapshot(), nil
	}
	d, e := s.catalog.Update(action, revision, games)
	if e != nil {
		return d, e
	}
	if action != "save" {
		s.state.ApplyGameCatalog(d.Published)
		s.broadcastSnapshot()
	}
	s.WebAudit("game catalog "+action, actor, "")
	return d, nil
}
