package main

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/common"
	monitor "github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/monitor/v20180724"
)

type fake struct {
	total                           float64
	p                               policy
	puts                            int
	failPut, failMonitor, failProbe bool
	windows                         [][2]time.Time
}

func (f *fake) Traffic(_ context.Context, start, end time.Time) (float64, error) {
	f.windows = append(f.windows, [2]time.Time{start, end})
	if f.failMonitor {
		return 0, errors.New("monitor failed")
	}
	return f.total, nil
}
func (f *fake) Policy(context.Context) (policy, error) { return f.p, nil }
func (f *fake) Put(_ context.Context, p policy) error {
	if f.failPut {
		return errors.New("write failed")
	}
	f.p = p
	f.puts++
	return nil
}
func (f *fake) Verify(context.Context) error {
	if f.failProbe {
		return errors.New("still public")
	}
	return nil
}
func setting() settings {
	return settings{Bucket: "edenvoice-1313309806", Region: "ap-chengdu", Limit: 500000000, ProbeKey: "releases/file.exe"}
}

var noon = time.Date(2026, 9, 10, 12, 0, 0, 0, cst)

func TestThresholdAndDryRun(t *testing.T) {
	for _, tc := range []struct {
		name    string
		total   float64
		enforce bool
		status  string
		writes  int
	}{{"below", 499999999, true, "below_threshold", 0}, {"equal", 500000000, true, "blocked_verified", 1}, {"above", 600000000, true, "blocked_verified", 1}, {"readonly", 600000000, false, "would_block", 0}} {
		t.Run(tc.name, func(t *testing.T) {
			f := &fake{total: tc.total}
			dir := t.TempDir()
			status, e := check(context.Background(), f, setting(), dir, tc.enforce, noon)
			if e != nil || status != tc.status || f.puts != tc.writes {
				t.Fatalf("%s %v writes=%d", status, e, f.puts)
			}
			if !tc.enforce {
				if _, e := os.Stat(filepath.Join(dir, "blocked.json")); !errors.Is(e, os.ErrNotExist) {
					t.Fatal("dry run persisted latch")
				}
			}
		})
	}
}
func TestRetryNextDayAndIdempotence(t *testing.T) {
	f := &fake{total: 500000000, failPut: true}
	dir := t.TempDir()
	if _, e := check(context.Background(), f, setting(), dir, true, noon); e == nil {
		t.Fatal("expected failure")
	}
	f.failPut = false
	f.total = 0
	for i := 0; i < 2; i++ {
		status, e := check(context.Background(), f, setting(), dir, true, noon.Add(48*time.Hour))
		if e != nil || status != "blocked_verified" {
			t.Fatal(status, e)
		}
	}
	if f.puts != 1 {
		t.Fatal("not idempotent")
	}
}
func TestPolicyPreservationAndConflict(t *testing.T) {
	p := policy{"version": "2.0", "Statement": []any{map[string]any{"effect": "allow", "future": map[string]any{"keep": "yes"}}}}
	f := &fake{total: 500000000, p: p}
	dir := t.TempDir()
	if _, e := check(context.Background(), f, setting(), dir, true, noon); e != nil {
		t.Fatal(e)
	}
	if !reflect.DeepEqual(f.p["Statement"].([]any)[0], p["Statement"].([]any)[0]) {
		t.Fatal("lost policy fields")
	}
	files, _ := filepath.Glob(filepath.Join(dir, "policy-before-*.json"))
	if len(files) != 1 {
		t.Fatal("missing backup")
	}
	var backup policy
	if e := loadJSON(files[0], &backup); e != nil || !reflect.DeepEqual(backup, p) {
		t.Fatal("bad backup", e)
	}
	conflict := policy{"statement": []any{map[string]any{"sid": sid, "effect": "allow"}}}
	if _, e := addBlock(conflict, setting()); e == nil {
		t.Fatal("SID collision accepted")
	}
}
func TestFailuresAndWrongState(t *testing.T) {
	for _, f := range []*fake{{failMonitor: true}, {total: 500000000, failProbe: true}} {
		if _, e := check(context.Background(), f, setting(), t.TempDir(), true, noon); e == nil {
			t.Fatal("failure falsely confirmed")
		}
	}
	dir := t.TempDir()
	if e := atomicJSON(filepath.Join(dir, "blocked.json"), trip{Bucket: "wrong", Region: "ap-chengdu"}); e != nil {
		t.Fatal(e)
	}
	if _, e := check(context.Background(), &fake{}, setting(), dir, true, noon); e == nil {
		t.Fatal("wrong bucket accepted")
	}
}
func TestCSTMidnightAndLateYesterday(t *testing.T) {
	f := &fake{}
	now := time.Date(2026, 9, 9, 16, 2, 0, 0, time.UTC)
	if _, e := check(context.Background(), f, setting(), t.TempDir(), true, now); e != nil {
		t.Fatal(e)
	}
	if len(f.windows) != 1 || f.windows[0][0].Format(time.RFC3339) != "2026-09-09T00:00:00+08:00" || f.windows[0][1].Format(time.RFC3339) != "2026-09-10T00:00:00+08:00" {
		t.Fatal(f.windows)
	}
}
func TestSumAndValidation(t *testing.T) {
	start := noon.Add(-12 * time.Hour)
	stamp := float64(start.Unix())
	r := &monitor.GetMonitorDataResponseParams{Period: common.Uint64Ptr(300), DataPoints: []*monitor.DataPoint{{Dimensions: []*monitor.Dimension{{Name: common.StringPtr("bucket"), Value: common.StringPtr(setting().Bucket)}}, Timestamps: []*float64{common.Float64Ptr(stamp - 300), common.Float64Ptr(stamp), common.Float64Ptr(stamp), common.Float64Ptr(stamp + 300)}, Values: []*float64{common.Float64Ptr(999), common.Float64Ptr(200000000), common.Float64Ptr(200000000), common.Float64Ptr(300000000)}}}}
	total, e := sumTraffic(r, setting().Bucket, start, noon)
	if e != nil || total != 500000000 {
		t.Fatal(total, e)
	}
	if _, e = sumTraffic(r, "wrong", start, noon); e == nil {
		t.Fatal("bucket mismatch")
	}
	r.DataPoints[0].Values[2] = common.Float64Ptr(1)
	if _, e = sumTraffic(r, setting().Bucket, start, noon); e == nil {
		t.Fatal("duplicate conflict")
	}
	if _, e = sumTraffic(nil, setting().Bucket, start, noon); e == nil {
		t.Fatal("missing data")
	}
}
func TestHTTPPolicyAndProbe(t *testing.T) {
	var saved policy
	publicStatus := 403
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.RawQuery == "policy" {
			if r.Method == "GET" {
				if saved == nil {
					w.WriteHeader(404)
					_, _ = w.Write([]byte("<Error><Code>NoSuchBucketPolicy</Code></Error>"))
					return
				}
				_ = json.NewEncoder(w).Encode(saved)
				return
			}
			if r.Method == "PUT" {
				if r.Header.Get("Content-MD5") == "" {
					t.Error("missing payload checksum")
				}
				if e := json.NewDecoder(r.Body).Decode(&saved); e != nil {
					t.Error(e)
				}
				w.WriteHeader(204)
				return
			}
		}
		if r.Header.Get("Range") != "bytes=0-0" {
			t.Error("probe not bounded")
		}
		w.WriteHeader(publicStatus)
	}))
	defer server.Close()
	c := &cloud{s: setting(), base: server.URL, signed: server.Client(), public: server.Client()}
	ctx := context.Background()
	p, e := c.Policy(ctx)
	if e != nil || p != nil {
		t.Fatal(p, e)
	}
	p, _ = addBlock(nil, setting())
	if e = c.Put(ctx, p); e != nil {
		t.Fatal(e)
	}
	got, e := c.Policy(ctx)
	if e != nil || !reflect.DeepEqual(got, p) {
		t.Fatal(got, e)
	}
	if e = c.Verify(ctx); e != nil {
		t.Fatal(e)
	}
	for _, status := range []int{200, 206, 302, 404, 500} {
		publicStatus = status
		if e = c.Verify(ctx); e == nil {
			t.Fatalf("accepted %d", status)
		}
	}
}
func TestCredentialPermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "credentials.json")
	if e := atomicJSON(path, credentials{SecretID: "test", SecretKey: "test"}); e != nil {
		t.Fatal(e)
	}
	if _, e := readCredentials(path); e != nil {
		t.Fatal(e)
	}
	if e := os.Chmod(path, 0644); e != nil {
		t.Fatal(e)
	}
	if _, e := readCredentials(path); e == nil {
		t.Fatal("world-readable credentials accepted")
	}
	link := path + "-link"
	if e := os.Symlink(path, link); e != nil {
		t.Fatal(e)
	}
	if _, e := readCredentials(link); e == nil {
		t.Fatal("symlink accepted")
	}
}

