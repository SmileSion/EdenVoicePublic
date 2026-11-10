// COS traffic fuse. Standalone tool; never linked into the voice server.
package main

import (
	"context"
	"crypto/md5"
	"encoding/base64"
	"encoding/json"
	"encoding/xml"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"math"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"syscall"
	"time"

	"github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/common"
	"github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/common/profile"
	monitor "github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/monitor/v20180724"
	cos "github.com/tencentyun/cos-go-sdk-v5"
	"golang.org/x/sys/unix"
	"golang.org/x/term"
)

const sid = "EdenVoiceTrafficGuardDenyPublicDownloads"

var cst = time.FixedZone("CST", 8*3600)

type policy map[string]any
type settings struct {
	Bucket   string `json:"bucket"`
	Region   string `json:"region"`
	Limit    int64  `json:"limit_bytes"`
	ProbeKey string `json:"probe_key"`
}
type credentials struct {
	SecretID  string `json:"secret_id"`
	SecretKey string `json:"secret_key"`
	Token     string `json:"token,omitempty"`
}
type trip struct {
	Bucket string  `json:"bucket"`
	Region string  `json:"region"`
	Day    string  `json:"day"`
	Bytes  float64 `json:"bytes"`
}
type api interface {
	Traffic(context.Context, time.Time, time.Time) (float64, error)
	Policy(context.Context) (policy, error)
	Put(context.Context, policy) error
	Verify(context.Context) error
}

