package main

import (
	"context"
	"fmt"
	"github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/common"
	monitor "github.com/tencentcloud/tencentcloud-sdk-go/tencentcloud/monitor/v20180724"
	"log/slog"
	"strings"
	"time"
)

// Read-only diagnostic: records metric metadata and aggregate counts, never credentials.
func (c *cloud) diagnose(ctx context.Context) error {
	now := time.Now().In(cst)
	mid := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, cst)
	for _, metric := range []string{"InternetTrafficDown", "InternetTraffic", "InternetTrafficUp"} {
		for _, legacy := range []bool{false, true} {
			for _, w := range [][2]time.Time{{mid.AddDate(0, 0, -1), mid}, {mid, now}} {
				r := monitor.NewGetMonitorDataRequest()
				r.Namespace = common.StringPtr("QCE/COS")
				r.MetricName = common.StringPtr(metric)
				r.Period = common.Uint64Ptr(300)
				r.StartTime = common.StringPtr(w[0].Format(time.RFC3339))
				r.EndTime = common.StringPtr(w[1].Format(time.RFC3339))
				dims := []*monitor.Dimension{{Name: common.StringPtr("bucket"), Value: common.StringPtr(c.s.Bucket)}}
				if legacy {
					dims = append(dims, &monitor.Dimension{Name: common.StringPtr("appid"), Value: common.StringPtr(c.s.Bucket[strings.LastIndex(c.s.Bucket, "-")+1:])})
				}
				r.Instances = []*monitor.Instance{{Dimensions: dims}}
				res, e := c.monitor.GetMonitorDataWithContext(ctx, r)
				if e != nil {
					slog.Error("diagnostic_request_failed", "metric", metric, "appid_dimension", legacy)
					continue
				}
				if res.Response == nil {
					return fmt.Errorf("empty monitor response")
				}
				total, e := sumTraffic(res.Response, c.s.Bucket, w[0], w[1])
				points, nonzero := 0, 0
				var raw float64
				for _, p := range res.Response.DataPoints {
					if p == nil {
						continue
					}
					points += len(p.Values)
					for _, v := range p.Values {
						if v != nil {
							raw += *v
							if *v != 0 {
								nonzero++
							}
						}
					}
				}
				slog.Info("diagnostic", "metric", metric, "appid_dimension", legacy, "day", w[0].Format("2006-01-02"), "points", points, "nonzero", nonzero, "raw_sum", raw, "filtered_sum", total, "valid", e == nil)
			}
		}
	}
	p, e := c.Policy(ctx)
	if e != nil {
		slog.Error("policy_read_failed", "reason", e.Error())
	} else {
		slog.Info("policy_read_ok", "exists", p != nil)
	}
	return nil
}
