package control

import (
	"bufio"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"path/filepath"
	"testing"
	"time"
)

func TestServerPasswordAdmission(t *testing.T) {
	for _, tt := range []struct{ name, password, supplied, code string }{{"open", "", "", ""}, {"correct", "SmileSion", "SmileSion", ""}, {"missing", "SmileSion", "", "server_password_required"}, {"wrong", "SmileSion", "smilesion", "server_password_invalid"}} {
		t.Run(tt.name, func(t *testing.T) {
			identity, _, err := security.LoadOrCreateIdentity(filepath.Join(t.TempDir(), "identity.json"))
			if err != nil {
				t.Fatal(err)
			}
			tlsConfig, err := identity.TLSConfig("test")
			if err != nil {
				t.Fatal(err)
			}
			s := NewSecure("", "127.0.0.1:9001", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)), identity, tlsConfig, false, nil)
			s.SetServerPassword(tt.password)
			a, b := net.Pipe()
			go s.handle(tls.Server(a, tlsConfig))
			conn := tls.Client(b, &tls.Config{InsecureSkipVerify: true, MinVersion: tls.VersionTLS13})
			defer conn.Close()
			conn.SetDeadline(time.Now().Add(5 * time.Second))
			enc, dec := json.NewEncoder(conn), json.NewDecoder(bufio.NewReader(conn))
			pub, priv, _ := ed25519.GenerateKey(rand.Reader)
			ephemeral, _ := ecdh.X25519().GenerateKey(rand.Reader)
			hello, _ := protocol.NewEnvelope(protocol.TypeHello, protocol.Hello{Nickname: "guest", ProtocolVersion: security.ProtocolVersion, SecurityCapabilities: requiredSecurityCapabilities(), ClientID: security.ClientID(pub), ClientPublicKey: base64.RawStdEncoding.EncodeToString(pub), ClientEphemeralKey: base64.RawStdEncoding.EncodeToString(ephemeral.PublicKey().Bytes())})
			if err = enc.Encode(hello); err != nil {
				t.Fatal(err)
			}
			var envelope protocol.Envelope
			if err = dec.Decode(&envelope); err != nil {
				t.Fatal(err)
			}
			if envelope.Type != protocol.TypeIdentityChallenge {
				t.Fatalf("unexpected pre-auth data: %s", envelope.Type)
			}
			var challenge protocol.IdentityChallenge
			envelope.Decode(&challenge)
			if challenge.ServerPasswordRequired != (tt.password != "") {
				t.Fatal("incorrect password requirement")
			}
			nonce, _ := base64.RawStdEncoding.DecodeString(challenge.Challenge)
			serverPub, _ := base64.RawStdEncoding.DecodeString(challenge.ServerPublicKey)
			serverEphemeral, _ := base64.RawStdEncoding.DecodeString(challenge.ServerEphemeralKey)
			transcript := security.AuthTranscript(nonce, serverPub, ephemeral.PublicKey().Bytes(), serverEphemeral)
			response, _ := protocol.NewEnvelope(protocol.TypeIdentityResponse, protocol.IdentityResponse{Signature: base64.RawStdEncoding.EncodeToString(ed25519.Sign(priv, transcript)), ServerPassword: tt.supplied})
			enc.Encode(response)
			if err = dec.Decode(&envelope); err != nil {
				t.Fatal(err)
			}
			if tt.code == "" {
				if envelope.Type != protocol.TypeWelcome {
					t.Fatalf("expected welcome, got %s", envelope.Type)
				}
			} else {
				var e protocol.Error
				if envelope.Type != protocol.TypeError {
					t.Fatalf("unauthorized data: %s", envelope.Type)
				}
				envelope.Decode(&e)
				if e.Code != tt.code {
					t.Fatalf("got %s want %s", e.Code, tt.code)
				}
			}
		})
	}
}
func TestServerPasswordLimits(t *testing.T) {
	if !errors.Is(checkServerPassword("secret", "wrong"), errServerPasswordInvalid) {
		t.Fatal("accepted wrong password")
	}
	l := passwordAttemptLimiter{}
	now := time.Now()
	for i := 0; i < 12; i++ {
		if !l.allow("127.0.0.1:10", now) {
			t.Fatal("early limit")
		}
	}
	if l.allow("127.0.0.1:11", now) {
		t.Fatal("port bypass")
	}
	if !l.allow("127.0.0.1:11", now.Add(time.Minute)) {
		t.Fatal("did not expire")
	}
}
