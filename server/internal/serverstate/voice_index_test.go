package serverstate

import (
	"edenvoice/server/internal/config"
	"fmt"
	"net"
	"testing"
)

func TestRoomVoiceIndexLifecycle(t *testing.T) {
	cfg := config.Default()
	cfg.Channels = []config.Channel{{ID: 1, Name: "one"}, {ID: 2, Name: "two"}}
	s := New(cfg)
	a := s.AddSession("a", nil)
	b := s.AddSession("b", nil)
	c := s.AddSession("c", nil)
	for _, id := range []uint64{a.ID, b.ID, c.ID} {
		if err := s.Join(id, 1); err != nil {
			t.Fatal(err)
		}
		if err := s.RegisterUDP(id, &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 9000 + int(id%1000)}); err != nil {
			t.Fatal(err)
		}
	}
	check := func(want int) {
		t.Helper()
		if got := len(s.VoiceTargets(a.ID)); got != want {
			t.Fatalf("recipients %d, want %d", got, want)
		}
	}
	check(2)
	s.SetDeafened(b.ID, true)
	check(1)
	s.SetDeafened(b.ID, false)
	check(2)
	s.SetSubscriptions(b.ID, []uint64{c.ID})
	check(1)
	s.SetSubscriptions(b.ID, []uint64{a.ID})
	check(2)
	if err := s.Join(b.ID, 2); err != nil {
		t.Fatal(err)
	}
	check(1)
	if err := s.Leave(c.ID); err != nil {
		t.Fatal(err)
	}
	check(0)
	s.Join(c.ID, 1)
	check(1)
	s.RemoveSession(c.ID)
	check(0)
	s.Join(b.ID, 1)
	check(1)
	if _, err := s.DeleteChannel(1, 2); err != nil {
		t.Fatal(err)
	}
	check(1)
	if len(s.roomMembers[1]) != 0 || len(s.roomMembers[2]) != 2 {
		t.Fatal("delete channel left stale index")
	}
	s.Leave(a.ID)
	check(0)
}

func TestUDPSourceCheckDoesNotAllocateOrExposeSession(t *testing.T) {
	s := newTestState()
	a := s.AddSession("a", nil)
	addr := &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 9000}
	if s.CheckUDPSource(a.ID, addr) != UDPSourceUnregistered {
		t.Fatal("unregistered")
	}
	s.RegisterUDP(a.ID, addr)
	if n := testing.AllocsPerRun(100, func() {
		if s.CheckUDPSource(a.ID, addr) != UDPSourceValid {
			panic("invalid")
		}
	}); n != 0 {
		t.Fatalf("allocations: %v", n)
	}
	if s.CheckUDPSource(a.ID, nil) != UDPSourceMismatch {
		t.Fatal("nil address")
	}
	s.RemoveSession(a.ID)
	if s.CheckUDPSource(a.ID, addr) != UDPSourceUnknownSession {
		t.Fatal("removed session")
	}
}

func BenchmarkRoomVoiceIndex(b *testing.B) {
	for _, online := range []int{2, 1000} {
		b.Run(fmt.Sprint(online), func(b *testing.B) {
			cfg := config.Default()
			cfg.Channels = []config.Channel{{ID: 1}, {ID: 2}}
			s := New(cfg)
			var sender uint64
			for i := 0; i < online; i++ {
				u := s.AddSession("peer", nil)
				room := uint64(2)
				if i < 2 {
					room = 1
				}
				s.Join(u.ID, room)
				s.RegisterUDP(u.ID, &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 9000 + i})
				if i == 0 {
					sender = u.ID
				}
			}
			b.Run("indexed", func(b *testing.B) {
				b.ReportAllocs()
				for i := 0; i < b.N; i++ {
					s.VoiceTargets(sender)
				}
			})
			b.Run("previous-scan", func(b *testing.B) {
				b.ReportAllocs()
				for i := 0; i < b.N; i++ {
					s.mu.RLock()
					result := make([]VoiceTarget, 0)
					for id, u := range s.sessions {
						if id != sender && u.ChannelID == 1 && u.UDPAddr != nil && !u.Deafened && (len(u.Subscriptions) == 0 || u.Subscriptions[sender]) {
							result = append(result, VoiceTarget{SessionID: id, Address: cloneUDPAddr(u.UDPAddr), MediaOutbound: u.MediaOutbound})
						}
					}
					s.mu.RUnlock()
					if len(result) != 1 {
						b.Fatal("wrong recipients")
					}
				}
			})
		})
	}
}
