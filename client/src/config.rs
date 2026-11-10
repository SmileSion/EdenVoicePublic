use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    dsp::{InputMode, NoiseSuppressionMode, NoiseSuppressionStrength},
    network::{AudioQualityPolicy, RoomNetworkPolicy},
};

const CURRENT_AUDIO_TUNING_REVISION: u8 = 1;

fn legacy_audio_tuning_revision() -> u8 {
    0
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Chinese,
    English,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedServer {
    pub display_name: String,
    pub server_name: String,
    pub address: String,
    pub nickname: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutBindings {
    pub toggle_window: String,
    pub toggle_mic: String,
    pub toggle_speaker: String,
    pub push_to_talk: String,
    pub toggle_members: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub share_game_activity: bool,
    pub remembered_user_volumes: std::collections::BTreeMap<String, f32>,
    pub mpv_path: String,
    pub nickname: String,
    pub avatar: String,
    pub last_server: String,
    pub last_successful_server: String,
    pub last_successful_nickname: String,
    pub auto_connect_last_server: bool,
    pub auto_connect_server: String,
    pub saved_servers: Vec<SavedServer>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_gain: f32,
    pub output_volume: f32,
    pub theme: ThemeMode,
    pub language: Language,
    pub mini_overlay_enabled: bool,
    pub mini_overlay_mode: String,
    pub mini_overlay_position_x: Option<i32>,
    pub mini_overlay_position_y: Option<i32>,
    pub input_mode: InputMode,
    pub shortcuts: ShortcutBindings,
    #[serde(default, rename = "ptt_key", skip_serializing)]
    legacy_ptt_key: String,
    pub echo_cancellation: bool,
    pub automatic_gain: bool,
    // Older saved configs without this field keep the historical mode. Only
    // a fresh config created through Default gets the new-install preference.
    #[serde(default)]
    pub noise_suppression_mode: NoiseSuppressionMode,
    pub noise_suppression_strength: NoiseSuppressionStrength,
    pub room_network_policy: RoomNetworkPolicy,
    pub audio_quality_policy: AudioQualityPolicy,
    #[serde(default = "legacy_audio_tuning_revision")]
    audio_tuning_revision: u8,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            share_game_activity: true,
            remembered_user_volumes: Default::default(),
            mpv_path: String::new(),
            nickname: String::new(),
            avatar: String::new(),
            last_server: String::new(),
            last_successful_server: String::new(),
            last_successful_nickname: String::new(),
            auto_connect_last_server: true,
            auto_connect_server: String::new(),
            saved_servers: Vec::new(),
            input_device: None,
            output_device: None,
            input_gain: 1.0,
            output_volume: 0.8,
            theme: ThemeMode::System,
            language: Language::Chinese,
            mini_overlay_enabled: false,
            mini_overlay_mode: "avatars".into(),
            mini_overlay_position_x: None,
            mini_overlay_position_y: None,
            input_mode: InputMode::VoiceActivity,
            shortcuts: ShortcutBindings::default(),
            legacy_ptt_key: String::new(),
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::RnNoise,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
            room_network_policy: RoomNetworkPolicy::Auto,
            audio_quality_policy: AudioQualityPolicy::Auto,
            audio_tuning_revision: CURRENT_AUDIO_TUNING_REVISION,
        }
    }
}

