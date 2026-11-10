package main

import (
	"context"
	"testing"
	"time"
)

func TestShortEncryptedMultiRoomLoad(t *testing.T) {
	opts := options{
		rooms: 2, usersPerRoom: 3, talkersPerRoom: 2,
		duration: 300 * time.Millisecond, reportInterval: time.Second,
		lossPercent: 1, reorderPercent: 5, maxDelay: 10 * time.Millisecond,
		burstEvery: time.Second, burstDuration: 10 * time.Millisecond,
		blackoutEvery: time.Second, blackoutDuration: 10 * time.Millisecond,
		payloadBytes: 80,
	}
	if err := run(context.Background(), opts); err != nil {
		t.Fatal(err)
	}
}

func TestRejectsInvalidTopology(t *testing.T) {
	if err := validate(options{rooms: 1, usersPerRoom: 2, talkersPerRoom: 3, duration: time.Second, reportInterval: time.Second, payloadBytes: 80}); err == nil {
		t.Fatal("invalid talker count should be rejected")
	}
}
