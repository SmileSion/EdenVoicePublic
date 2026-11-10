// admin-demo runs the real web console with isolated sample identities and
// simulated voice sessions. It never connects to a production server.
package main

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"image"
	"image/color"
	"image/draw"
	"image/jpeg"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/control"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
	"edenvoice/server/internal/webadmin"
)

type sender struct{}

func (sender) Send(protocol.Envelope) error { return nil }
func main() {
	password := os.Getenv("EDENVOICE_DEMO_PASSWORD")
	if len(password) < 12 {
		fmt.Fprintln(os.Stderr, "Set EDENVOICE_DEMO_PASSWORD to a 12–72 byte local demo password")
		os.Exit(1)
	}
	dir, err := os.MkdirTemp("", "edenvoice-admin-demo-")
	must(err)
	defer os.RemoveAll(dir)
	cfg := config.Default()
	cfg.ServerName = "EdenVoice · 本机演示"
	cfg.Security.DataDir = dir
	cfg.Channels[0].Name = "大厅 · 自由交流"
	cfg.Channels[1].Name = "游戏开黑"
	cfg.Channels[2].Name = "周末放映室"
	state := serverstate.New(cfg)
	store, token, err := administration.LoadOrCreate(filepath.Join(dir, "administration.json"), state.Snapshot().Channels)
	must(err)
	ids := []string{"demo-master", "demo-admin", "demo-lin", "demo-ze", "demo-moon", "demo-offline"}
	names := []string{"SmileSion", "小鹿", "小林", "阿泽", "Moon", "离线成员"}
	sessions := []uint64{}
	for i, id := range ids {
		_, err = store.Admit(id, "demo-public-key", names[i], time.Now())
		must(err)
		must(store.UpdateAvatar(id, avatar(i), time.Now()))
		if i == 0 {
			must(store.ProvisionMaster(id, token))
		}
		if i == 1 {
			must(store.SetServerRole(id, administration.RoleAdmin))
		}
		if i < 5 {
			u, _ := store.User(id)
			x := state.AddAuthenticatedSession(names[i], id, string(u.ServerRole), "", sender{}, nil, nil)
			if i == 0 {
				state.SetClientBuild(x.ID, "0.8.1", "windows")
			}
			if i == 1 {
				state.SetClientBuild(x.ID, "0.8.0", "macos")
			}
			state.UpdateAvatar(id, u.Avatar)
			must(state.Join(x.ID, cfg.Channels[i%3].ID))
			sessions = append(sessions, x.ID)
		}
	}
	must(store.SetChannelRole(1, "demo-lin", administration.RoleChannelAdmin))
	accounts, err := webadmin.OpenAccounts(filepath.Join(dir, "web-accounts.json"))
	must(err)
	must(accounts.Put("admin", "demo-master", password, false, false))
	must(accounts.Put("operator", "demo-admin", password, false, false))
	must(accounts.Put("viewer", "demo-admin", password, true, false))
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	ctrl := control.NewSecure("127.0.0.1:0", "127.0.0.1:0", state, logger, nil, nil, false, store)
	must(ctrl.ConfigureScreenSharing(cfg.ScreenShare, filepath.Join(dir, "screen-sharing.json")))
	defer ctrl.CloseScreenSharing()
	addr := os.Getenv("EDENVOICE_DEMO_ADDR")
	if addr == "" {
		addr = "127.0.0.1:9181"
	}
	relay := voice.New("127.0.0.1:0", state, logger)
	web, err := webadmin.New(config.WebAdmin{Enabled: true, ListenAddr: addr}, ctrl, relay, accounts, logger)
	must(err)
	go func() {
		t := time.NewTicker(5 * time.Second)
		defer t.Stop()
		for range t.C {
			for _, id := range sessions {
				_ = state.Touch(id)
			}
		}
	}()
	fmt.Println("Demo: http://" + addr + " · accounts admin / operator / viewer · simulated voice members; actual web API and host metrics")
	if os.Getenv("EDENVOICE_GAME_DEMO") == "1" {
		must(http.ListenAndServe(addr, gameDemoHandler(web.Handler(), state, sessions)))
		return
	}
	must(web.ListenAndServe())
}
func must(err error) {
	if err != nil {
		panic(err)
	}
}
func avatar(i int) string {
	palette := []color.RGBA{{174, 156, 216, 255}, {122, 182, 164, 255}, {203, 170, 141, 255}, {126, 154, 193, 255}}
	bg := palette[i%len(palette)]
	img := image.NewRGBA(image.Rect(0, 0, 128, 128))
	draw.Draw(img, img.Bounds(), &image.Uniform{bg}, image.Point{}, draw.Src)
	for y := 0; y < 128; y++ {
		for x := 0; x < 128; x++ {
			dx, dy := x-64, y-45
			if dx*dx+dy*dy < 24*24 {
				img.Set(x, y, color.RGBA{245, 233, 222, 255})
			}
			dx, dy = x-64, y-135
			if dx*dx+dy*dy < 54*54 {
				img.Set(x, y, color.RGBA{67, 65, 88, 255})
			}
		}
	}
	var out bytes.Buffer
	_ = jpeg.Encode(&out, img, &jpeg.Options{Quality: 80})
	return "data:image/jpeg;base64," + base64.StdEncoding.EncodeToString(out.Bytes())
}
