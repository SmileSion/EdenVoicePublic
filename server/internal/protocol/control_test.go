package protocol

import (
	"encoding/json"
	"slices"
	"testing"
)

func TestControlEnvelopeUsesFlatNDJSONFields(t *testing.T) {
	message, err := NewEnvelope(TypeHello, Hello{Nickname: "Sion"})
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := json.Marshal(message)
	if err != nil {
		t.Fatal(err)
	}
	if string(encoded) != `{"nickname":"Sion","type":"hello"}` {
		t.Fatalf("unexpected control wire format: %s", encoded)
	}
	var decoded Envelope
	if err := json.Unmarshal(encoded, &decoded); err != nil {
		t.Fatal(err)
	}
	var hello Hello
	if err := decoded.Decode(&hello); err != nil {
		t.Fatal(err)
	}
	if hello.Nickname != "Sion" {
		t.Fatalf("unexpected nickname %q", hello.Nickname)
	}
}

func TestControlEnvelopeRejectsMissingType(t *testing.T) {
	var message Envelope
	if err := json.Unmarshal([]byte(`{"nickname":"Sion"}`), &message); err == nil {
		t.Fatal("missing type was accepted")
	}
}

func TestAudioQoEFeedbackPreservesUnknownPathMetrics(t *testing.T) {
	for _, wire := range []string{
		`{"type":"audio_qoe_feedback","peer_id":7,"path_metrics_available":false}`,
		`{"type":"audio_qoe_feedback","peer_id":7}`,
	} {
		var incoming Envelope
		if err := json.Unmarshal([]byte(wire), &incoming); err != nil {
			t.Fatal(err)
		}
		var feedback AudioQoEFeedback
		if err := incoming.Decode(&feedback); err != nil {
			t.Fatal(err)
		}
		forwarded, err := NewEnvelope(incoming.Type, feedback)
		if err != nil {
			t.Fatal(err)
		}
		encoded, err := json.Marshal(forwarded)
		if err != nil {
			t.Fatal(err)
		}
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(encoded, &fields); err != nil {
			t.Fatal(err)
		}
		if feedback.PathMetricsAvailable == nil {
			if _, exists := fields["path_metrics_available"]; exists {
				t.Fatal("legacy feedback acquired invented probe availability")
			}
		} else if string(fields["path_metrics_available"]) != "false" {
			t.Fatal("explicit unavailable metrics lost during forwarding")
		}
	}
}

func TestCapabilitiesCarryBackwardCompatibleNetworkPolicyScope(t *testing.T) {
	message, err := NewEnvelope(TypeCapabilities, Capabilities{
		ServerRole:              "USER",
		Actions:                 []string{"network.policy.set"},
		NetworkPolicyChannelIDs: []uint64{7, 9},
	})
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := json.Marshal(message)
	if err != nil {
		t.Fatal(err)
	}
	var wire map[string]json.RawMessage
	if err := json.Unmarshal(encoded, &wire); err != nil {
		t.Fatal(err)
	}
	if _, ok := wire["network_policy_channel_ids"]; !ok {
		t.Fatalf("network-policy channel scope missing from wire message: %s", encoded)
	}

	var current Envelope
	if err := json.Unmarshal(encoded, &current); err != nil {
		t.Fatal(err)
	}
	var capabilities Capabilities
	if err := current.Decode(&capabilities); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(capabilities.NetworkPolicyChannelIDs, []uint64{7, 9}) {
		t.Fatalf("network-policy channel scope = %v, want [7 9]", capabilities.NetworkPolicyChannelIDs)
	}

	var legacy Envelope
	if err := json.Unmarshal([]byte(`{"type":"capabilities","server_role":"ADMIN","actions":["network.policy.set"]}`), &legacy); err != nil {
		t.Fatal(err)
	}
	capabilities = Capabilities{}
	if err := legacy.Decode(&capabilities); err != nil {
		t.Fatal(err)
	}
	if len(capabilities.NetworkPolicyChannelIDs) != 0 {
		t.Fatalf("legacy capability message produced scope %v", capabilities.NetworkPolicyChannelIDs)
	}
}

func TestChannelTracksWhetherTemporaryWasExplicitlyProvided(t *testing.T) {
	var legacy Channel
	if err := json.Unmarshal([]byte(`{"id":1,"name":"Legacy","sort_order":0}`), &legacy); err != nil {
		t.Fatal(err)
	}
	if legacy.Temporary || legacy.TemporarySpecified {
		t.Fatalf("legacy channel unexpectedly specified temporary state: %+v", legacy)
	}

	var permanent Channel
	if err := json.Unmarshal([]byte(`{"id":1,"name":"Permanent","sort_order":0,"temporary":false}`), &permanent); err != nil {
		t.Fatal(err)
	}
	if permanent.Temporary || !permanent.TemporarySpecified {
		t.Fatalf("explicit permanent channel lost field presence: %+v", permanent)
	}

	var temporary Channel
	if err := json.Unmarshal([]byte(`{"id":1,"name":"Temporary","sort_order":0,"temporary":true}`), &temporary); err != nil {
		t.Fatal(err)
	}
	if !temporary.Temporary || !temporary.TemporarySpecified {
		t.Fatalf("explicit temporary channel lost field presence: %+v", temporary)
	}
}