func TestDownloadMetricRequest(t *testing.T) {
	r := trafficRequest(setting().Bucket, noon.Add(-time.Hour), noon)
	if *r.MetricName != "InternetTraffic" || *r.Namespace != "QCE/COS" || *r.Period != 300 {
		t.Fatal("incorrect COS download metric")
	}
	if *r.StartTime != "2026-09-10T11:00:00+08:00" {
		t.Fatal("incorrect time zone")
	}
	if *r.Instances[0].Dimensions[0].Value != setting().Bucket {
		t.Fatal("incorrect bucket")
	}
}
func TestMissingPolicyCodes(t *testing.T) {
	for _, code := range []string{"NoSuchPolicy", "NoSuchBucketPolicy", "AccessDenied", "NoSuchBucket"} {
		t.Run(code, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(404)
				_, _ = w.Write([]byte("<Error><Code>" + code + "</Code></Error>"))
			}))
			defer server.Close()
			c := &cloud{base: server.URL, signed: server.Client()}
			p, e := c.Policy(context.Background())
			missing := code == "NoSuchPolicy" || code == "NoSuchBucketPolicy"
			if missing && (e != nil || p != nil) {
				t.Fatal(p, e)
			}
			if !missing && e == nil {
				t.Fatal("treated non-policy error as missing policy")
			}
		})
	}
}

func TestEmptyCOSPolicyNotFound(t *testing.T) {
	for _, cosResponse := range []bool{false, true} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if cosResponse {
				w.Header().Set("X-Cos-Request-Id", "test-request")
			}
			w.WriteHeader(404)
		}))
		c := &cloud{base: server.URL, signed: server.Client()}
		p, e := c.Policy(context.Background())
		server.Close()
		if cosResponse && (p != nil || e != nil) {
			t.Fatal(p, e)
		}
		if !cosResponse && e == nil {
			t.Fatal("unidentified empty 404 accepted")
		}
	}
}
