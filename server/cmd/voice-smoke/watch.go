package main

import (
	"errors"
	"fmt"
	"slices"
	"time"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/watchparty"
)

// Exercises synchronization only; it never sends paths or video contents.
func verifyWatchPair(a, b *smokeClient, timeout time.Duration) error {
	for _, c := range []*smokeClient{a, b} {
		if !slices.Contains(c.welcome.ServerFeatures, "watch_party_v1") {
			return errors.New("server does not advertise watch_party_v1")
		}
	}
	var requestID uint64
	request := func(c *smokeClient, req protocol.WatchRequest) (protocol.WatchState, error) {
		time.Sleep(60 * time.Millisecond) // respect the server's watch command cadence
		requestID++
		req.RequestID, req.ChannelID = requestID, c.channelID
		if err := c.send(protocol.TypeWatchRequest, req); err != nil {
			return protocol.WatchState{}, err
		}
		deadline := time.Now().Add(timeout)
		for time.Now().Before(deadline) {
			msg, err := c.read(time.Until(deadline))
			if err != nil {
				return protocol.WatchState{}, err
			}
			if msg.Type != protocol.TypeWatchState {
				continue
			}
			var state protocol.WatchState
			if err := msg.Decode(&state); err != nil {
				return state, err
			}
			if state.RequestID != req.RequestID {
				continue
			}
			if state.Error != "" {
				return state, errors.New(state.Error)
			}
			if state.ActorID != c.welcome.SessionID {
				continue
			}
			return state, nil
		}
		return protocol.WatchState{}, errors.New("watch response timed out")
	}
	rooms := slices.Contains(a.welcome.ServerFeatures, "watch_rooms_v1")
	firstAction := "join"
	if rooms {
		initial, err := request(a, protocol.WatchRequest{Action: "status"})
		if err != nil {
			return err
		}
		if initial.Epoch != 0 {
			return errors.New("watch test requires an unused watch room")
		}
		firstAction = "open"
	}
	joined, err := request(a, protocol.WatchRequest{Action: firstAction})
	if err != nil {
		return err
	}
	epoch := joined.Epoch
	if rooms {
		visible, err := request(b, protocol.WatchRequest{Action: "status"})
		if err != nil {
			return err
		}
		if visible.Epoch != epoch || visible.HostID != a.welcome.SessionID {
			return errors.New("open room is not discoverable")
		}
	}
	joined, err = request(b, protocol.WatchRequest{Action: "join"})
	if err != nil {
		return err
	}
	if joined.Epoch != epoch || len(joined.Members) != 2 {
		return errors.New("viewers did not share a watch session")
	}
	for _, c := range []*smokeClient{a, b} {
		_, err = request(c, protocol.WatchRequest{Action: "file", Epoch: epoch, File: &watchparty.File{Name: "deployment-check.mp4", Size: 1024, Duration: 120}, Ready: true})
		if err != nil {
			return err
		}
	}
	for _, step := range []struct {
		action   string
		position float64
		paused   bool
	}{{"seek", 42, true}, {"play", 42, false}, {"pause", 43, true}} {
		_, err = request(a, protocol.WatchRequest{Action: step.action, Epoch: epoch, Position: step.position})
		if err != nil {
			return err
		}
		observed, err := request(b, protocol.WatchRequest{Action: "sync", Epoch: epoch})
		if err != nil {
			return err
		}
		if observed.Paused != step.paused || observed.Position < step.position || observed.Position > step.position+timeout.Seconds() {
			return fmt.Errorf("%s was not synchronized: %+v", step.action, observed.Snapshot)
		}
	}
	if slices.Contains(a.welcome.ServerFeatures, "watch_moderation_v1") {
		denied, deniedErr := request(b, protocol.WatchRequest{Action: "play", Epoch: epoch, Position: 60})
		if deniedErr == nil || denied.Error == "" || denied.Permissions == nil || denied.Permissions.CanControl {
			return errors.New("ordinary viewer was not denied global playback")
		}
		transferred, transferErr := request(a, protocol.WatchRequest{Action: "transfer_host", Epoch: epoch, TargetSessionID: b.welcome.SessionID})
		if transferErr != nil || transferred.HostID != b.welcome.SessionID || transferred.Permissions == nil || transferred.Permissions.CanControl {
			return errors.New("explicit transfer did not revoke former host controls")
		}
		if _, err = request(b, protocol.WatchRequest{Action: "pause", Epoch: epoch, Position: 44}); err != nil {
			return err
		}
		if _, err = request(b, protocol.WatchRequest{Action: "transfer_host", Epoch: epoch, TargetSessionID: a.welcome.SessionID}); err != nil {
			return err
		}
	}
	if _, err = request(a, protocol.WatchRequest{Action: "leave", Epoch: epoch}); err != nil {
		return err
	}
	remaining, err := request(b, protocol.WatchRequest{Action: "sync", Epoch: epoch})
	if err != nil {
		return err
	}
	if len(remaining.Members) != 1 {
		return errors.New("watch leave did not remove viewer")
	}
	if rooms {
		if remaining.HostID != b.welcome.SessionID {
			return errors.New("host role did not transfer")
		}
		if _, err = request(b, protocol.WatchRequest{Action: "close", Epoch: epoch}); err != nil {
			return err
		}
		closed, err := request(a, protocol.WatchRequest{Action: "status"})
		if err != nil {
			return err
		}
		if closed.Epoch != 0 {
			return errors.New("closed room still exists")
		}
		reopened, err := request(a, protocol.WatchRequest{Action: "open"})
		if err != nil {
			return err
		}
		if reopened.Epoch == epoch {
			return errors.New("reopen reused prior epoch")
		}
		_, err = request(a, protocol.WatchRequest{Action: "close", Epoch: reopened.Epoch})
		return err
	}
	_, err = request(b, protocol.WatchRequest{Action: "leave", Epoch: epoch})
	return err
}
