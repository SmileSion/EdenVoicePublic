package config

import (
	"errors"
	"fmt"
	"net"
	"net/url"
)

type WebAdmin struct {
	Enabled    bool   `toml:"enabled"`
	ListenAddr string `toml:"listen_addr"`
	PublicURL  string `toml:"public_url"`
	TLSCert    string `toml:"tls_cert"`
	TLSKey     string `toml:"tls_key"`
}

func (c WebAdmin) Origin() string {
	if c.PublicURL != "" {
		return c.PublicURL
	}
	scheme := "http"
	if c.TLSCert != "" {
		scheme = "https"
	}
	return scheme + "://" + c.ListenAddr
}
func (c WebAdmin) Validate() error {
	if !c.Enabled {
		return nil
	}
	host, _, err := net.SplitHostPort(c.ListenAddr)
	if err != nil {
		return fmt.Errorf("invalid web_admin.listen_addr: %w", err)
	}
	if (c.TLSCert == "") != (c.TLSKey == "") {
		return errors.New("web_admin TLS cert and key must be configured together")
	}
	ip := net.ParseIP(host)
	loopback := host == "localhost" || (ip != nil && ip.IsLoopback())
	if !loopback && c.TLSCert == "" {
		return errors.New("public web_admin listener requires TLS; bind loopback for an HTTPS reverse proxy")
	}
	u, err := url.Parse(c.Origin())
	if err != nil || u.Host == "" || u.User != nil || u.Path != "" || u.RawQuery != "" || u.Fragment != "" || (u.Scheme != "https" && u.Scheme != "http") {
		return errors.New("web_admin.public_url must be an origin such as https://admin.example.com without a trailing slash")
	}
	external := net.ParseIP(u.Hostname())
	originLoopback := u.Hostname() == "localhost" || (external != nil && external.IsLoopback())
	if u.Scheme == "http" && (!loopback || !originLoopback) {
		return errors.New("web_admin public origin requires HTTPS")
	}
	if c.TLSCert != "" && u.Scheme != "https" {
		return errors.New("TLS listener requires an HTTPS public_url")
	}
	return nil
}
