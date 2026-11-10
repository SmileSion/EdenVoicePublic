package config

import "testing"

func TestWebAdminRequiresSafeOriginAndTLS(t *testing.T) {
	for _, c := range []WebAdmin{{Enabled: true, ListenAddr: "0.0.0.0:9101"}, {Enabled: true, ListenAddr: "127.0.0.1:9101", PublicURL: "http://admin.example.com"}, {Enabled: true, ListenAddr: "127.0.0.1:9101", PublicURL: "https://admin.example.com/path"}, {Enabled: true, ListenAddr: "127.0.0.1:9101", TLSCert: "only.pem"}} {
		if c.Validate() == nil {
			t.Fatal("unsafe configuration accepted", c)
		}
	}
	for _, c := range []WebAdmin{{Enabled: true, ListenAddr: "127.0.0.1:9101"}, {Enabled: true, ListenAddr: "127.0.0.1:9101", PublicURL: "https://admin.example.com"}, {Enabled: true, ListenAddr: "0.0.0.0:9101", PublicURL: "https://admin.example.com", TLSCert: "cert.pem", TLSKey: "key.pem"}} {
		if err := c.Validate(); err != nil {
			t.Fatal(c, err)
		}
	}
}
