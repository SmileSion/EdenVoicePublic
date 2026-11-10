package security

import (
	"bytes"
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestIdentityPersistsAndClientIDDoesNotDependOnDisplayName(t *testing.T) {
	path := filepath.Join(t.TempDir(), "identity.json")
	first, created, err := LoadOrCreateIdentity(path)
	if err != nil || !created {
		t.Fatalf("create identity: created=%v err=%v", created, err)
	}
	if err := os.Chmod(path, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	second, created, err := LoadOrCreateIdentity(path)
	if err != nil || created {
		t.Fatalf("reload identity: created=%v err=%v", created, err)
	}
	if ClientID(first.PublicKey) != ClientID(second.PublicKey) || first.Fingerprint() != second.Fingerprint() {
		t.Fatal("persistent identity changed")
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("identity permissions were not repaired: info=%v err=%v", info, err)
	}
	directoryInfo, err := os.Stat(filepath.Dir(path))
	if err != nil || directoryInfo.Mode().Perm() != 0o700 {
		t.Fatalf("identity directory permissions were not repaired: info=%v err=%v", directoryInfo, err)
	}
}

func TestIdentityPrivateKeyJSONRoundTripsWithoutImmutableSecretStrings(t *testing.T) {
	privateKey := secretBytes{1, 2, 3, 4, 5}
	encoded := encodeIdentityFile(identityFile{Version: identityFileVersion, PublicKey: "public", PrivateKey: privateKey})
	var decoded identityFile
	if err := json.Unmarshal(encoded, &decoded); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(decoded.PrivateKey, privateKey) {
		t.Fatalf("private key encoding changed: %v", decoded.PrivateKey)
	}
	clear(encoded)
	clear(decoded.PrivateKey)
	if !allZero(encoded) || !allZero(decoded.PrivateKey) {
		t.Fatal("identity JSON buffers could not be explicitly cleared")
	}
}

func allZero(value []byte) bool {
	for _, item := range value {
		if item != 0 {
			return false
		}
	}
	return true
}

func TestIdentityChallengeRejectsForgedIDAndReplayedContext(t *testing.T) {
	publicKey, privateKey, _ := ed25519.GenerateKey(cryptorand.Reader)
	serverIdentity := make([]byte, ed25519.PublicKeySize)
	clientEphemeral := make([]byte, 32)
	serverEphemeral := make([]byte, 32)
	challenge, _ := NewChallenge()
	transcript := AuthTranscript(challenge, serverIdentity, clientEphemeral, serverEphemeral)
	signature := ed25519.Sign(privateKey, transcript)
	if err := VerifyClient(publicKey, ClientID(publicKey), transcript, signature); err != nil {
		t.Fatalf("valid identity rejected: %v", err)
	}
	if err := VerifyClient(publicKey, "forged", transcript, signature); err == nil {
		t.Fatal("forged client ID accepted")
	}
	otherChallenge, _ := NewChallenge()
	if err := VerifyClient(publicKey, ClientID(publicKey), AuthTranscript(otherChallenge, serverIdentity, clientEphemeral, serverEphemeral), signature); err == nil {
		t.Fatal("signature replayed into another challenge")
	}
}

func TestX25519DerivationMatchesAndReconnectChangesSecrets(t *testing.T) {
	client, _ := NewEphemeralKey()
	server, _ := NewEphemeralKey()
	defer client.Destroy()
	defer server.Destroy()
	transcript := []byte("session one")
	clientSecrets, err := DeriveSessionSecrets(client, server.PublicKey(), transcript)
	if err != nil {
		t.Fatal(err)
	}
	serverSecrets, err := DeriveSessionSecrets(server, client.PublicKey(), transcript)
	if err != nil {
		t.Fatal(err)
	}
	if clientSecrets != serverSecrets {
		t.Fatal("session peers derived different secrets")
	}
	reconnected, _ := DeriveSessionSecrets(server, client.PublicKey(), []byte("session two"))
	if reconnected == serverSecrets {
		t.Fatal("reconnect reused session secrets")
	}
}

func TestEphemeralKeyDestroyClearsPrivateMaterialAndPreventsReuse(t *testing.T) {
	key, err := NewEphemeralKey()
	if err != nil {
		t.Fatal(err)
	}
	peer, err := NewEphemeralKey()
	if err != nil {
		t.Fatal(err)
	}
	defer peer.Destroy()
	peerPublic := peer.PublicKey()
	key.Destroy()
	key.Destroy()
	if key.private != [32]byte{} || key.public != [32]byte{} || !key.destroyed {
		t.Fatal("ephemeral key retained material after destruction")
	}
	if key.PublicKey() != nil {
		t.Fatal("destroyed ephemeral key exposed a public key")
	}
	if _, err := DeriveSessionSecrets(key, peerPublic, []byte("destroyed")); err == nil {
		t.Fatal("destroyed ephemeral key derived session secrets")
	}
}

func TestSessionSecretsDestroyClearsEveryDirection(t *testing.T) {
	secrets := SessionSecrets{
		ClientToServerKey: [32]byte{1},
		ServerToClientKey: [32]byte{2},
		ClientNoncePrefix: [6]byte{3},
		ServerNoncePrefix: [6]byte{4},
	}
	secrets.Destroy()
	if secrets.ClientToServerKey != [32]byte{} || secrets.ServerToClientKey != [32]byte{} || secrets.ClientNoncePrefix != [6]byte{} || secrets.ServerNoncePrefix != [6]byte{} {
		t.Fatal("session secrets retained key material after destruction")
	}
}

func TestMediaCipherAuthenticatesTamperReplayReorderAndEpoch(t *testing.T) {
	var key [32]byte
	var prefix [6]byte
	_, _ = cryptorand.Read(key[:])
	_, _ = cryptorand.Read(prefix[:])
	sender, _ := NewMediaCipher(key, prefix, 1)
	receiver, _ := NewMediaCipher(key, prefix, 1)
	aad := []byte("authenticated header")
	sealed10, epoch, _ := sender.Seal(aad, 42, 10, []byte("secret opus"))
	sealed12, _, _ := sender.Seal(aad, 42, 12, []byte("later"))
	sealed11, _, _ := sender.Seal(aad, 42, 11, []byte("reordered"))
	if got, err := receiver.Open(aad, 42, epoch, 10, sealed10); err != nil || string(got) != "secret opus" {
		t.Fatalf("open: %q %v", got, err)
	}
	if _, err := receiver.Open(aad, 42, epoch, 10, sealed10); err == nil {
		t.Fatal("replay accepted")
	}
	if _, err := receiver.Open(aad, 42, epoch, 12, sealed12); err != nil {
		t.Fatal("new packet rejected")
	}
	if _, err := receiver.Open(aad, 42, epoch, 11, sealed11); err != nil {
		t.Fatal("valid reorder rejected")
	}
	tampered := append([]byte(nil), sealed10...)
	tampered[0] ^= 1
	fresh, _ := NewMediaCipher(key, prefix, 1)
	if _, err := fresh.Open(aad, 42, epoch, 10, tampered); err == nil {
		t.Fatal("ciphertext tamper accepted")
	}
	if _, err := fresh.Open([]byte("tampered header"), 42, epoch, 10, sealed10); err == nil {
		t.Fatal("header tamper accepted")
	}
	if _, err := fresh.Open(aad, 42, 2, 10, sealed10); err == nil {
		t.Fatal("wrong epoch accepted")
	}
}

func TestMediaCipherHidesPlaintextAndRejectsWrongKey(t *testing.T) {
	key := [32]byte{1}
	prefix := [6]byte{2}
	sender, _ := NewMediaCipher(key, prefix, 1)
	plaintext := []byte("recognizable opus payload that must stay encrypted")
	ciphertext, epoch, err := sender.Seal([]byte("authenticated header"), 42, 1, plaintext)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Equal(ciphertext, plaintext) || bytes.Contains(ciphertext, plaintext) {
		t.Fatal("encrypted media exposed its plaintext payload")
	}

	wrongKey := [32]byte{9}
	receiver, _ := NewMediaCipher(wrongKey, prefix, 1)
	if _, err := receiver.Open([]byte("authenticated header"), 42, epoch, 1, ciphertext); err == nil {
		t.Fatal("media encrypted under a different key was accepted")
	}
}

func TestRekeyAcceptsPreviousUntilGraceEnds(t *testing.T) {
	var oldKey, newKey [32]byte
	var oldPrefix, newPrefix [6]byte
	_, _ = cryptorand.Read(oldKey[:])
	_, _ = cryptorand.Read(newKey[:])
	oldSender, _ := NewMediaCipher(oldKey, oldPrefix, 1)
	receiver, _ := NewMediaCipher(oldKey, oldPrefix, 1)
	oldPacket, _, _ := oldSender.Seal(nil, 42, 1, []byte("in flight"))
	if err := receiver.Rekey(newKey, newPrefix, 2); err != nil {
		t.Fatal(err)
	}
	if _, err := receiver.Open(nil, 42, 1, 1, oldPacket); err != nil {
		t.Fatalf("previous epoch rejected during grace: %v", err)
	}
	receiver.DropPrevious()
	if _, err := receiver.Open(nil, 42, 1, 2, oldPacket); err == nil {
		t.Fatal("previous epoch accepted after grace")
	}
}

func TestOldGraceTimerCannotDropANewerPreviousEpoch(t *testing.T) {
	var key [32]byte
	var prefix [6]byte
	cipher, _ := NewMediaCipher(key, prefix, 1)
	if err := cipher.PrepareRekey([]byte("epoch two"), 2); err != nil {
		t.Fatal(err)
	}
	pending := cipher.pending
	if err := cipher.ActivatePrepared(2); err != nil {
		t.Fatal(err)
	}
	if pending.epoch != 0 || pending.rootKey != [32]byte{} || pending.noncePrefix != [6]byte{} || pending.streams != nil || pending.replay != nil {
		t.Fatal("activated pending epoch retained a copied secret buffer")
	}
	if err := cipher.PrepareRekey([]byte("epoch three"), 3); err != nil {
		t.Fatal(err)
	}
	if err := cipher.ActivatePrepared(3); err != nil {
		t.Fatal(err)
	}

	cipher.DropPreviousIfCurrent(2)
	if cipher.previous == nil || cipher.previous.epoch != 2 {
		t.Fatal("stale grace timer dropped the newer grace epoch")
	}
	cipher.DropPreviousIfCurrent(3)
	if cipher.previous != nil {
		t.Fatal("current grace timer did not destroy the previous epoch")
	}
}

func TestMediaCipherDestroyZeroizesAndRejectsFurtherUse(t *testing.T) {
	key := [32]byte{1}
	prefix := [6]byte{2}
	cipher, _ := NewMediaCipher(key, prefix, 1)
	if _, _, err := cipher.Seal(nil, 42, 1, []byte("populate stream cache")); err != nil {
		t.Fatal(err)
	}
	if err := cipher.PrepareRekey([]byte("epoch two"), 2); err != nil {
		t.Fatal(err)
	}
	if err := cipher.ActivatePrepared(2); err != nil {
		t.Fatal(err)
	}
	if err := cipher.PrepareRekey([]byte("epoch three"), 3); err != nil {
		t.Fatal(err)
	}

	cipher.Destroy()
	cipher.Destroy()

	if !cipher.destroyed || cipher.current.epoch != 0 || cipher.previous != nil || cipher.pending != nil {
		t.Fatal("cipher retained state after destruction")
	}
	if cipher.current.rootKey != [32]byte{} || cipher.current.noncePrefix != [6]byte{} || len(cipher.current.streams) != 0 || len(cipher.current.replay) != 0 {
		t.Fatal("current epoch secrets were not zeroized")
	}
	if cipher.Epoch() != 0 {
		t.Fatal("destroyed cipher exposed a live epoch")
	}
	if _, _, err := cipher.Seal(nil, 1, 1, nil); err == nil {
		t.Fatal("destroyed cipher accepted Seal")
	}
	if _, err := cipher.Open(nil, 1, 1, 1, nil); err == nil {
		t.Fatal("destroyed cipher accepted Open")
	}
	if err := cipher.Rekey(key, prefix, 4); err == nil {
		t.Fatal("destroyed cipher accepted Rekey")
	}
	if err := cipher.PrepareRekey([]byte("destroyed"), 4); err == nil {
		t.Fatal("destroyed cipher accepted PrepareRekey")
	}
	if err := cipher.ActivatePrepared(4); err == nil {
		t.Fatal("destroyed cipher accepted ActivatePrepared")
	}
}

func TestPreparedRekeyAcceptsFutureBeforeAtomicActivation(t *testing.T) {
	var key [32]byte
	var prefix [6]byte
	a, _ := NewMediaCipher(key, prefix, 1)
	b, _ := NewMediaCipher(key, prefix, 1)
	salt := []byte("fresh rekey salt")
	if err := a.PrepareRekey(salt, 2); err != nil {
		t.Fatal(err)
	}
	if err := b.PrepareRekey(salt, 2); err != nil {
		t.Fatal(err)
	}
	if err := a.ActivatePrepared(2); err != nil {
		t.Fatal(err)
	}
	packet, epoch, _ := a.Seal(nil, 42, 2, []byte("new epoch"))
	if _, err := b.Open(nil, 42, epoch, 2, packet); err != nil {
		t.Fatalf("prepared future epoch rejected: %v", err)
	}
	if err := b.ActivatePrepared(2); err != nil {
		t.Fatal(err)
	}
}

func BenchmarkMediaSealOpen(b *testing.B) {
	var key [32]byte
	var prefix [6]byte
	sender, _ := NewMediaCipher(key, prefix, 1)
	receiver, _ := NewMediaCipher(key, prefix, 1)
	aad := make([]byte, 26)
	payload := make([]byte, 120)
	b.ReportAllocs()
	for i := 0; i < b.N; i++ {
		sequence := uint32(i)
		ciphertext, epoch, err := sender.Seal(aad, 42, sequence, payload)
		if err != nil {
			b.Fatal(err)
		}
		if _, err := receiver.Open(aad, 42, epoch, sequence, ciphertext); err != nil {
			b.Fatal(err)
		}
	}
}
