package protocol

import (
	_ "embed"
	"encoding/json"
	"errors"
	"regexp"
	"strings"
)

//go:embed game_catalog.json
var gameCatalogJSON []byte

type Game struct {
	ID          string   `json:"id"`
	Title       string   `json:"title"`
	Executables []string `json:"executables"`
	Enabled     bool     `json:"enabled"`
}
type GameCatalog struct {
	Version uint64 `json:"version"`
	Games   []Game `json:"games"`
}

func DefaultGames() []Game {
	var games []Game
	if err := json.Unmarshal(gameCatalogJSON, &games); err != nil {
		panic(err)
	}
	for i := range games {
		games[i].Enabled = true
	}
	return games
}
func GameTitle(id string) (string, bool) {
	for _, g := range DefaultGames() {
		if g.ID == id {
			return g.Title, true
		}
	}
	return "", false
}

var gameIDPattern = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,47}$`)

func ValidateGames(games []Game) error {
	encoded, _ := json.Marshal(games)
	if len(encoded) > 24*1024 {
		return errors.New("识别目录内容不能超过 24 KB")
	}
	if games == nil || len(games) > 64 {
		return errors.New("目录最多支持 64 款游戏")
	}
	ids, names := map[string]bool{}, map[string]bool{}
	for _, g := range games {
		if !gameIDPattern.MatchString(g.ID) || ids[g.ID] {
			return errors.New("游戏 ID 无效或重复")
		}
		ids[g.ID] = true
		if strings.TrimSpace(g.Title) != g.Title || g.Title == "" || len(g.Title) > 96 || strings.ContainsAny(g.Title, "\r\n\x00") {
			return errors.New("游戏名称应为 1–96 字节单行文字")
		}
		if len(g.Executables) == 0 || len(g.Executables) > 8 {
			return errors.New("每款游戏需要 1–8 个进程文件名")
		}
		for _, exe := range g.Executables {
			n := strings.ToLower(exe)
			if len(n) > 96 || len(n) <= 4 || !strings.HasSuffix(n, ".exe") || strings.TrimSpace(n) != n || strings.ContainsAny(n, `/\:*?"<>|`+"\r\n\x00") {
				return errors.New("仅支持精确 .exe 文件名，不支持路径或通配符")
			}
			switch n {
			case "java.exe", "javaw.exe", "steam.exe", "epicgameslauncher.exe", "riotclientservices.exe", "cmd.exe", "powershell.exe", "explorer.exe", "python.exe", "pythonw.exe", "node.exe", "rundll32.exe", "msedge.exe", "chrome.exe", "firefox.exe", "battle.net.exe":
				return errors.New("不能使用通用进程或游戏启动器")
			}
			if names[n] {
				return errors.New("进程文件名不能重复或归属多款游戏")
			}
			names[n] = true
		}
	}
	return nil
}

type GameActivityRequest struct {
	GameID string `json:"game_id"`
}
type GameActivity struct {
	GameID    string `json:"gameId"`
	Kind      string `json:"kind"`
	Title     string `json:"title"`
	Source    string `json:"source"`
	UpdatedAt int64  `json:"updatedAt"`
	ExpiresAt int64  `json:"expiresAt"`
}
type RoomScene struct {
	GameID string `json:"game_id"`
	Title  string `json:"title"`
}
