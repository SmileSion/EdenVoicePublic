package main

import (
	"os"
	"strconv"
	"strings"
	"testing"
	"time"
)

// Explicitly opt in only against an empty room configured with a two-session limit.
func TestLiveTwoSessionCapacity(t *testing.T) {
	addr := os.Getenv("EDENVOICE_CAPACITY_CONTROL")
	if addr == "" {
		t.Skip("live capacity check is opt-in")
	}
	channel, err := strconv.ParseUint(os.Getenv("EDENVOICE_CAPACITY_CHANNEL"), 10, 64)
	if err != nil {
		t.Fatal(err)
	}
	opts := options{controlAddr: addr, fingerprint: os.Getenv("EDENVOICE_CAPACITY_FINGERPRINT"), channelID: channel, timeout: 5 * time.Second}
	identity, err := loadIdentity("")
	if err != nil {
		t.Fatal(err)
	}
	clients := make([]*smokeClient, 0, 3)
	for i := 0; i < 3; i++ {
		c, err := connect(opts, identity, "Capacity verification")
		if err != nil {
			t.Fatal(err)
		}
		defer c.close()
		clients = append(clients, c)
	}
	for _, c := range clients[:2] {
		if err := c.join(channel, opts.timeout); err != nil {
			t.Fatal(err)
		}
	}
	if err := clients[2].join(channel, opts.timeout); err == nil || !strings.Contains(err.Error(), "channel user limit reached") {
		t.Fatalf("expected full-room rejection, got %v", err)
	}
	t.Log("first two sessions joined; third rejected by server")
}
