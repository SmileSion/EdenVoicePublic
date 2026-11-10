package security

import (
	"crypto/ed25519"
	cryptorand "crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"os"
	"strconv"
	"time"

	"edenvoice/server/internal/privatefs"
)

const identityFileVersion = 1

// Identity is a persistent Ed25519 identity. PrivateKey must never be logged.
type Identity struct {
	PublicKey  ed25519.PublicKey
	PrivateKey ed25519.PrivateKey
}

type identityFile struct {
	Version    int         `json:"version"`
	PublicKey  string      `json:"public_key"`
	PrivateKey secretBytes `json:"private_key"`
}

// secretBytes keeps identity private-key JSON out of immutable Go strings so
// every decoded and encoded temporary buffer can be explicitly cleared.
type secretBytes []byte

func (s *secretBytes) UnmarshalJSON(encoded []byte) error {
	if len(encoded) < 2 || encoded[0] != '"' || encoded[len(encoded)-1] != '"' {
		return errors.New("invalid identity private key encoding")
	}
	payload := encoded[1 : len(encoded)-1]
	decoded := make([]byte, base64.RawStdEncoding.DecodedLen(len(payload)))
	n, err := base64.RawStdEncoding.Decode(decoded, payload)
	if err != nil {
		clear(decoded)
		return err
	}
	*s = decoded[:n]
	return nil
}

func LoadOrCreateIdentity(path string) (*Identity, bool, error) {
	bytes, err := os.ReadFile(path)
	if err == nil {
		defer clear(bytes)
		if err := privatefs.Ensure(path); err != nil {
			return nil, false, err
		}
		identity, decodeErr := decodeIdentity(bytes)
		return identity, false, decodeErr
	}
	if !errors.Is(err, os.ErrNotExist) {
		return nil, false, fmt.Errorf("read identity: %w", err)
	}
	publicKey, privateKey, err := ed25519.GenerateKey(cryptorand.Reader)
	if err != nil {
		return nil, false, fmt.Errorf("generate identity: %w", err)
	}
	identity := &Identity{PublicKey: publicKey, PrivateKey: privateKey}
	if err := persistIdentity(path, identity); err != nil {
		return nil, false, err
	}
	return identity, true, nil
}

func (i *Identity) Fingerprint() string {
	digest := sha256.Sum256(i.PublicKey)
	return base64.RawURLEncoding.EncodeToString(digest[:])
}

func ClientID(publicKey ed25519.PublicKey) string {
	digest := sha256.Sum256(publicKey)
	return base64.RawURLEncoding.EncodeToString(digest[:])
}

func (i *Identity) TLSConfig(serverName string) (*tls.Config, error) {
	serialLimit := new(big.Int).Lsh(big.NewInt(1), 128)
	serial, err := cryptorand.Int(cryptorand.Reader, serialLimit)
	if err != nil {
		return nil, fmt.Errorf("generate certificate serial: %w", err)
	}
	now := time.Now()
	template := &x509.Certificate{
		SerialNumber:          serial,
		Subject:               pkix.Name{CommonName: serverName},
		NotBefore:             now.Add(-5 * time.Minute),
		NotAfter:              now.AddDate(1, 0, 0),
		KeyUsage:              x509.KeyUsageDigitalSignature,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
	}
	der, err := x509.CreateCertificate(cryptorand.Reader, template, template, i.PublicKey, i.PrivateKey)
	if err != nil {
		return nil, fmt.Errorf("create TLS certificate: %w", err)
	}
	certificate := tls.Certificate{Certificate: [][]byte{der}, PrivateKey: i.PrivateKey}
	return &tls.Config{Certificates: []tls.Certificate{certificate}, MinVersion: tls.VersionTLS13}, nil
}

func persistIdentity(path string, identity *Identity) error {
	if err := privatefs.PrepareDirectory(path); err != nil {
		return err
	}
	stored := identityFile{
		Version:    identityFileVersion,
		PublicKey:  base64.RawStdEncoding.EncodeToString(identity.PublicKey),
		PrivateKey: append(secretBytes(nil), identity.PrivateKey...),
	}
	defer clear(stored.PrivateKey)
	encoded := encodeIdentityFile(stored)
	defer clear(encoded)
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, encoded, 0o600); err != nil {
		return fmt.Errorf("write identity: %w", err)
	}
	if err := os.Rename(temporary, path); err != nil {
		_ = os.Remove(temporary)
		return fmt.Errorf("commit identity: %w", err)
	}
	return privatefs.Ensure(path)
}

func encodeIdentityFile(stored identityFile) []byte {
	encoded := make([]byte, 0, 160)
	encoded = append(encoded, "{\n  \"version\": "...)
	encoded = strconv.AppendInt(encoded, int64(stored.Version), 10)
	encoded = append(encoded, ",\n  \"public_key\": "...)
	encoded = strconv.AppendQuote(encoded, stored.PublicKey)
	encoded = append(encoded, ",\n  \"private_key\": \""...)
	start := len(encoded)
	encoded = append(encoded, make([]byte, base64.RawStdEncoding.EncodedLen(len(stored.PrivateKey)))...)
	base64.RawStdEncoding.Encode(encoded[start:], stored.PrivateKey)
	encoded = append(encoded, "\"\n}"...)
	return encoded
}

func decodeIdentity(bytes []byte) (*Identity, error) {
	var stored identityFile
	if err := json.Unmarshal(bytes, &stored); err != nil {
		return nil, fmt.Errorf("decode identity: %w", err)
	}
	defer clear(stored.PrivateKey)
	if stored.Version != identityFileVersion {
		return nil, fmt.Errorf("unsupported identity version %d", stored.Version)
	}
	publicKey, err := base64.RawStdEncoding.DecodeString(stored.PublicKey)
	if err != nil || len(publicKey) != ed25519.PublicKeySize {
		return nil, errors.New("invalid identity public key")
	}
	if len(stored.PrivateKey) != ed25519.PrivateKeySize {
		return nil, errors.New("invalid identity private key")
	}
	if !ed25519.PublicKey(publicKey).Equal(ed25519.PrivateKey(stored.PrivateKey).Public()) {
		return nil, errors.New("identity key pair does not match")
	}
	privateKey := append(ed25519.PrivateKey(nil), stored.PrivateKey...)
	return &Identity{PublicKey: ed25519.PublicKey(publicKey), PrivateKey: privateKey}, nil
}