impl ClientConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let mut config: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        config.migrate_saved_servers();
        config.migrate_shortcuts();
        config.migrate_audio_tuning();
        config.output_volume = config.output_volume.clamp(0.01, 3.0);
        config
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path =
            config_path().ok_or_else(|| anyhow::anyhow!("config directory is unavailable"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub(crate) fn decode_backup(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(bytes.len() <= 1024 * 1024, "backup settings are too large");
        let mut config: Self = serde_json::from_slice(bytes)?;
        config.migrate_saved_servers();
        config.migrate_shortcuts();
        config.migrate_audio_tuning();
        config.input_gain = config.input_gain.clamp(0.0, 3.0);
        config.output_volume = config.output_volume.clamp(0.01, 3.0);
        anyhow::ensure!(config.input_gain.is_finite(), "invalid input gain");
        anyhow::ensure!(config.output_volume.is_finite(), "invalid output volume");
        anyhow::ensure!(
            config.nickname.chars().count() <= 64,
            "nickname is too long"
        );
        anyhow::ensure!(config.saved_servers.len() <= 8, "too many saved servers");
        anyhow::ensure!(
            matches!(config.mini_overlay_mode.as_str(), "avatars" | "list"),
            "invalid mini overlay mode"
        );
        Ok(config)
    }

    pub fn remember_server(&mut self, address: String, nickname: String, server_name: String) {
        let address = address.trim().to_owned();
        let nickname = nickname.trim().to_owned();
        if address.is_empty() {
            return;
        }
        let existing_name = self
            .saved_servers
            .iter()
            .find(|profile| profile.address == address)
            .map(|profile| profile.server_name.clone())
            .unwrap_or_default();
        let existing_display_name = self
            .saved_servers
            .iter()
            .find(|profile| profile.address == address)
            .map(|profile| profile.display_name.clone())
            .unwrap_or_default();
        self.saved_servers
            .retain(|profile| profile.address != address);
        self.saved_servers.insert(
            0,
            SavedServer {
                display_name: existing_display_name,
                server_name: if server_name.trim().is_empty() {
                    existing_name
                } else {
                    server_name.trim().to_owned()
                },
                address: address.clone(),
                nickname: nickname.clone(),
            },
        );
        self.saved_servers.truncate(8);
        self.last_server = address;
        self.nickname = nickname;
    }

    pub fn record_successful_connection(
        &mut self,
        address: String,
        nickname: String,
        server_name: String,
        remember: bool,
    ) {
        let address = address.trim().to_owned();
        let nickname = nickname.trim().to_owned();
        if address.is_empty() || nickname.is_empty() {
            return;
        }
        if remember {
            self.remember_server(address.clone(), nickname.clone(), server_name);
        }
        self.last_server = address.clone();
        self.nickname = nickname.clone();
        self.last_successful_server = address;
        self.last_successful_nickname = nickname;
    }

    pub fn update_saved_server(&mut self, index: usize, mut profile: SavedServer) -> bool {
        if index >= self.saved_servers.len() || profile.address.trim().is_empty() {
            return false;
        }
        let authoritative_name = if self.saved_servers[index].address == profile.address.trim() {
            self.saved_servers[index].server_name.clone()
        } else {
            String::new()
        };
        profile.display_name = profile.display_name.trim().to_owned();
        profile.server_name = if profile.server_name.trim().is_empty() {
            authoritative_name
        } else {
            profile.server_name.trim().to_owned()
        };
        profile.address = profile.address.trim().to_owned();
        profile.nickname = profile.nickname.trim().to_owned();
        self.saved_servers[index] = profile;
        true
    }

    pub fn add_saved_server(&mut self, mut profile: SavedServer) -> bool {
        profile.display_name = profile.display_name.trim().to_owned();
        profile.server_name = profile.server_name.trim().to_owned();
        profile.address = profile.address.trim().to_owned();
        profile.nickname = profile.nickname.trim().to_owned();
        if profile.address.is_empty() {
            return false;
        }
        if let Some(index) = self
            .saved_servers
            .iter()
            .position(|saved| saved.address == profile.address)
        {
            if profile.server_name.is_empty() {
                profile.server_name = self.saved_servers[index].server_name.clone();
            }
            self.saved_servers[index] = profile;
        } else {
            self.saved_servers.insert(0, profile);
            self.saved_servers.truncate(8);
        }
        true
    }

    pub fn delete_saved_server(&mut self, index: usize) -> Option<SavedServer> {
        (index < self.saved_servers.len()).then(|| self.saved_servers.remove(index))
    }

    fn migrate_saved_servers(&mut self) {
        if self.saved_servers.is_empty() && !self.last_server.trim().is_empty() {
            self.saved_servers.push(SavedServer {
                display_name: String::new(),
                server_name: String::new(),
                address: self.last_server.trim().to_owned(),
                nickname: self.nickname.trim().to_owned(),
            });
        }
    }

    fn migrate_shortcuts(&mut self) {
        let legacy_ptt_key = std::mem::take(&mut self.legacy_ptt_key);
        let legacy_ptt_key = legacy_ptt_key.trim();
        if self.shortcuts.push_to_talk.is_empty()
            && !legacy_ptt_key.is_empty()
            && !legacy_ptt_key.eq_ignore_ascii_case("Backquote")
        {
            self.shortcuts.push_to_talk = legacy_ptt_key.to_owned();
        }
    }

    fn migrate_audio_tuning(&mut self) {
        if self.audio_tuning_revision >= CURRENT_AUDIO_TUNING_REVISION {
            return;
        }

        // Only replace the exact legacy defaults. Any deliberate user change
        // to gain, AEC, AGC, denoiser, or strength must remain authoritative.
        let still_using_legacy_defaults = self.input_gain == 1.0
            && self.echo_cancellation
            && self.automatic_gain
            && self.noise_suppression_mode == NoiseSuppressionMode::Standard
            && self.noise_suppression_strength == NoiseSuppressionStrength::Medium;
        if still_using_legacy_defaults {
            self.echo_cancellation = false;
            self.automatic_gain = false;
            self.noise_suppression_strength = NoiseSuppressionStrength::Low;
        }
        self.audio_tuning_revision = CURRENT_AUDIO_TUNING_REVISION;
    }
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "EdenVoice", "EdenVoice")
        .map(|dirs| dirs.config_dir().join("config.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_round_trips_and_defaults_to_system() {
        assert_eq!(ClientConfig::default().theme, ThemeMode::System);
        let json = serde_json::to_string(&ThemeMode::Dark).unwrap();
        assert_eq!(json, "\"dark\"");
        assert_eq!(
            serde_json::from_str::<ThemeMode>(&json).unwrap(),
            ThemeMode::Dark
        );
    }

    #[test]
    fn language_defaults_to_chinese_and_shortcuts_are_unbound() {
        let config = ClientConfig::default();
        assert_eq!(config.language, Language::Chinese);
        assert_eq!(config.shortcuts, ShortcutBindings::default());
        let decoded: ClientConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(decoded.language, Language::Chinese);
        assert_eq!(decoded.shortcuts, ShortcutBindings::default());
    }

    #[test]
    fn fresh_install_has_no_server_address_or_saved_profiles() {
        let config = ClientConfig::default();
        assert!(config.last_server.is_empty());
        assert!(config.last_successful_server.is_empty());
        assert!(config.last_successful_nickname.is_empty());
        assert!(config.auto_connect_last_server);
        assert!(config.saved_servers.is_empty());

        let mut decoded: ClientConfig = serde_json::from_str("{}").unwrap();
        decoded.migrate_saved_servers();
        assert!(decoded.last_server.is_empty());
        assert!(decoded.last_successful_server.is_empty());
        assert!(decoded.last_successful_nickname.is_empty());
        assert!(decoded.auto_connect_last_server);
        assert!(decoded.saved_servers.is_empty());
    }

    #[test]
    fn successful_connection_enables_the_next_startup_without_forcing_a_saved_profile() {
        let mut config = ClientConfig::default();
        config.record_successful_connection(
            "voice.test:9000".into(),
            "Alice".into(),
            "Vocal Hub".into(),
            false,
        );
        assert_eq!(config.last_server, "voice.test:9000");
        assert_eq!(config.nickname, "Alice");
        assert_eq!(config.last_successful_server, "voice.test:9000");
        assert_eq!(config.last_successful_nickname, "Alice");
        assert!(config.saved_servers.is_empty());
    }

    #[test]
    fn fresh_audio_defaults_use_rnnoise_medium_suppression() {
        let config = ClientConfig::default();
        assert!(!config.echo_cancellation);
        assert!(!config.automatic_gain);
        assert_eq!(config.noise_suppression_mode, NoiseSuppressionMode::RnNoise);
        assert_eq!(
            config.noise_suppression_strength,
            NoiseSuppressionStrength::Medium
        );
        assert_eq!(config.audio_quality_policy, AudioQualityPolicy::Auto);
        assert_eq!(config.audio_tuning_revision, CURRENT_AUDIO_TUNING_REVISION);
    }

    #[test]
    fn mpv_path_survives_configuration_round_trip() {
        let config = ClientConfig {
            mpv_path: r"C:\播放器\mpv.exe".into(),
            ..ClientConfig::default()
        };
        let saved = serde_json::to_vec_pretty(&config).unwrap();
        let restored: ClientConfig = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.mpv_path, config.mpv_path);
        let legacy: ClientConfig = serde_json::from_str("{}").unwrap();
        assert!(legacy.mpv_path.is_empty());
    }
    #[test]
    fn saved_denoising_choices_survive_loading_and_backup_restore() {
        for mode in [
            NoiseSuppressionMode::Off,
            NoiseSuppressionMode::Standard,
            NoiseSuppressionMode::RnNoise,
            NoiseSuppressionMode::DeepFilterNet,
            NoiseSuppressionMode::DeepFilterNetStrong,
        ] {
            let config = ClientConfig {
                noise_suppression_mode: mode,
                ..ClientConfig::default()
            };
            let bytes = serde_json::to_vec(&config).unwrap();
            let mut loaded: ClientConfig = serde_json::from_slice(&bytes).unwrap();
            loaded.migrate_audio_tuning();
            assert_eq!(loaded.noise_suppression_mode, mode);
            assert_eq!(
                ClientConfig::decode_backup(&bytes)
                    .unwrap()
                    .noise_suppression_mode,
                mode
            );
        }
    }

    #[test]
    fn legacy_settings_without_a_denoising_field_keep_standard_mode() {
        let mut config: ClientConfig = serde_json::from_str("{}").unwrap();
        config.migrate_audio_tuning();
        assert_eq!(
            config.noise_suppression_mode,
            NoiseSuppressionMode::Standard
        );
    }

    #[test]
    fn legacy_manual_sample_rates_are_ignored_and_not_persisted() {
        let config: ClientConfig =
            serde_json::from_str(r#"{"input_sample_rate":44100,"output_sample_rate":96000}"#)
                .unwrap();
        let saved = serde_json::to_string(&config).unwrap();
        assert!(!saved.contains("input_sample_rate"));
        assert!(!saved.contains("output_sample_rate"));
    }

    #[test]
    fn legacy_audio_defaults_migrate_once_to_the_natural_voice_profile() {
        let mut config: ClientConfig = serde_json::from_str(
            r#"{
                "input_gain": 1.0,
                "echo_cancellation": true,
                "automatic_gain": true,
                "noise_suppression_mode": "standard",
                "noise_suppression_strength": "medium"
            }"#,
        )
        .unwrap();
        assert_eq!(config.audio_tuning_revision, 0);
        config.migrate_audio_tuning();
        assert!(!config.echo_cancellation);
        assert!(!config.automatic_gain);
        assert_eq!(
            config.noise_suppression_strength,
            NoiseSuppressionStrength::Low
        );
        assert_eq!(config.audio_tuning_revision, CURRENT_AUDIO_TUNING_REVISION);
    }

    #[test]
    fn audio_tuning_migration_preserves_any_custom_processing_choice() {
        let mut config: ClientConfig = serde_json::from_str(
            r#"{
                "input_gain": 1.0,
                "echo_cancellation": true,
                "automatic_gain": false,
                "noise_suppression_mode": "standard",
                "noise_suppression_strength": "medium"
            }"#,
        )
        .unwrap();
        config.migrate_audio_tuning();
        assert!(config.echo_cancellation);
        assert!(!config.automatic_gain);
        assert_eq!(
            config.noise_suppression_strength,
            NoiseSuppressionStrength::Medium
        );
        assert_eq!(config.audio_tuning_revision, CURRENT_AUDIO_TUNING_REVISION);
    }

    #[test]
    fn legacy_default_ptt_key_is_removed_instead_of_becoming_a_default_shortcut() {
        let mut config: ClientConfig = serde_json::from_str(r#"{"ptt_key":"Backquote"}"#).unwrap();
        config.migrate_shortcuts();
        assert!(config.shortcuts.push_to_talk.is_empty());
        assert!(!serde_json::to_string(&config).unwrap().contains("ptt_key"));
    }

    #[test]
    fn legacy_custom_ptt_key_migrates_to_the_new_shortcut_binding() {
        let mut config: ClientConfig = serde_json::from_str(r#"{"ptt_key":"Space"}"#).unwrap();
        config.migrate_shortcuts();
        assert_eq!(config.shortcuts.push_to_talk, "Space");
        assert!(!serde_json::to_string(&config).unwrap().contains("ptt_key"));
    }

    #[test]
    fn new_shortcut_binding_wins_over_a_stale_legacy_value() {
        let mut config: ClientConfig = serde_json::from_str(
            r#"{"ptt_key":"Space","shortcuts":{"push_to_talk":"Control+KeyV"}}"#,
        )
        .unwrap();
        config.migrate_shortcuts();
        assert_eq!(config.shortcuts.push_to_talk, "Control+KeyV");
    }

    #[test]
    fn remembered_servers_are_deduplicated_and_keep_the_server_name() {
        let mut config = ClientConfig::default();
        config.remember_server("voice.test:9000".into(), "Alice".into(), "Vocal Hub".into());
        config.remember_server("voice.test:9000".into(), "Bob".into(), String::new());
        assert_eq!(config.saved_servers.len(), 1);
        assert_eq!(config.saved_servers[0].server_name, "Vocal Hub");
        assert_eq!(config.saved_servers[0].nickname, "Bob");
    }

    #[test]
    fn editing_a_profile_keeps_the_authoritative_server_name() {
        let mut config = ClientConfig::default();
        config.remember_server("voice.test:9000".into(), "Alice".into(), "Vocal Hub".into());
        assert!(config.update_saved_server(
            0,
            SavedServer {
                display_name: "游戏开黑".into(),
                server_name: String::new(),
                address: "voice.test:9000".into(),
                nickname: "Bob".into(),
            }
        ));
        assert_eq!(config.saved_servers[0].display_name, "游戏开黑");
        assert_eq!(config.saved_servers[0].server_name, "Vocal Hub");
    }

    #[test]
    fn managed_servers_can_be_added_deduplicated_and_deleted() {
        let mut config = ClientConfig::default();
        assert!(config.add_saved_server(SavedServer {
            display_name: "  游戏服  ".into(),
            address: " voice.test:9000 ".into(),
            nickname: " Alice ".into(),
            ..Default::default()
        }));
        assert!(config.add_saved_server(SavedServer {
            display_name: "工作服".into(),
            address: "voice.test:9000".into(),
            nickname: "Bob".into(),
            ..Default::default()
        }));
        assert_eq!(config.saved_servers.len(), 1);
        assert_eq!(config.saved_servers[0].display_name, "工作服");
        assert_eq!(config.saved_servers[0].nickname, "Bob");
        assert!(config.delete_saved_server(0).is_some());
        assert!(config.saved_servers.is_empty());
        assert!(config.delete_saved_server(0).is_none());
    }

    #[test]
    fn avatar_defaults_compatibly_and_persists_a_selection() {
        let legacy: ClientConfig = serde_json::from_str("{}").unwrap();
        assert!(legacy.avatar.is_empty());
        let config = ClientConfig {
            avatar: "E".into(),
            ..Default::default()
        };
        let restored: ClientConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.avatar, "E");
    }

    #[test]
    fn mini_overlay_is_disabled_by_default_and_round_trips_its_mode() {
        let legacy: ClientConfig = serde_json::from_str("{}").unwrap();
        assert!(!legacy.mini_overlay_enabled);
        assert_eq!(legacy.mini_overlay_mode, "avatars");
        assert_eq!(legacy.mini_overlay_position_x, None);
        assert_eq!(legacy.mini_overlay_position_y, None);
        let config = ClientConfig {
            mini_overlay_enabled: true,
            mini_overlay_mode: "list".into(),
            mini_overlay_position_x: Some(-420),
            mini_overlay_position_y: Some(180),
            ..Default::default()
        };
        let restored: ClientConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert!(restored.mini_overlay_enabled);
        assert_eq!(restored.mini_overlay_mode, "list");
        assert_eq!(restored.mini_overlay_position_x, Some(-420));
        assert_eq!(restored.mini_overlay_position_y, Some(180));
    }
}

