package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/announcements"
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/chatstore"
	"edenvoice/server/internal/clientupdates"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/control"
	"edenvoice/server/internal/gamecatalog"
	"edenvoice/server/internal/observability"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/security"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/voice"
	"edenvoice/server/internal/webadmin"
)

const version = "0.8.5"

func main() {
	showVersion := flag.Bool("version", false, "print server version and exit")
	configPath := flag.String("config", "", "path to TOML config (defaults are used when omitted)")
	recoverMasterClientID := flag.String("recover-master-client-id", "", "locally bind MASTER to an existing ClientID and exit")
	webInit := flag.String("web-admin-init", "", "create or reset a web username using a password from stdin, then exit")
	webClientID := flag.String("web-admin-client-id", "", "existing MASTER/ADMIN ClientID bound to this web account (defaults to current MASTER)")
	flag.Parse()
	if *showVersion {
		fmt.Println("EdenVoice server v" + version)
		return
	}
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	logger.Info("EdenVoice server starting", "version", version)
	cfg, err := config.Load(*configPath)
	if err != nil {
		logger.Error("configuration failed", "error", err)
		os.Exit(1)
	}
	initialChannels := configuredChannels(cfg.Channels)
	administrationStore, masterToken, err := administration.LoadOrCreateWithToken(filepath.Join(cfg.Security.DataDir, "administration.json"), initialChannels, cfg.Security.MasterProvisionToken)
	if err != nil {
		logger.Error("administration persistence failed", "error", err)
		os.Exit(1)
	}
	if masterToken != "" {
		logger.Warn("MASTER PROVISION TOKEN (shown once)", "token", masterToken)
		securityAudit(logger, slog.LevelWarn, "master_provision_token_generated")
	}
	if *recoverMasterClientID != "" {
		if err := administrationStore.RecoverMaster(*recoverMasterClientID); err != nil {
			logger.Error("MASTER recovery failed", "error", err)
			os.Exit(1)
		}
		logger.Info("MASTER identity recovered", "client_id", *recoverMasterClientID)
		securityAudit(logger, slog.LevelInfo, "master_identity_recovered", "target_client_id", *recoverMasterClientID, "operator", "local")
		return
	}
	if *webInit != "" {
		clientID := *webClientID
		if clientID == "" {
			for _, u := range administrationStore.Profiles() {
				if u.ServerRole == administration.RoleMaster {
					clientID = u.ClientID
					break
				}
			}
		}
		u, ok := administrationStore.User(clientID)
		if !ok || (u.ServerRole != administration.RoleMaster && u.ServerRole != administration.RoleAdmin) {
			logger.Error("bind an existing MASTER or ADMIN identity; provision MASTER in the voice client first")
			os.Exit(1)
		}
		accounts, e := webadmin.OpenAccounts(filepath.Join(cfg.Security.DataDir, "web-accounts.json"))
		if e != nil {
			logger.Error("web account store failed", "error", e)
			os.Exit(1)
		}
		fmt.Fprintln(os.Stderr, "Read password from stdin (12–72 bytes; do not use a command-line password argument):")
		password, e := io.ReadAll(io.LimitReader(os.Stdin, 1024))
		if e == nil {
			e = accounts.Put(*webInit, clientID, strings.TrimSuffix(strings.TrimSuffix(string(password), "\n"), "\r"), false, false)
		}
		if e != nil {
			logger.Error("web account initialization failed", "error", e)
			os.Exit(1)
		}
		logger.Info("web account initialized", "username", *webInit, "client_id", clientID)
		return
	}
	cfg.Channels = restoredChannels(administrationStore.Channels())
	state := serverstate.New(cfg)
	chat, chatErr := chatstore.Open(filepath.Join(cfg.Security.DataDir, "chat"))
	if chatErr == nil {
		chatErr = state.RestoreChat(chat)
	} else {
		_ = state.RestoreChat(nil)
	}
	if chatErr != nil {
		logger.Error("chat persistence unavailable; chat sending disabled, voice will continue", "error", chatErr)
	}
	archiveCollector := startOptionalArchive(cfg, logger)
	if archiveCollector != nil {
		defer archiveCollector.Close()
		for _, channel := range cfg.Channels {
			archiveCollector.Observe(archive.Activity{Kind: archive.Created, RoomID: channel.ID, RoomName: channel.Name, At: time.Now()})
		}
	}
	identity, created, err := security.LoadOrCreateIdentity(filepath.Join(cfg.Security.DataDir, "server-identity.json"))
	if err != nil {
		logger.Error("server identity failed", "error", err)
		os.Exit(1)
	}
	if created {
		logger.Info("server identity created", "fingerprint", identity.Fingerprint())
		securityAudit(logger, slog.LevelInfo, "server_identity_created", "fingerprint", identity.Fingerprint())
	} else {
		logger.Info("server identity loaded", "fingerprint", identity.Fingerprint())
	}
	if cfg.Security.AllowLegacyInsecure {
		securityAudit(logger, slog.LevelWarn, "legacy_insecure_compatibility_enabled")
	}
	tlsConfig, err := identity.TLSConfig(cfg.ServerName)
	if err != nil {
		logger.Error("TLS configuration failed", "error", err)
		os.Exit(1)
	}
	controlServer := control.NewSecure(cfg.ControlAddr, cfg.VoiceAddr, state, logger, identity, tlsConfig, cfg.Security.AllowLegacyInsecure, administrationStore, archiveCollector)
	if err := controlServer.ConfigureRoomImages(filepath.Join(cfg.Security.DataDir, "room-images")); err != nil {
		logger.Error("room image storage unavailable; uploads disabled", "error", err)
	}
	controlServer.SetServerPassword(cfg.ServerPassword)
	catalog, catalogErr := gamecatalog.Open(filepath.Join(cfg.Security.DataDir, "game-catalog.json"))
	if catalogErr != nil {
		logger.Error("game catalog load failed", "error", catalogErr)
		os.Exit(1)
	}
	controlServer.SetGameCatalog(catalog)
	updates, updateErr := clientupdates.Open(filepath.Join(cfg.Security.DataDir, "client-updates.json"))
	if updateErr != nil {
		logger.Error("client updates load failed", "error", updateErr)
		os.Exit(1)
	}
	controlServer.SetClientUpdates(updates)
	announcementStore, announcementErr := announcements.Open(filepath.Join(cfg.Security.DataDir, "announcements.json"))
	if announcementErr != nil {
		logger.Error("announcements load failed", "error", announcementErr)
		os.Exit(1)
	}
	controlServer.SetAnnouncements(announcementStore)
	if err := controlServer.ConfigureScreenSharing(cfg.ScreenShare, filepath.Join(cfg.Security.DataDir, "screen-sharing.json")); err != nil {
		logger.Error("screen sharing disabled; voice continues", "error", err)
	}
	defer controlServer.CloseScreenSharing()
	relay := voice.New(cfg.VoiceAddr, state, logger, voice.Options{
		WorkerCount:     cfg.Voice.WorkerCount,
		WorkerQueueSize: cfg.Voice.WorkerQueueSize,
		MaxQueueDelay:   time.Duration(cfg.Voice.MaxQueueDelayMS) * time.Millisecond,
		WriterQueueSize: cfg.Voice.WriterQueueSize,
		WriteBatchSize:  cfg.Voice.WriteBatchSize,
	})
	// Console failures are isolated from ongoing voice service.
	if cfg.WebAdmin.Enabled {
		accounts, e := webadmin.OpenAccounts(filepath.Join(cfg.Security.DataDir, "web-accounts.json"))
		if e != nil {
			logger.Error("web console disabled: account store unavailable", "error", e)
		} else {
			web, e := webadmin.New(cfg.WebAdmin, controlServer, relay, accounts, logger)
			if e != nil {
				logger.Error("web console disabled", "error", e)
			} else {
				go func() {
					if e := web.ListenAndServe(); e != nil {
						logger.Error("web console stopped; voice continues", "error", e)
					}
				}()
			}
		}
	}
	errCh := make(chan error, 2)
	go func() { errCh <- relay.ListenAndServe() }()
	go func() { errCh <- controlServer.ListenAndServe() }()
	if cfg.Observability.MetricsAddr != "" {
		metricsServer := observability.New(cfg.Observability.MetricsAddr, state, relay, logger)
		go runOptionalMetrics(metricsServer.ListenAndServe, logger)
	}
	if err := <-errCh; err != nil {
		logger.Error("server stopped", "error", err)
		os.Exit(1)
	}
}

