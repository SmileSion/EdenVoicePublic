package webadmin

import (
	"crypto/sha256"
	"crypto/subtle"
	"embed"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"log/slog"
	"mime"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/announcements"
	"edenvoice/server/internal/clientupdates"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/control"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"
	"edenvoice/server/internal/voice"
)

//go:embed static/*
var assets embed.FS

type loginSession struct {
	Username, Version, CSRF, Role string
	Created, Seen                 time.Time
}
type rateWindow struct {
	Start time.Time
	Count int
}
type Server struct {
	cfg            config.WebAdmin
	control        *control.Server
	relay          *voice.Relay
	accounts       *Accounts
	logger         *slog.Logger
	origin         *url.URL
	mu             sync.Mutex
	sessions       map[[32]byte]loginSession
	rates          map[string]rateWindow
	hashSlots      chan struct{}
	metricsMu      sync.Mutex
	metricsHistory []Sample
	metricsAt      time.Time
	started        time.Time
}

func New(cfg config.WebAdmin, ctrl *control.Server, relay *voice.Relay, accounts *Accounts, logger *slog.Logger) (*Server, error) {
	if err := cfg.Validate(); err != nil {
		return nil, err
	}
	origin, err := url.Parse(cfg.Origin())
	if err != nil {
		return nil, err
	}
	return &Server{cfg: cfg, control: ctrl, relay: relay, accounts: accounts, logger: logger, origin: origin, sessions: map[[32]byte]loginSession{}, rates: map[string]rateWindow{}, hashSlots: make(chan struct{}, 1), started: time.Now()}, nil
}
func (s *Server) ListenAndServe() error {
	srv := &http.Server{Addr: s.cfg.ListenAddr, Handler: s.Handler(), ReadHeaderTimeout: 3 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 15 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 16 << 10}
	s.logger.Info("web administration started", "origin", s.origin.String(), "addr", s.cfg.ListenAddr)
	if s.cfg.TLSCert != "" {
		return srv.ListenAndServeTLS(s.cfg.TLSCert, s.cfg.TLSKey)
	}
	return srv.ListenAndServe()
}
func (s *Server) Handler() http.Handler {
	files, _ := fs.Sub(assets, "static")
	static := http.FileServer(http.FS(files))
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("X-Frame-Options", "DENY")
		w.Header().Set("Referrer-Policy", "no-referrer")
		w.Header().Set("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
		w.Header().Set("Cache-Control", "no-store")
		if !strings.EqualFold(r.Host, s.origin.Host) {
			http.Error(w, "invalid host", http.StatusMisdirectedRequest)
			return
		}
		if s.origin.Scheme == "https" {
			w.Header().Set("Strict-Transport-Security", "max-age=31536000")
		}
		if strings.HasPrefix(r.URL.Path, "/api/") {
			s.api(w, r)
			return
		}
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			http.Error(w, "method not allowed", 405)
			return
		}
		switch r.URL.Path {
		case "/", "/index.html", "/app.js", "/style.css", "/app-icon.svg":
			static.ServeHTTP(w, r)
		default:
			http.NotFound(w, r)
		}
	})
}
func fail(w http.ResponseWriter, status int, message string) {
	writeJSON(w, status, map[string]string{"error": message})
}
func writeJSON(w http.ResponseWriter, status int, value any) {
	data, err := json.Marshal(value)
	if err != nil {
		http.Error(w, "serialization failed", 500)
		return
	}
	// Browser identifiers must remain exact even for random 64-bit session IDs.
	var tree any
	d := json.NewDecoder(strings.NewReader(string(data)))
	d.UseNumber()
	_ = d.Decode(&tree)
	stringifyIDs(tree)
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(tree)
}
func isID(k string) bool {
	return k == "id" || k == "epoch" || k == "revision" || strings.HasSuffix(k, "_id") || strings.HasSuffix(k, "ID")
}
func stringifyIDs(v any) {
	switch x := v.(type) {
	case map[string]any:
		for k, v := range x {
			if n, ok := v.(json.Number); ok && isID(k) {
				x[k] = n.String()
			} else {
				stringifyIDs(v)
			}
		}
	case []any:
		for _, v := range x {
			stringifyIDs(v)
		}
	}
}
func numericIDs(v any) error {
	switch x := v.(type) {
	case map[string]any:
		for k, v := range x {
			if str, ok := v.(string); ok && isID(k) && k != "client_id" && k != "operator_client_id" {
				if _, err := strconv.ParseUint(str, 10, 64); err != nil {
					return errors.New("invalid numeric identifier")
				}
				x[k] = json.Number(str)
			} else if err := numericIDs(v); err != nil {
				return err
			}
		}
	case []any:
		for _, v := range x {
			if err := numericIDs(v); err != nil {
				return err
			}
		}
	}
	return nil
}
func decode(w http.ResponseWriter, r *http.Request, v any) error {
	media, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if media != "application/json" {
		return errors.New("需要 JSON 请求")
	}
	r.Body = http.MaxBytesReader(w, r.Body, 64<<10)
	d := json.NewDecoder(r.Body)
	d.DisallowUnknownFields()
	if err := d.Decode(v); err != nil {
		return errors.New("请求格式错误或内容过大")
	}
	if d.Decode(&struct{}{}) != io.EOF {
		return errors.New("请求包含多余数据")
	}
	return nil
}
func (s *Server) rate(key string, max int, now time.Time) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for k, v := range s.rates {
		if now.Sub(v.Start) > time.Minute {
			delete(s.rates, k)
		}
	}
	v := s.rates[key]
	if v.Start.IsZero() {
		if len(s.rates) >= 4096 {
			return false
		}
		v.Start = now
	}
	v.Count++
	s.rates[key] = v
	return v.Count <= max
}
func (s *Server) cookie(w http.ResponseWriter, token string, expired bool) {
	max := 8 * 60 * 60
	if expired {
		max = -1
	}
	http.SetCookie(w, &http.Cookie{Name: "eden_admin", Value: token, Path: "/", HttpOnly: true, Secure: s.origin.Scheme == "https", SameSite: http.SameSiteStrictMode, MaxAge: max})
}
func (s *Server) authenticated(r *http.Request) (Account, loginSession, [32]byte, error) {
	cookie, err := r.Cookie("eden_admin")
	if err != nil {
		return Account{}, loginSession{}, [32]byte{}, err
	}
	key := sha256.Sum256([]byte(cookie.Value))
	now := time.Now()
	s.mu.Lock()
	session, ok := s.sessions[key]
	if !ok || now.Sub(session.Created) > 8*time.Hour || now.Sub(session.Seen) > 30*time.Minute {
		delete(s.sessions, key)
		s.mu.Unlock()
		return Account{}, session, key, ErrCredentials
	}
	session.Seen = now
	s.sessions[key] = session
	s.mu.Unlock()
	u, ok := s.accounts.Get(session.Username)
	if !ok || u.Disabled || u.Version != session.Version {
		return Account{}, session, key, ErrCredentials
	}
	if role, roleErr := s.control.WebRole(u.ClientID); roleErr != nil || role != session.Role {
		return Account{}, session, key, ErrCredentials
	}
	return u, session, key, nil
}
func (s *Server) api(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodPost {
		fail(w, 405, "不支持此请求方法")
		return
	}
	if r.Method == http.MethodPost && r.Header.Get("Origin") != s.origin.String() {
		fail(w, 403, "请求来源不匹配，请重新打开管理页面")
		return
	}
	if r.URL.Path == "/api/login" {
		if r.Method != http.MethodPost {
			fail(w, 405, "请使用登录表单")
			return
		}
		s.login(w, r)
		return
	}
	u, session, key, err := s.authenticated(r)
	if err != nil {
		fail(w, 401, "登录已失效，请重新登录")
		return
	}
	role, _ := s.control.WebRole(u.ClientID)
	if r.Method == http.MethodPost {
		if subtle.ConstantTimeCompare([]byte(r.Header.Get("X-CSRF-Token")), []byte(session.CSRF)) != 1 {
			fail(w, 403, "会话校验失败，请刷新页面")
			return
		}
		if !s.rate("write:"+u.Username, 30, time.Now()) {
			fail(w, 429, "操作过于频繁，请稍后重试")
			return
		}
	}
	switch r.Method + " " + r.URL.Path {
	case "GET /api/announcements", "POST /api/announcements":
		var req struct {
			Action   string                `json:"action"`
			Revision uint64                `json:"revision,string"`
			Content  announcements.Content `json:"content"`
		}
		if r.Method == http.MethodPost {
			if u.ReadOnly {
				fail(w, 403, "只读账号不能修改公告")
				return
			}
			if e := decode(w, r, &req); e != nil {
				fail(w, 400, e.Error())
				return
			}
			if req.Action == "" {
				fail(w, 400, "请选择公告操作")
				return
			}
		}
		result, e := s.control.WebAnnouncements(u.ClientID, req.Action, req.Revision, req.Content)
		if e != nil {
			fail(w, 409, e.Error())
			return
		}
		writeJSON(w, 200, result)
	case "GET /api/updates", "POST /api/updates":
		var req struct {
			Action   string                `json:"action"`
			Revision uint64                `json:"revision,string"`
			Release  clientupdates.Release `json:"release"`
		}
		if r.Method == http.MethodPost {
			if u.ReadOnly {
				fail(w, 403, "只读账号不能发布更新")
				return
			}
			if err = decode(w, r, &req); err != nil {
				fail(w, 400, err.Error())
				return
			}
			if req.Action == "" {
				fail(w, 400, "缺少操作")
				return
			}
		}
		if req.Action == "publish" {
			d, e := s.control.WebClientUpdates(u.ClientID, "", 0, clientupdates.Release{})
			if e != nil {
				fail(w, 403, e.Error())
				return
			}
			entry, ok := d.Entries[req.Release.Platform]
			if !ok || d.Revision != req.Revision {
				fail(w, 409, "草稿已变化，请刷新")
				return
			}
			if e = clientupdates.CheckURL(r.Context(), entry.Draft); e != nil {
				fail(w, 400, e.Error())
				return
			}
		}
		result, e := s.control.WebClientUpdates(u.ClientID, req.Action, req.Revision, req.Release)
		if e != nil {
			fail(w, 409, e.Error())
			return
		}
		writeJSON(w, 200, result)
	case "GET /api/screen-sharing", "POST /api/screen-sharing":
		var update *screenshare.PolicyDocument
		if r.Method == http.MethodPost {
			if u.ReadOnly {
				fail(w, 403, "只读账号不能修改共享限制")
				return
			}
			var req struct {
				Revision uint64             `json:"revision,string"`
				Policy   screenshare.Policy `json:"policy"`
			}
			if err = decode(w, r, &req); err != nil {
				fail(w, 400, err.Error())
				return
			}
			update = &screenshare.PolicyDocument{Revision: req.Revision, Policy: req.Policy}
		}
		result, e := s.control.WebScreenSharing(u.ClientID, update)
		if e != nil {
			fail(w, 409, e.Error())
			return
		}
		writeJSON(w, 200, result)
	case "GET /api/games", "POST /api/games":
		action := ""
		var req struct {
			Action   string          `json:"action"`
			Revision uint64          `json:"revision,string"`
			Games    []protocol.Game `json:"games"`
		}
		if r.Method == http.MethodPost {
			if u.ReadOnly {
				fail(w, 403, "只读账号不能修改游戏目录")
				return
			}
			if err = decode(w, r, &req); err != nil {
				fail(w, 400, err.Error())
				return
			}
			action = req.Action
			if action == "" {
				fail(w, 400, "缺少目录操作")
				return
			}
		}
		result, e := s.control.WebGameCatalog(u.ClientID, action, req.Revision, req.Games)
		if e != nil {
			fail(w, 409, e.Error())
			return
		}
		writeJSON(w, 200, result)
	case "GET /api/session":
		writeJSON(w, 200, map[string]any{"username": u.Username, "role": role, "read_only": u.ReadOnly, "client_id": u.ClientID, "csrf": session.CSRF})
	case "POST /api/logout":
		s.mu.Lock()
		delete(s.sessions, key)
		s.mu.Unlock()
		s.cookie(w, "", true)
		s.control.WebAudit("web logout", u.ClientID, u.Username)
		writeJSON(w, 200, map[string]bool{"ok": true})
	case "GET /api/overview":
		if !s.rate("read:"+u.Username, 90, time.Now()) {
			fail(w, 429, "刷新过于频繁")
			return
		}
		data, err := s.control.WebSnapshot(u.ClientID)
		if err != nil {
			fail(w, 403, "权限不足")
			return
		}
		writeJSON(w, 200, map[string]any{"data": data, "performance": s.performance(), "observed_at": time.Now()})
	case "GET /api/avatar":
		data, ok := s.control.WebAvatar(u.ClientID, r.URL.Query().Get("reference"))
		if !ok {
			fail(w, 404, "头像未找到")
			return
		}
		writeJSON(w, 200, map[string]string{"data": data})
	case "POST /api/command":
		if u.ReadOnly {
			fail(w, 403, "只读账号不能执行管理操作")
			return
		}
		var req struct {
			Action  string          `json:"action"`
			Payload json.RawMessage `json:"payload"`
		}
		if err = decode(w, r, &req); err != nil {
			fail(w, 400, err.Error())
			return
		}
		var payload map[string]any
		d := json.NewDecoder(strings.NewReader(string(req.Payload)))
		d.UseNumber()
		if err = d.Decode(&payload); err != nil || payload == nil {
			fail(w, 400, "缺少操作参数")
			return
		}
		if err = numericIDs(payload); err != nil {
			fail(w, 400, err.Error())
			return
		}
		envelope, e := protocol.NewEnvelope(protocol.MessageType(req.Action), payload)
		if e != nil {
			fail(w, 400, "操作格式错误")
			return
		}
		if req.Action == "watch_close" {
			var q struct {
				ChannelID uint64 `json:"channel_id"`
				Epoch     uint64 `json:"epoch"`
			}
			err = envelope.Decode(&q)
			if err == nil {
				err = s.control.WebCloseWatch(u.ClientID, q.ChannelID, q.Epoch)
			}
		} else {
			err = s.control.WebCommand(u.ClientID, envelope)
		}
		if err != nil {
			s.control.WebAudit("web action rejected: "+bounded(req.Action, 48), u.ClientID, "")
			if errors.Is(err, administration.ErrForbidden) {
				fail(w, 403, "当前身份没有权限执行此操作")
			} else {
				fail(w, 409, err.Error())
			}
			return
		}
		s.control.WebAudit("web action: "+bounded(req.Action, 48), u.ClientID, u.Username)
		writeJSON(w, 200, map[string]bool{"ok": true})
	case "POST /api/password":
		select {
		case s.hashSlots <- struct{}{}:
			defer func() { <-s.hashSlots }()
		default:
			fail(w, 429, "密码服务繁忙，请稍后重试")
			return
		}
		var req struct {
			Current  string `json:"current"`
			Password string `json:"password"`
		}
		if err = decode(w, r, &req); err != nil {
			fail(w, 400, err.Error())
			return
		}
		if _, err = s.accounts.Authenticate(u.Username, req.Current); err != nil {
			fail(w, 403, "当前密码不正确")
			return
		}
		if req.Password == "" {
			fail(w, 400, "新密码不能为空")
			return
		}
		if err = s.accounts.Put(u.Username, u.ClientID, req.Password, u.ReadOnly, u.Disabled); err != nil {
			fail(w, 400, err.Error())
			return
		}
		s.cookie(w, "", true)
		s.control.WebAudit("web password changed", u.ClientID, u.Username)
		writeJSON(w, 200, map[string]bool{"ok": true})
	case "GET /api/accounts":
		if role != "MASTER" || u.ReadOnly {
			fail(w, 403, "仅 MASTER 可管理 Web 账号")
			return
		}
		writeJSON(w, 200, s.accounts.List())
	case "POST /api/accounts":
		if role != "MASTER" || u.ReadOnly {
			fail(w, 403, "仅 MASTER 可管理 Web 账号")
			return
		}
		select {
		case s.hashSlots <- struct{}{}:
			defer func() { <-s.hashSlots }()
		default:
			fail(w, 429, "密码服务繁忙，请稍后重试")
			return
		}
		var req struct {
			Username string `json:"username"`
			ClientID string `json:"client_id"`
			Password string `json:"password"`
			ReadOnly bool   `json:"read_only"`
			Disabled bool   `json:"disabled"`
		}
		if err = decode(w, r, &req); err != nil {
			fail(w, 400, err.Error())
			return
		}
		if req.Username == u.Username {
			fail(w, 409, "当前账号请使用修改密码，不能在此停用或改绑自己")
			return
		}
		if _, err = s.control.WebRole(req.ClientID); err != nil {
			fail(w, 400, "请绑定现有 MASTER 或服务器 ADMIN 身份")
			return
		}
		if err = s.accounts.Put(req.Username, req.ClientID, req.Password, req.ReadOnly, req.Disabled); err != nil {
			fail(w, 400, err.Error())
			return
		}
		s.control.WebAudit("web account updated", u.ClientID, req.Username)
		writeJSON(w, 200, map[string]bool{"ok": true})
	default:
		fail(w, 404, "接口不存在")
	}
}
func bounded(s string, n int) string {
	if len(s) > n {
		return s[:n]
	}
	return s
}
func (s *Server) login(w http.ResponseWriter, r *http.Request) {
	ip, _, _ := net.SplitHostPort(r.RemoteAddr)
	if !s.rate("login-ip:"+ip, 10, time.Now()) {
		fail(w, 429, "登录尝试过多，请一分钟后重试")
		return
	}
	var req struct {
		Username string `json:"username"`
		Password string `json:"password"`
	}
	if err := decode(w, r, &req); err != nil {
		fail(w, 400, err.Error())
		return
	}
	if len(req.Username) > 48 || len(req.Password) > 72 {
		fail(w, 401, ErrCredentials.Error())
		return
	}
	if !s.rate("login-user:"+req.Username, 8, time.Now()) {
		fail(w, 429, "登录尝试过多，请一分钟后重试")
		return
	}
	select {
	case s.hashSlots <- struct{}{}:
		defer func() { <-s.hashSlots }()
	default:
		fail(w, 429, "登录服务繁忙，请稍后重试")
		return
	}
	u, err := s.accounts.Authenticate(req.Username, req.Password)
	if err == nil {
		_, err = s.control.WebRole(u.ClientID)
	}
	if err != nil {
		s.control.WebAudit("web login failed", "", bounded(req.Username, 48))
		fail(w, 401, ErrCredentials.Error())
		return
	}
	token := randomValue()
	csrf := randomValue()
	now := time.Now()
	s.mu.Lock()
	for k, v := range s.sessions {
		if now.Sub(v.Created) > 8*time.Hour || now.Sub(v.Seen) > 30*time.Minute {
			delete(s.sessions, k)
		}
	}
	if len(s.sessions) >= 256 {
		s.mu.Unlock()
		fail(w, 429, "管理会话已满，请稍后重试")
		return
	}
	if old, err := r.Cookie("eden_admin"); err == nil {
		delete(s.sessions, sha256.Sum256([]byte(old.Value)))
	}
	s.sessions[sha256.Sum256([]byte(token))] = loginSession{Username: u.Username, Version: u.Version, CSRF: csrf, Role: func() string { role, _ := s.control.WebRole(u.ClientID); return role }(), Created: now, Seen: now}
	s.mu.Unlock()
	s.cookie(w, token, false)
	s.control.WebAudit("web login", u.ClientID, u.Username)
	writeJSON(w, 200, map[string]string{"csrf": csrf})
}
