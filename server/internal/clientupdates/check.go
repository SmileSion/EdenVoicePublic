package clientupdates

import (
	"context"
	"errors"
	"net"
	"net/http"
	"time"
)

// Admin link checks must not become a route into local/internal services.
func CheckURL(ctx context.Context, r Release) error {
	if e := Validate(r); e != nil {
		return e
	}
	transport := &http.Transport{DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
		host, port, e := net.SplitHostPort(addr)
		if e != nil {
			return nil, e
		}
		ips, e := net.DefaultResolver.LookupIPAddr(ctx, host)
		if e != nil {
			return nil, e
		}
		for _, ip := range ips {
			if !ip.IP.IsGlobalUnicast() || ip.IP.IsPrivate() || ip.IP.IsLoopback() || ip.IP.IsLinkLocalUnicast() {
				return nil, errors.New("下载地址不能指向内网")
			}
		}
		if len(ips) == 0 {
			return nil, errors.New("域名没有地址")
		}
		return (&net.Dialer{Timeout: 5 * time.Second}).DialContext(ctx, network, net.JoinHostPort(ips[0].IP.String(), port))
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Timeout: 8 * time.Second, Transport: transport, CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= 3 || req.URL.Scheme != "https" {
			return errors.New("下载链接重定向无效")
		}
		return nil
	}}
	req, e := http.NewRequestWithContext(ctx, http.MethodHead, r.URL, nil)
	if e != nil {
		return e
	}
	resp, e := client.Do(req)
	if e != nil {
		return errors.New("无法访问下载链接，请检查 COS 公有读权限和地址")
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 || resp.ContentLength == 0 {
		return errors.New("下载链接未返回有效文件")
	}
	return nil
}
