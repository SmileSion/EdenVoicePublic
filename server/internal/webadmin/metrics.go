package webadmin

import (
	"bufio"
	"os"
	"path/filepath"
	"runtime"
	rm "runtime/metrics"
	"strconv"
	"strings"
	"time"

	"edenvoice/server/internal/voice"
)

type Sample struct {
	At            time.Time             `json:"at"`
	Uptime        float64               `json:"uptime_seconds"`
	CPUSeconds    float64               `json:"go_cpu_seconds"`
	CPUPercent    *float64              `json:"go_cpu_percent"`
	Heap          uint64                `json:"heap_bytes"`
	Goroutines    int                   `json:"goroutines"`
	GOMAXPROCS    int                   `json:"gomaxprocs"`
	RXBPS         float64               `json:"rx_bps"`
	TXBPS         float64               `json:"tx_bps"`
	RSS           *uint64               `json:"rss_bytes"`
	HostMemory    *uint64               `json:"host_memory_bytes"`
	HostAvailable *uint64               `json:"host_available_bytes"`
	DiskFree      *uint64               `json:"disk_free_bytes"`
	DiskTotal     *uint64               `json:"disk_total_bytes"`
	Load1         *float64              `json:"host_load_1"`
	Media         voice.MetricsSnapshot `json:"media"`
}

func (s *Server) performance() []Sample {
	s.metricsMu.Lock()
	defer s.metricsMu.Unlock()
	now := time.Now()
	if now.Sub(s.metricsAt) < 5*time.Second {
		return append([]Sample{}, s.metricsHistory...)
	}
	sample := Sample{At: now, Uptime: now.Sub(s.started).Seconds(), Goroutines: runtime.NumGoroutine(), GOMAXPROCS: runtime.GOMAXPROCS(0), Media: s.relay.Metrics()}
	stats := []rm.Sample{{Name: "/cpu/classes/user:cpu-seconds"}, {Name: "/cpu/classes/gc/total:cpu-seconds"}, {Name: "/cpu/classes/scavenge/total:cpu-seconds"}, {Name: "/memory/classes/heap/objects:bytes"}}
	rm.Read(stats)
	for _, x := range stats {
		if x.Value.Kind() == rm.KindFloat64 {
			sample.CPUSeconds += x.Value.Float64()
		} else if x.Value.Kind() == rm.KindUint64 {
			sample.Heap = x.Value.Uint64()
		}
	}
	if len(s.metricsHistory) > 0 {
		last := s.metricsHistory[len(s.metricsHistory)-1]
		dt := now.Sub(last.At).Seconds()
		v := 100 * (sample.CPUSeconds - last.CPUSeconds) / dt
		sample.CPUPercent = &v
		sample.RXBPS = float64(sample.Media.RXBytes-last.Media.RXBytes) * 8 / dt
		sample.TXBPS = float64(sample.Media.TXBytes-last.Media.TXBytes) * 8 / dt
	}
	if runtime.GOOS == "linux" {
		sample.RSS = procKB("/proc/self/status", "VmRSS:")
		sample.HostMemory = procKB("/proc/meminfo", "MemTotal:")
		sample.HostAvailable = procKB("/proc/meminfo", "MemAvailable:")
		if raw, err := os.ReadFile("/proc/loadavg"); err == nil {
			f := strings.Fields(string(raw))
			if len(f) > 0 {
				v, e := strconv.ParseFloat(f[0], 64)
				if e == nil {
					sample.Load1 = &v
				}
			}
		}
	}
	sample.DiskFree, sample.DiskTotal = diskSpace(filepath.Dir(s.accounts.path))
	s.metricsHistory = append(s.metricsHistory, sample)
	if len(s.metricsHistory) > 120 {
		s.metricsHistory = append([]Sample(nil), s.metricsHistory[len(s.metricsHistory)-120:]...)
	}
	s.metricsAt = now
	return append([]Sample{}, s.metricsHistory...)
}
func procKB(path, key string) *uint64 {
	f, e := os.Open(path)
	if e != nil {
		return nil
	}
	defer f.Close()
	scan := bufio.NewScanner(f)
	for scan.Scan() {
		parts := strings.Fields(scan.Text())
		if len(parts) >= 2 && parts[0] == key {
			v, e := strconv.ParseUint(parts[1], 10, 64)
			if e == nil {
				v *= 1024
				return &v
			}
		}
	}
	return nil
}
