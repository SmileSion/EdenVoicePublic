package webadmin

import (
	"edenvoice/server/internal/announcements"
	"testing"
)

func TestAnnouncementPermissionsCSRFAndPublication(t *testing.T) {
	f := setup(t)
	cookie, csrf := f.login(t, "operator")
	origin := f.s.origin.String()
	if r := f.request("GET", "announcements", nil, cookie, "", ""); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	body := map[string]any{"action": "save", "revision": "1", "content": map[string]string{"title": "测试公告", "body": "仅最新一条"}}
	if r := f.request("POST", "announcements", body, cookie, "", origin); r.Code != 403 {
		t.Fatal("missing CSRF", r.Code)
	}
	viewer, vc := f.login(t, "viewer")
	if r := f.request("POST", "announcements", body, viewer, vc, origin); r.Code != 403 {
		t.Fatal("readonly write", r.Code)
	}
	if r := f.request("POST", "announcements", body, cookie, csrf, origin); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	if r := f.request("POST", "announcements", body, cookie, csrf, origin); r.Code != 409 {
		t.Fatal("stale write", r.Code)
	}
	if r := f.request("POST", "announcements", map[string]string{"action": "publish", "revision": "2"}, cookie, csrf, origin); r.Code != 200 {
		t.Fatal(r.Body.String())
	}
	if _, e := f.s.control.WebAnnouncements("user", "", 0, announcements.Content{}); e == nil {
		t.Fatal("ordinary member accessed admin endpoint")
	}
}
