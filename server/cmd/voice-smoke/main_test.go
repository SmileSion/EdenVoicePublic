package main

import (
	"bytes"
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"encoding/base64"
	"testing"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
)

func TestVerifyChallengeBindsFingerprintCertificateAndCapabilities(t *testing.T) {
	serverPublic, _, err := ed25519.GenerateKey(cryptorand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	clientEphemeral, err := security.NewEphemeralKey()
	if err != nil {
		t.Fatal(err)
	}
	defer clientEphemeral.Destroy()
	serverEphemeral, err := security.NewEphemeralKey()
	if err != nil {
		t.Fatal(err)
	}
	defer serverEphemeral.Destroy()
	challengeBytes := bytes.Repeat([]byte{0x5a}, 32)
	fingerprint := (&security.Identity{PublicKey: serverPublic}).Fingerprint()
	challenge := protocol.IdentityChallenge{
		Challenge:            base64.RawStdEncoding.EncodeToString(challengeBytes),
		ServerPublicKey:      base64.RawStdEncoding.EncodeToString(serverPublic),
		ServerFingerprint:    fingerprint,
		ServerEphemeralKey:   base64.RawStdEncoding.EncodeToString(serverEphemeral.PublicKey()),
		RequiredCapabilities: append([]string(nil), requiredCapabilities...),
	}

	publicKey, transcript, err := verifyChallenge(challenge, fingerprint, serverPublic, clientEphemeral.PublicKey())
	if err != nil {
		t.Fatalf("verify valid challenge: %v", err)
	}
	if !bytes.Equal(publicKey, serverPublic) {
		t.Fatal("verified public key differs from challenge identity")
	}
	wantTranscript := security.AuthTranscript(challengeBytes, serverPublic, clientEphemeral.PublicKey(), serverEphemeral.PublicKey())
	if !bytes.Equal(transcript, wantTranscript) {
		t.Fatal("verified transcript differs from the authenticated context")
	}

	t.Run("wrong pinned fingerprint", func(t *testing.T) {
		if _, _, err := verifyChallenge(challenge, "wrong", serverPublic, clientEphemeral.PublicKey()); err == nil {
			t.Fatal("expected fingerprint mismatch")
		}
	})
	t.Run("different TLS certificate identity", func(t *testing.T) {
		otherPublic, _, err := ed25519.GenerateKey(cryptorand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		if _, _, err := verifyChallenge(challenge, fingerprint, otherPublic, clientEphemeral.PublicKey()); err == nil {
			t.Fatal("expected TLS identity mismatch")
		}
	})
	t.Run("missing required capability", func(t *testing.T) {
		changed := challenge
		changed.RequiredCapabilities = changed.RequiredCapabilities[:len(changed.RequiredCapabilities)-1]
		if _, _, err := verifyChallenge(changed, fingerprint, serverPublic, clientEphemeral.PublicKey()); err == nil {
			t.Fatal("expected security capability mismatch")
		}
	})
}
