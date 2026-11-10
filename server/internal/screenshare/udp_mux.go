package screenshare

import (
	"errors"
	"fmt"
	"net"
	"syscall"

	"github.com/pion/ice/v4"
)

// familyUDPMux limits the candidates to addresses reachable through its socket.
// Pion's wildcard IPv6 discovery also lists IPv4 addresses, even though an
// explicit udp6 socket is IPv6-only. Its own demultiplexer still owns all reads,
// writes and the per-ICE-ufrag connection lifetime.
type familyUDPMux struct {
	ice.UDPMux
	addresses  []net.Addr
	socketAddr net.Addr
}

func (m *familyUDPMux) GetListenAddresses() []net.Addr { return m.addresses }

func (m *familyUDPMux) GetConn(ufrag string, _ net.Addr) (net.PacketConn, error) {
	// All advertised addresses for this family resolve to this wildcard socket.
	// Its address also selects the correct IPv4/IPv6 ufrag map inside Pion.
	return m.UDPMux.GetConn(ufrag, m.socketAddr)
}

// Use one wildcard socket per address family, on the same fixed port. Per-IP
// sockets would be incorrect behind 1:1 NAT on multi-interface hosts: Pion
// deduplicates identical public-IP:port candidates before registering their
// ufrag, so an inbound packet on another interface's mux could be discarded.
func listenFixedUDPMux(cfg Config) (ice.UDPMux, error) {
	publicIP := net.ParseIP(cfg.PublicIP)
	networks := []string{"udp4", "udp6"}
	if publicIP != nil {
		if publicIP.To4() != nil {
			networks = []string{"udp4"}
		} else {
			networks = []string{"udp6"}
		}
	}
	var muxes []ice.UDPMux
	cleanup := func() {
		for _, m := range muxes {
			_ = m.Close()
		}
	}
	for _, network := range networks {
		conn, err := net.ListenUDP(network, &net.UDPAddr{Port: int(cfg.UDPPort)})
		if err != nil {
			// IPv6 may be disabled at OS level. With no IPv6 public mapping, IPv4
			// service can still run; a port conflict is never silently ignored.
			if network == "udp6" && publicIP == nil && ipv6Unavailable(err) {
				continue
			}
			cleanup()
			return nil, fmt.Errorf("bind screen sharing %s port %d: %w", network, cfg.UDPPort, err)
		}
		_ = conn.SetReadBuffer(1 << 20)
		_ = conn.SetWriteBuffer(1 << 20)
		// Pion supports wildcard sockets and enumerates their local addresses.
		// Its generic multi-interface warning does not account for the NAT case
		// above: sharing the same ufrag table across ingress interfaces is required.
		underlying := ice.NewUDPMuxDefault(ice.UDPMuxParams{UDPConn: conn})
		family := &familyUDPMux{UDPMux: underlying, socketAddr: conn.LocalAddr()}
		if publicIP != nil {
			// Advertise the NAT mapping without choosing one ingress interface.
			family.addresses = []net.Addr{&net.UDPAddr{IP: publicIP, Port: int(cfg.UDPPort)}}
		} else {
			for _, addr := range underlying.GetListenAddresses() {
				udp, ok := addr.(*net.UDPAddr)
				if !ok || (udp.IP.To4() == nil) != (network == "udp6") || udp.IP.IsUnspecified() || udp.IP.IsLinkLocalUnicast() || udp.IP.IsLinkLocalMulticast() {
					continue
				}
				// Avoid advertising a remote user's localhost.
				if udp.IP.IsLoopback() {
					continue
				}
				family.addresses = append(family.addresses, addr)
			}
		}
		if len(family.addresses) == 0 {
			_ = family.Close()
			continue
		}
		muxes = append(muxes, family)
	}
	if len(muxes) == 0 {
		return nil, errors.New("screen sharing has no usable network interface for the configured address family")
	}
	return ice.NewMultiUDPMuxDefault(muxes...), nil
}

func ipv6Unavailable(err error) bool {
	return errors.Is(err, syscall.EAFNOSUPPORT) || errors.Is(err, syscall.EPROTONOSUPPORT) || errors.Is(err, syscall.EADDRNOTAVAIL)
}