#[cfg(test)] mod game_activity_tests {
 #[test] fn sharing_defaults_on_and_preserves_opt_out() {
  let mut config:super::ClientConfig=serde_json::from_str("{}").unwrap();
  assert!(config.share_game_activity);
  config.share_game_activity=false;
  let loaded:super::ClientConfig=serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
  assert!(!loaded.share_game_activity);
 }
}

#[cfg(test)] mod remembered_volume_tests {
 #[test] fn volume_preferences_survive_reload_and_isolate_server_and_identity(){
  let mut c=super::ClientConfig::default();
  let key=serde_json::to_string(&("server|fingerprint","stable-client-id")).unwrap();
  c.remembered_user_volumes.insert(key.clone(),0.42);
  let loaded:super::ClientConfig=serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
  assert_eq!(loaded.remembered_user_volumes.get(&key),Some(&0.42));
  assert!(!loaded.remembered_user_volumes.contains_key(&serde_json::to_string(&("other-server|fingerprint","stable-client-id")).unwrap()));
  let legacy:super::ClientConfig=serde_json::from_str("{}").unwrap();assert!(legacy.remembered_user_volumes.is_empty());
 }
}

#[cfg(test)]
mod extended_volume_tests {
    use super::*;

    #[test]
    fn backup_preserves_three_hundred_percent_volumes() {
        let mut config = ClientConfig::default();
        config.input_gain = 3.0;
        config.output_volume = 3.0;
        config.remembered_user_volumes.insert("member".into(), 3.0);
        let restored = ClientConfig::decode_backup(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(restored.input_gain, 3.0);
        assert_eq!(restored.output_volume, 3.0);
        assert_eq!(restored.remembered_user_volumes["member"], 3.0);
    }
}