func runOptionalMetrics(listen func() error, logger *slog.Logger) {
	if err := listen(); err != nil {
		logger.Error("observability stopped; voice continues", "error", err)
	}
}

func loadArchiveCollector(cfg config.Config) (*archive.Collector, error) {
	if !cfg.Archive.Enabled {
		return nil, nil
	}
	archivePath := filepath.Join(cfg.Security.DataDir, "eden-archive.json")
	rulesPath := cfg.Archive.RulesFile
	if rulesPath == "" {
		rulesPath = filepath.Join(cfg.Security.DataDir, "eden-archive-rules.json")
	}
	return archive.LoadOrCreateWithRulesPath(archivePath, cfg.ServerName, rulesPath)
}

// Archive is optional: a corrupt or inaccessible archive must not prevent
// voice service startup. Keep the original files intact for recovery.
func startOptionalArchive(cfg config.Config, logger *slog.Logger) *archive.Collector {
	collector, err := loadArchiveCollector(cfg)
	if err != nil {
		logger.Warn("Eden Archive disabled for this process; voice service will continue", "error", err)
		return nil
	}
	return collector
}

func securityAudit(logger *slog.Logger, level slog.Level, event string, attributes ...any) {
	fields := []any{"event", event}
	fields = append(fields, attributes...)
	logger.Log(context.Background(), level, "security audit", fields...)
}

// Preserve room limits both on first initialization and on every restart.
func configuredChannels(channels []config.Channel) []protocol.Channel {
	result := make([]protocol.Channel, 0, len(channels))
	for _, c := range channels {
		result = append(result, protocol.Channel{ID: c.ID, ParentID: c.ParentID, Name: c.Name, SortOrder: c.SortOrder, UserLimit: uint32(c.UserLimit), NetworkPolicy: c.NetworkPolicy, AudioQualityPolicy: c.AudioQualityPolicy})
	}
	return result
}
func restoredChannels(channels []protocol.Channel) []config.Channel {
	result := make([]config.Channel, 0, len(channels))
	for _, c := range channels {
		result = append(result, config.Channel{ID: c.ID, ParentID: c.ParentID, Name: c.Name, SortOrder: c.SortOrder, UserLimit: int(c.UserLimit), NetworkPolicy: c.NetworkPolicy, AudioQualityPolicy: c.AudioQualityPolicy})
	}
	return result
}