func atomicJSON(path string, value any) error {
	b, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".guard-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(b); err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	return os.Rename(f.Name(), path)
}
func loadJSON(path string, value any) error {
	b, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	return json.Unmarshal(b, value)
}
func initCredentials(path string) error {
	if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
		return errors.New("凭据文件已存在或不可访问，不覆盖")
	}
	if !term.IsTerminal(int(os.Stdin.Fd())) {
		return errors.New("请在交互终端输入密钥")
	}
	c := credentials{}
	for _, item := range []struct {
		name   string
		target *string
	}{{"SecretId", &c.SecretID}, {"SecretKey", &c.SecretKey}} {
		fmt.Fprintf(os.Stderr, "请输入 %s（不回显）: ", item.name)
		data, err := term.ReadPassword(int(os.Stdin.Fd()))
		fmt.Fprintln(os.Stderr)
		if err != nil {
			return err
		}
		*item.target = strings.TrimSpace(string(data))
	}
	if c.SecretID == "" || c.SecretKey == "" {
		return errors.New("密钥不能为空")
	}
	b, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	_, err = f.Write(b)
	if err == nil {
		err = f.Sync()
	}
	ce := f.Close()
	if err != nil {
		return err
	}
	return ce
}
func readCredentials(path string) (credentials, error) {
	var c credentials
	info, err := os.Lstat(path)
	if err != nil {
		return c, err
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0077 != 0 {
		return c, errors.New("凭据必须为普通文件且权限为 600 或更严格")
	}
	if err = loadJSON(path, &c); err != nil {
		return c, errors.New("凭据文件格式错误")
	}
	if c.SecretID == "" || c.SecretKey == "" {
		return c, errors.New("缺少 SecretId/SecretKey")
	}
	return c, nil
}
func blockRule(s settings) map[string]any {
	appid := s.Bucket[strings.LastIndex(s.Bucket, "-")+1:]
	return map[string]any{"sid": sid, "effect": "deny", "principal": map[string]any{"qcs": []any{"qcs::cam::anyone:anyone"}}, "action": []any{"name/cos:GetObject"}, "resource": []any{fmt.Sprintf("qcs::cos:%s:uid/%s:%s/*", s.Region, appid, s.Bucket)}}
}
func addBlock(original policy, s settings) (policy, error) {
	var result policy
	if original == nil {
		result = policy{"version": "2.0", "statement": []any{}}
	} else {
		b, e := json.Marshal(original)
		if e != nil {
			return nil, e
		}
		if e = json.Unmarshal(b, &result); e != nil {
			return nil, e
		}
	}
	key := "statement"
	if _, ok := result["Statement"]; ok {
		key = "Statement"
	}
	statements, ok := result[key].([]any)
	if !ok {
		return nil, errors.New("无法识别现有桶策略")
	}
	expected := blockRule(s)
	for _, raw := range statements {
		st, ok := raw.(map[string]any)
		if !ok {
			return nil, errors.New("无法识别现有策略条目")
		}
		normalized := map[string]any{}
		for k, v := range st {
			normalized[strings.ToLower(k)] = v
		}
		if normalized["sid"] == sid {
			if !reflect.DeepEqual(normalized, expected) {
				return nil, errors.New("封堵 SID 与现有规则冲突")
			}
			return result, nil
		}
	}
	result[key] = append(statements, expected)
	return result, nil
}
func check(ctx context.Context, a api, s settings, stateDir string, enforce bool, now time.Time) (string, error) {
	marker := filepath.Join(stateDir, "blocked.json")
	var reason trip
	err := loadJSON(marker, &reason)
	if err == nil {
		if reason.Bucket != s.Bucket || reason.Region != s.Region {
			return "", errors.New("状态目录属于另一个桶")
		}
	} else if errors.Is(err, os.ErrNotExist) {
		local := now.In(cst)
		midnight := time.Date(local.Year(), local.Month(), local.Day(), 0, 0, 0, 0, cst)
		var monitorErr error
		for _, window := range [][2]time.Time{{midnight.AddDate(0, 0, -1), midnight}, {midnight, now}} {
			if window[1].Sub(window[0]) < 5*time.Minute {
				continue
			}
			total, e := a.Traffic(ctx, window[0], window[1])
			if e != nil {
				monitorErr = e
				continue
			}
			slog.Info("traffic", "day", window[0].Format("2006-01-02"), "bytes", total, "limit", s.Limit)
			if total >= float64(s.Limit) {
				reason = trip{s.Bucket, s.Region, window[0].Format("2006-01-02"), total}
				break
			}
		}
		if reason.Day == "" {
			if monitorErr != nil {
				return "", monitorErr
			}
			return "below_threshold", nil
		}
	} else {
		return "", err
	}
	if !enforce {
		return "would_block", nil
	}
	if err = atomicJSON(marker, reason); err != nil {
		return "", err
	}
	original, err := a.Policy(ctx)
	if err != nil {
		return "", err
	}
	desired, err := addBlock(original, s)
	if err != nil {
		return "", err
	}
	if !reflect.DeepEqual(original, desired) {
		backup := filepath.Join(stateDir, fmt.Sprintf("policy-before-%d.json", time.Now().UnixNano()))
		if err = atomicJSON(backup, original); err != nil {
			return "", err
		}
		latest, e := a.Policy(ctx)
		if e != nil {
			return "", e
		}
		if !reflect.DeepEqual(latest, original) {
			return "", errors.New("桶策略同时发生修改，稍后重试")
		}
		if err = a.Put(ctx, desired); err != nil {
			return "", err
		}
	}
	confirmed, err := a.Policy(ctx)
	if err != nil {
		return "", err
	}
	merged, err := addBlock(confirmed, s)
	if err != nil {
		return "", err
	}
	if confirmed == nil || !reflect.DeepEqual(merged, confirmed) {
		return "", errors.New("未读回封堵规则")
	}
	if err = a.Verify(ctx); err != nil {
		return "", err
	}
	return "blocked_verified", nil
}

type cloud struct {
	s              settings
	monitor        *monitor.Client
	signed, public *http.Client
	base           string
}

func newCloud(s settings, c credentials) (*cloud, error) {
	p := profile.NewClientProfile()
	p.HttpProfile.ReqTimeout = 15
	m, err := monitor.NewClient(common.NewTokenCredential(c.SecretID, c.SecretKey, c.Token), "ap-guangzhou", p)
	if err != nil {
		return nil, err
	}
	noRedirect := func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	transport := &cos.AuthorizationTransport{SecretID: c.SecretID, SecretKey: c.SecretKey, SessionToken: c.Token}
	return &cloud{s: s, monitor: m, base: fmt.Sprintf("https://%s.cos.%s.myqcloud.com", s.Bucket, s.Region), signed: &http.Client{Timeout: 15 * time.Second, Transport: transport, CheckRedirect: noRedirect}, public: &http.Client{Timeout: 15 * time.Second, CheckRedirect: noRedirect}}, nil
}
func trafficRequest(bucket string, start, end time.Time) *monitor.GetMonitorDataRequest {
	r := monitor.NewGetMonitorDataRequest()
	r.Namespace = common.StringPtr("QCE/COS")
	r.MetricName = common.StringPtr("InternetTraffic")
	r.Period = common.Uint64Ptr(300)
	r.StartTime = common.StringPtr(start.Format(time.RFC3339))
	r.EndTime = common.StringPtr(end.Format(time.RFC3339))
	r.Instances = []*monitor.Instance{{Dimensions: []*monitor.Dimension{{Name: common.StringPtr("bucket"), Value: common.StringPtr(bucket)}}}}
	return r
}

func (c *cloud) Traffic(ctx context.Context, start, end time.Time) (float64, error) {
	r := trafficRequest(c.s.Bucket, start, end)
	response, err := c.monitor.GetMonitorDataWithContext(ctx, r)
	if err != nil {
		return 0, errors.New("monitor_request_failed")
	}
	return sumTraffic(response.Response, c.s.Bucket, start, end)
}
func sumTraffic(r *monitor.GetMonitorDataResponseParams, bucket string, start, end time.Time) (float64, error) {
	bad := errors.New("monitor_data_missing_or_invalid")
	if r == nil || r.Period == nil || *r.Period != 300 || len(r.DataPoints) != 1 || r.DataPoints[0] == nil {
		return 0, bad
	}
	p := r.DataPoints[0]
	match := false
	for _, d := range p.Dimensions {
		if d != nil && d.Name != nil && d.Value != nil && *d.Name == "bucket" && *d.Value == bucket {
			match = true
		}
	}
	if !match || len(p.Timestamps) == 0 || len(p.Timestamps) != len(p.Values) {
		return 0, bad
	}
	seen := map[int64]float64{}
	for i, stamp := range p.Timestamps {
		value := p.Values[i]
		if stamp == nil || value == nil || *stamp >= float64(math.MaxInt64) || math.IsNaN(*stamp) || math.IsInf(*stamp, 0) || *stamp < 0 || math.Trunc(*stamp) != *stamp || math.IsNaN(*value) || math.IsInf(*value, 0) || *value < 0 {
			return 0, bad
		}
		ts := int64(*stamp)
		if ts < start.Unix() || ts >= end.Unix() {
			continue
		}
		if prior, ok := seen[ts]; ok && prior != *value {
			return 0, bad
		}
		seen[ts] = *value
	}
	if len(seen) == 0 {
		return 0, bad
	}
	var total float64
	for _, v := range seen {
		total += v
	}
	if math.IsInf(total, 0) {
		return 0, bad
	}
	return total, nil
}
func (c *cloud) Policy(ctx context.Context) (policy, error) {
	req, _ := http.NewRequestWithContext(ctx, "GET", c.base+"/?policy", nil)
	resp, err := c.signed.Do(req)
	if err != nil {
		return nil, errors.New("get_policy_failed")
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, errors.New("read_policy_failed")
	}
	var apiError struct {
		Code string `xml:"Code"`
	}
	_ = xml.Unmarshal(body, &apiError)

	if resp.StatusCode == 404 && (apiError.Code == "NoSuchBucketPolicy" || apiError.Code == "NoSuchPolicy" || (len(body) == 0 && resp.Header.Get("X-Cos-Request-Id") != "")) {
		return nil, nil
	}
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("get_policy_http_%d_code_%s", resp.StatusCode, regexp.MustCompile(`[^A-Za-z0-9_]`).ReplaceAllString(apiError.Code, ""))
	}
	var p policy
	if err = json.Unmarshal(body, &p); err != nil {
		return nil, errors.New("invalid_policy_json")
	}
	return p, nil
}
func (c *cloud) Put(ctx context.Context, p policy) error {
	b, err := json.Marshal(p)
	if err != nil {
		return err
	}
	req, _ := http.NewRequestWithContext(ctx, "PUT", c.base+"/?policy", strings.NewReader(string(b)))
	req.Header.Set("Content-Type", "application/json")
	checksum := md5.Sum(b)
	req.Header.Set("Content-MD5", base64.StdEncoding.EncodeToString(checksum[:]))
	resp, err := c.signed.Do(req)
	if err != nil {
		return errors.New("put_policy_failed")
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 && resp.StatusCode != 204 {
		return fmt.Errorf("put_policy_http_%d", resp.StatusCode)
	}
	return nil
}
func (c *cloud) Verify(ctx context.Context) error {
	u, _ := url.Parse(c.base)
	u.Path = "/" + c.s.ProbeKey
	req, _ := http.NewRequestWithContext(ctx, "GET", u.String(), nil)
	req.Header.Set("Range", "bytes=0-0")
	resp, err := c.public.Do(req)
	if err != nil {
		return errors.New("probe_request_failed")
	}
	defer resp.Body.Close()
	if resp.StatusCode != 403 {
		return fmt.Errorf("public_download_not_denied_http_%d", resp.StatusCode)
	}
	return nil
}
func run() error {
	configPath := flag.String("config", "config.json", "non-secret settings")
	credsPath := flag.String("credentials", "credentials.json", "private credentials file (0600)")
	stateDir := flag.String("state-dir", "cos-guard-state", "persistent state and policy backups")
	init := flag.Bool("init-credentials", false, "interactively save credentials, without echo")
	enforce := flag.Bool("enforce", false, "apply block; otherwise read-only")
	watch := flag.Bool("watch", false, "repeat every 60 seconds")
	diagnose := flag.Bool("diagnose", false, "read-only comparison of COS traffic metrics")
	version := flag.Bool("version", false, "print version")
	flag.Parse()
	if *version {
		fmt.Println("EdenVoice COS traffic guard 1.0.1")
		return nil
	}
	if *init {
		return initCredentials(*credsPath)
	}
	var s settings
	if err := loadJSON(*configPath, &s); err != nil {
		return errors.New("无法读取配置文件")
	}
	if !regexp.MustCompile(`^[a-z0-9][a-z0-9-]*-[0-9]+$`).MatchString(s.Bucket) || !regexp.MustCompile(`^[a-z]+-[a-z]+(?:-[a-z]+)?$`).MatchString(s.Region) || s.Limit <= 0 || s.ProbeKey == "" {
		return errors.New("配置无效")
	}
	c, err := readCredentials(*credsPath)
	if err != nil {
		return errors.New("无法读取凭据，请使用 init-credentials 创建并限制文件权限")
	}
	if err = os.MkdirAll(*stateDir, 0700); err != nil {
		return err
	}
	info, err := os.Lstat(*stateDir)
	if err != nil {
		return err
	}
	if !info.IsDir() || info.Mode().Perm()&0077 != 0 {
		return errors.New("状态目录必须为 700 或更严格")
	}
	lock, err := os.OpenFile(filepath.Join(*stateDir, "guard.lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err = unix.Flock(int(lock.Fd()), unix.LOCK_EX|unix.LOCK_NB); err != nil {
		return errors.New("另一个守护进程正在运行")
	}
	a, err := newCloud(s, c)
	if err != nil {
		return errors.New("无法初始化云客户端")
	}
	if *diagnose {
		return a.diagnose(context.Background())
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	for {
		status, e := check(ctx, a, s, *stateDir, *enforce, time.Now())
		if e != nil {
			slog.Error("check_failed", "reason", e.Error())
		} else {
			slog.Info(status)
		}
		if !*watch {
			return e
		}
		select {
		case <-ctx.Done():
			return nil
		case <-time.After(60 * time.Second):
		}
	}
}
func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, nil)))
	if err := run(); err != nil {
		slog.Error("guard_stopped", "reason", err.Error())
		os.Exit(1)
	}
}
