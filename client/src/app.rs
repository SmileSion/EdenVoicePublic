use std::time::Duration;

use eframe::egui::{self, Color32, RichText, Stroke};

use crate::{
    application::{
        AppCommand, Application, AudioPreferences, AudioState, ConnectionState, InputMode,
        Language, MicTestStatus, NoiseSuppressionMode, NoiseSuppressionStrength, ThemeMode,
    },
    ui,
};

pub struct EdenVoiceApp {
    application: Application,
    server_address: String,
    nickname: String,
    settings_open: bool,
    settings_section: SettingsSection,
    audio_preferences: AudioPreferences,
    selected_user: Option<u64>,
    push_to_talk_pressed: bool,
    capturing_ptt_key: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SettingsSection {
    Appearance,
    #[default]
    Audio,
    Diagnostics,
}

impl EdenVoiceApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let application = Application::new();
        ui::configure_fonts(&context.egui_ctx);
        ui::configure(&context.egui_ctx, application.theme());
        let server_address = application.server_address().to_owned();
        let nickname = application.nickname().to_owned();
        let audio_preferences = application.audio_preferences();
        Self {
            application,
            server_address,
            nickname,
            settings_open: false,
            settings_section: SettingsSection::default(),
            audio_preferences,
            selected_user: None,
            push_to_talk_pressed: false,
            capturing_ptt_key: false,
        }
    }

    fn connection_screen(&mut self, root: &mut egui::Ui) {
        let language = self.application.language();
        egui::CentralPanel::default().show(root, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                let palette = ui::theme::current(ui.ctx());
                ui.heading(RichText::new("EdenVoice").size(36.0).color(palette.accent));
                ui.label(
                    RichText::new(tr(
                        language,
                        "面向自建服务器的低延迟语音",
                        "Low-latency voice for your own server",
                    ))
                    .color(Color32::GRAY),
                );
                ui.label(
                    RichText::new(format!(
                        "v{} · Daily Driver Foundation",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .small()
                    .color(palette.text_secondary),
                );
                ui.add_space(34.0);
                egui::Frame::group(ui.style())
                    .corner_radius(14)
                    .inner_margin(24)
                    .show(ui, |ui| {
                        ui.set_width(400.0);
                        ui.label(tr(language, "服务器地址", "Server address"));
                        ui.add_sized(
                            [400.0, 38.0],
                            egui::TextEdit::singleline(&mut self.server_address)
                                .hint_text("127.0.0.1:9000"),
                        );
                        ui.add_space(12.0);
                        ui.label(tr(language, "昵称", "Nickname"));
                        ui.add_sized(
                            [400.0, 38.0],
                            egui::TextEdit::singleline(&mut self.nickname).hint_text("Alice"),
                        );
                        ui.add_space(18.0);
                        let valid = !self.server_address.trim().is_empty()
                            && !self.nickname.trim().is_empty()
                            && self.nickname.len() <= 64;
                        if ui
                            .add_enabled(
                                valid,
                                egui::Button::new(
                                    RichText::new(tr(language, "连接", "Connect")).strong(),
                                )
                                .fill(palette.accent)
                                .min_size([400.0, 42.0].into()),
                            )
                            .clicked()
                        {
                            self.application.dispatch(AppCommand::Connect { server_password: Default::default(),
                                address: self.server_address.trim().to_owned(),
                                nickname: self.nickname.trim().to_owned(),
                                remember: true,
                                auto_connect: None,
                            });
                        }
                        ui.add_space(8.0);
                        if ui
                            .button(tr(
                                language,
                                "⚙ 音频与外观设置",
                                "⚙ Audio & appearance settings",
                            ))
                            .clicked()
                        {
                            self.application.dispatch(AppCommand::RefreshDevices);
                            self.audio_preferences = self.application.audio_preferences();
                            self.application.dispatch(AppCommand::PrepareLocalAudio);
                            self.settings_open = true;
                        }
                        match &self.application.state().connection {
                            ConnectionState::Connecting => {
                                ui.spinner();
                                ui.label(tr(language, "正在连接…", "Connecting…"));
                            }
                            ConnectionState::Reconnecting => {
                                ui.spinner();
                                ui.label(tr(language, "正在重新连接…", "Reconnecting…"));
                            }
                            _ => {}
                        }
                        if let Some(error) = self.application.error() {
                            ui.add_space(8.0);
                            ui.label(RichText::new(error).color(palette.danger));
                        }
                    });
            });
        });
    }

    fn channel_tree_layout(&mut self, root: &mut egui::Ui, language: Language) {
        let palette = ui::theme::current(root.ctx());
        let state = self.application.state();
        let own_channel = state.self_session.and_then(|id| state.users.get(&id)).map(|u| u.channel_id).unwrap_or(0);
        let channel_name = state.channels.iter().find(|c| c.id == own_channel).map(|c| c.name.clone()).unwrap_or_else(|| tr(language, "未加入频道", "No channel").into());
        let transport = state.actual_transport.clone();
        let rtt = state.rtt_ms.map(|v| format!("{v}ms")).unwrap_or_else(|| "--ms".into());
        let output_device = state.active_output_device.clone().unwrap_or_else(|| tr(language, "系统默认输出", "System output").into());
        let sample_rate = state.active_output_sample_rate.unwrap_or(48_000) / 1000;
        let muted = state.muted;
        let deafened = state.deafened;

        egui::TopBottomPanel::top("voice_topbar").exact_height(54.0).show_inside(root, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(RichText::new("EdenVoice").size(16.0).strong());
                ui.add_space(22.0);
                topology_badge(ui, &transport);
                ui.add_space(12.0);
                ui::draw_icon(ui.painter(), ui.cursor().center(), ui::Icon::Signal, palette.success);
                ui.label(RichText::new(&rtt).size(12.0).color(palette.text_secondary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::icon_button(ui, ui::Icon::Settings, false, tr(language, "设置", "Settings")).clicked() { self.application.dispatch(AppCommand::RefreshDevices); self.audio_preferences = self.application.audio_preferences(); self.settings_open = true; }
                    ui::icon_button(ui, ui::Icon::Users, false, tr(language, "成员", "Members"));
                    ui::icon_button(ui, ui::Icon::Search, false, tr(language, "搜索", "Search"));
                    ui.add_space(8.0);
                });
            });
        });
        egui::TopBottomPanel::bottom("voice_controls").exact_height(66.0).show_inside(root, |ui| {
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let (rect, response) = ui.allocate_exact_size(egui::vec2(42.0,42.0),egui::Sense::click());
                ui.painter().circle_filled(rect.center(), 20.0, if muted { palette.danger } else { palette.accent });
                ui::draw_icon(ui.painter(), rect.center(), if muted { ui::Icon::MicOff } else { ui::Icon::Mic }, Color32::WHITE);
                if response.on_hover_text(tr(language, "麦克风", "Microphone")).clicked() { self.application.dispatch(AppCommand::SetMuted(!muted)); }
                if ui::icon_button(ui, if deafened { ui::Icon::VolumeOff } else { ui::Icon::Volume }, deafened, tr(language, "扬声器", "Speaker")).clicked() { self.application.dispatch(AppCommand::SetDeafened(!deafened)); }
                ui.separator();
                ui.label(RichText::new(output_device).size(12.0));
                ui::icon_button(ui, ui::Icon::ChevronDown, false, tr(language, "输出设备", "Output device"));
                if ui::icon_button(ui, ui::Icon::Settings, false, tr(language, "设置", "Settings")).clicked() { self.application.dispatch(AppCommand::RefreshDevices); self.audio_preferences = self.application.audio_preferences(); self.settings_open = true; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new(format!("OPUS  {sample_rate}kHz")).size(11.0).color(palette.text_secondary));
                    ui.label(RichText::new(&rtt).size(11.0).color(palette.success));
                    ui::draw_icon(ui.painter(), ui.cursor().center(), ui::Icon::Signal, palette.success);
                });
            });
        });
        egui::SidePanel::left("channel_tree").exact_width(245.0).show_inside(root, |ui| {
            ui.add_space(10.0);
            egui::Frame::new().fill(palette.surface_secondary).corner_radius(7).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                ui.horizontal(|ui| { ui::status_dot(ui, palette.success, &self.application.state().server_name); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui::draw_icon(ui.painter(), ui.cursor().center(), ui::Icon::ChevronDown, palette.text_secondary); }); });
            });
            ui.add_space(12.0);
            for channel in self.application.state().channels.clone() {
                let members: Vec<_> = self.application.state().users.values().filter(|u| u.channel_id == channel.id).cloned().collect();
                let selected = channel.id == own_channel;
                let response = ui.add_sized([ui.available_width(), 30.0], egui::Button::new(RichText::new(&channel.name).strong()).selected(selected).frame(false));
                if response.clicked() { self.application.dispatch(AppCommand::JoinChannel(channel.id)); }
                let badge = format!("{}", members.len());
                ui.painter().text(response.rect.right_center()-egui::vec2(14.0,0.0),egui::Align2::CENTER_CENTER,badge,egui::FontId::proportional(10.0),palette.accent);
                for user in members {
                    ui.horizontal(|ui| {
                        avatar(ui, &user.nickname, 21.0, palette);
                        let speaking = self.application.state().is_talking(user.session_id);
                        let label = ui.selectable_label(self.selected_user == Some(user.session_id), RichText::new(&user.nickname).size(12.0));
                        if label.clicked() { self.selected_user=Some(user.session_id); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if user.muted { ui::draw_icon(ui.painter(),ui.cursor().center(),ui::Icon::MicOff,palette.text_muted); } else if speaking { ui::draw_icon(ui.painter(),ui.cursor().center(),ui::Icon::Signal,palette.success); } });
                    });
                }
                ui.add_space(7.0);
            }
        });
        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| { ui.heading(&channel_name); ui.label(RichText::new(format!(" · {} {}", self.application.state().users.values().filter(|u|u.channel_id==own_channel).count(),tr(language,"人在线","online"))).size(12.0).color(palette.text_secondary)); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| { ui::icon_button(ui,ui::Icon::More,false,"More"); ui::icon_button(ui,ui::Icon::Users,false,"Members"); }); });
            ui.label(RichText::new(tr(language,"语音频道","Voice channel")).size(12.0).color(palette.text_secondary));
            ui.separator();
            ui.add_space(12.0);
            for user in self.application.state().users.values().filter(|u| u.channel_id == own_channel && own_channel != 0) {
                egui::Frame::new().fill(palette.surface).corner_radius(8).inner_margin(12).show(ui, |ui| { ui.horizontal(|ui| { avatar(ui,&user.nickname,30.0,palette); ui.vertical(|ui| { ui.label(RichText::new(&user.nickname).strong()); ui.label(RichText::new(if self.application.state().is_talking(user.session_id) { tr(language,"正在说话","Speaking") } else { tr(language,"已连接","Connected") }).size(11.0).color(if self.application.state().is_talking(user.session_id) { palette.success } else { palette.text_secondary })); }); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| { ui::draw_icon(ui.painter(),ui.cursor().center(),if user.muted { ui::Icon::MicOff } else { ui::Icon::Mic },if user.muted { palette.text_muted } else { palette.text_secondary }); }); }); });
                ui.add_space(6.0);
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| { egui::Frame::new().fill(palette.surface_secondary).corner_radius(8).inner_margin(10).show(ui,|ui| { ui.horizontal(|ui| { ui.label(RichText::new(tr(language,"输入消息…","Message…")).color(palette.text_muted)); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| { ui::icon_button(ui,ui::Icon::More,false,"Emoji"); }); }); }); });
        });
    }

    fn main_screen(&mut self, root: &mut egui::Ui, show_connected_ui: bool) {
        let language = self.application.language();
        if self.settings_open
            && !show_connected_ui
            && matches!(self.application.state().audio_state, AudioState::Stopped)
        {
            self.application.dispatch(AppCommand::PrepareLocalAudio);
        }
        if show_connected_ui { self.channel_tree_layout(root, language); }
        if self.settings_open {
            let context = root.ctx().clone();
            let mut settings_open = self.settings_open;
            egui::Window::new(tr(language, "设置", "Settings"))
                .open(&mut settings_open)
                .default_size([500.0, 680.0])
                .resizable(true)
                .vscroll(true)
                .show(&context, |ui| {
                    self.settings_navigation(ui);
                    ui.separator();
                    ui.add_space(ui::theme::SPACE_3);
                    match self.settings_section {
                        SettingsSection::Appearance => {
                            self.appearance_settings(ui);
                            return;
                        }
                        SettingsSection::Diagnostics => {
                            self.diagnostics_settings(ui);
                            return;
                        }
                        SettingsSection::Audio => {}
                    }
                    ui.heading(tr(language, "音频", "Audio"));
                    ui.label(RichText::new(tr(language, "输入模式", "Input mode")).strong());
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.audio_preferences.input_mode,
                            InputMode::VoiceActivity,
                            tr(language, "语音活动检测", "Voice Activity"),
                        );
                        ui.selectable_value(
                            &mut self.audio_preferences.input_mode,
                            InputMode::PushToTalk,
                            tr(language, "按键说话", "Push to Talk"),
                        );
                    });
                    if self.audio_preferences.input_mode == InputMode::PushToTalk {
                        ui.label(
                            RichText::new(tr(
                                language,
                                "按键说话快捷键",
                                "Push-to-talk key",
                            ))
                            .strong(),
                        );
                        ui.horizontal(|ui| {
                            let button_text = if self.capturing_ptt_key {
                                tr(
                                    language,
                                    "请按下新按键…（Esc 取消）",
                                    "Press a new key… (Esc to cancel)",
                                )
                                .to_owned()
                            } else {
                                format!(
                                    "{}：{}",
                                    tr(language, "当前", "Current"),
                                    ptt_key_label(&self.audio_preferences.ptt_key)
                                )
                            };
                            if ui
                                .add_sized([260.0, 34.0], egui::Button::new(button_text))
                                .clicked()
                            {
                                self.capturing_ptt_key = true;
                            }
                            if ui
                                .button(tr(language, "恢复 ~", "Reset to ~"))
                                .clicked()
                            {
                                self.audio_preferences.ptt_key = "Backquote".into();
                                self.application
                                    .dispatch(AppCommand::SetPttKey("Backquote".into()));
                                self.capturing_ptt_key = false;
                            }
                        });
                        ui.label(
                            RichText::new(format!(
                                "{} {}",
                                tr(
                                    language,
                                    "按住此键即可说话：",
                                    "Hold this key to talk:"
                                ),
                                ptt_key_label(&self.audio_preferences.ptt_key)
                            ))
                            .color(ui::theme::current(ui.ctx()).text_secondary),
                        );
                        if let Some(error) = self.application.global_ptt_error() {
                            ui.label(
                                RichText::new(error).color(ui::theme::current(ui.ctx()).warning),
                            );
                        }
                    }
                    ui.add_space(ui::theme::SPACE_3);
                    egui::ComboBox::from_label(tr(language, "输入设备", "Input device"))
                        .selected_text(
                            self.audio_preferences
                                .input_device
                                .as_deref()
                                .unwrap_or(tr(language, "系统默认", "System default")),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.audio_preferences.input_device,
                                None,
                                tr(language, "系统默认", "System default"),
                            );
                            for name in &self.application.devices().inputs {
                                ui.selectable_value(
                                    &mut self.audio_preferences.input_device,
                                    Some(name.clone()),
                                    name,
                                );
                            }
                        });
                    ui.label(
                        RichText::new(noise_suppression_description(
                            language,
                            self.audio_preferences.noise_suppression_mode,
                        ))
                        .color(ui::theme::current(ui.ctx()).text_secondary),
                    );
                    egui::ComboBox::from_label(tr(language, "输出设备", "Output device"))
                        .selected_text(
                            self.audio_preferences
                                .output_device
                                .as_deref()
                                .unwrap_or(tr(language, "系统默认", "System default")),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.audio_preferences.output_device,
                                None,
                                tr(language, "系统默认", "System default"),
                            );
                            for name in &self.application.devices().outputs {
                                ui.selectable_value(
                                    &mut self.audio_preferences.output_device,
                                    Some(name.clone()),
                                    name,
                                );
                            }
                        });
                    ui.label(
                        RichText::new(tr(
                            language,
                            "EdenVoice 内部统一使用 48 kHz；设备采样率会在输入输出边界自动适配。",
                            "EdenVoice uses 48 kHz internally and adapts device sample rates at the I/O boundary.",
                        ))
                        .color(ui::theme::current(ui.ctx()).text_secondary),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Slider::new(&mut self.audio_preferences.input_gain, 0.0..=2.0)
                                .text(tr(language, "输入增益", "Input gain"))
                                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                        );
                        if ui.button(tr(language, "重置为 100%", "Reset 100%")).clicked() {
                            self.audio_preferences.input_gain = 1.0;
                        }
                    });
                    ui.label(tr(language, "输入电平", "Input level"));
                    ui.add(
                        egui::ProgressBar::new(self.application.state().input_level)
                            .desired_width(280.0),
                    );
                    ui.add_space(ui::theme::SPACE_3);
                    ui.label(
                        RichText::new(tr(
                            language,
                            "麦克风自听测试",
                            "Microphone self-test",
                        ))
                        .strong(),
                    );
                    ui.label(
                        RichText::new(tr(
                            language,
                            "录制 4 秒后，可分别回放原声和处理后的声音，录音只保存在本机内存中。",
                            "Record for 4 seconds, then hear the original and processed versions. Audio stays in local memory.",
                        ))
                        .color(ui::theme::current(ui.ctx()).text_secondary),
                    );
                    ui.horizontal(|ui| match self.application.state().mic_test_status {
                        MicTestStatus::Idle => {
                            let settings_saved =
                                self.audio_preferences == self.application.audio_preferences();
                            let audio_ready =
                                matches!(self.application.state().audio_state, AudioState::Ready);
                            let requested_mode = self.audio_preferences.noise_suppression_mode;
                            let active_mode = self.application.state().active_noise_suppression;
                            let dsp_loading = self.application.state().dsp_loading;
                            let dsp_ready = dsp_loading.is_none() && active_mode == requested_mode;
                            if ui
                                .add_enabled(
                                    settings_saved && audio_ready && dsp_ready,
                                    egui::Button::new(tr(
                                        language,
                                        "开始麦克风测试",
                                        "Start microphone test",
                                    )),
                                )
                                .clicked()
                            {
                                self.application.dispatch(AppCommand::StartMicTest);
                            }
                            if !audio_ready {
                                ui.label(
                                    RichText::new(tr(
                                        language,
                                        "正在启动本地音频，请稍候…",
                                        "Starting local audio, please wait…",
                                    ))
                                        .color(ui::theme::current(ui.ctx()).text_secondary),
                                );
                            } else if !settings_saved {
                                if ui
                                    .button(tr(language, "保存并应用", "Save and apply"))
                                    .clicked()
                                {
                                    self.application.dispatch(
                                        AppCommand::UpdateAudioPreferences(
                                            self.audio_preferences.clone(),
                                        ),
                                    );
                                }
                            } else if !dsp_ready {
                                ui.spinner();
                                ui.label(
                                    RichText::new(if let Some(loading) = dsp_loading {
                                        format!(
                                            "{} {}…",
                                            tr(
                                                language,
                                                "正在准备",
                                                "Preparing"
                                            ),
                                            noise_suppression_label(language, loading),
                                        )
                                    } else {
                                        format!(
                                            "{} {} · {} {}",
                                            tr(language, "请求", "Requested"),
                                            noise_suppression_label(language, requested_mode),
                                            tr(language, "当前生效", "currently active"),
                                            noise_suppression_label(language, active_mode),
                                        )
                                    })
                                    .color(ui::theme::current(ui.ctx()).text_secondary),
                                );
                            } else {
                                ui.label(
                                    RichText::new(format!(
                                        "{}：{}",
                                        tr(language, "当前生效", "Active"),
                                        noise_suppression_label(language, active_mode),
                                    ))
                                    .color(ui::theme::current(ui.ctx()).text_secondary),
                                );
                            }
                        }
                        MicTestStatus::Recording => {
                            ui.spinner();
                            ui.label(tr(
                                language,
                                "正在本地录制 4 秒原声与处理后声音…",
                                "Recording original and processed audio for 4 seconds locally…",
                            ));
                            if ui.button(tr(language, "停止", "Stop")).clicked() {
                                self.application.dispatch(AppCommand::StopMicTest);
                            }
                        }
                        MicTestStatus::PreviewReady => {
                            ui.label(tr(language, "试听：", "Preview:"));
                            if ui
                                .button(tr(language, "播放原声", "Play Original"))
                                .clicked()
                            {
                                self.application.dispatch(AppCommand::PlayMicTestOriginal);
                            }
                            if ui
                                .button(tr(language, "播放处理后声音", "Play Processed"))
                                .clicked()
                            {
                                self.application.dispatch(AppCommand::PlayMicTestProcessed);
                            }
                            if ui.button(tr(language, "清除", "Clear")).clicked() {
                                self.application.dispatch(AppCommand::StopMicTest);
                            }
                        }
                        MicTestStatus::PlayingOriginal | MicTestStatus::PlayingProcessed => {
                            ui.label(
                                if self.application.state().mic_test_status
                                    == MicTestStatus::PlayingOriginal
                                {
                                    tr(
                                        language,
                                        "正在本地播放原声录音…",
                                        "Playing original recording locally…",
                                    )
                                } else {
                                    tr(
                                        language,
                                        "正在本地播放处理后录音…",
                                        "Playing processed recording locally…",
                                    )
                                },
                            );
                            if ui.button(tr(language, "停止", "Stop")).clicked() {
                                self.application.dispatch(AppCommand::StopMicTest);
                            }
                        }
                    });
                    ui.add(
                        egui::Slider::new(&mut self.audio_preferences.output_volume, 0.01..=2.0)
                            .text(tr(language, "输出音量", "Output volume")),
                    );
                    let current_quality = self.application.state().audio_quality_mode.clone();
                    let mut selected_quality = current_quality.clone();
                    egui::ComboBox::from_label(tr(
                        language,
                        "通话音频码率",
                        "Voice bitrate",
                    ))
                    .selected_text(audio_quality_label(language, &selected_quality))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut selected_quality,
                            "auto".into(),
                            tr(language, "自动（推荐）", "Auto (recommended)"),
                        );
                        for bitrate in [24, 32, 48, 64] {
                            ui.selectable_value(
                                &mut selected_quality,
                                format!("fixed_{bitrate}k"),
                                format!("{bitrate} kbps"),
                            );
                        }
                    });
                    if selected_quality != current_quality {
                        self.application.dispatch(AppCommand::SetAudioQualityPolicy(
                            parse_audio_quality_policy(&selected_quality),
                        ));
                    }
                    let audio_ready =
                        matches!(self.application.state().audio_state, AudioState::Ready);
                    if ui
                        .add_enabled(
                            audio_ready,
                            egui::Button::new(tr(language, "播放输出测试音", "Play output test")),
                        )
                        .clicked()
                    {
                        self.application.dispatch(AppCommand::PlayOutputTest);
                    }
                    ui.add_space(ui::theme::SPACE_5);
                    ui.separator();
                    ui.add_space(ui::theme::SPACE_3);
                    ui.heading(tr(language, "语音处理", "Voice processing"));
                    ui.checkbox(
                        &mut self.audio_preferences.echo_cancellation,
                        tr(language, "回声消除", "Echo cancellation"),
                    );
                    ui.checkbox(
                        &mut self.audio_preferences.automatic_gain,
                        tr(language, "自动增益控制", "Automatic gain control"),
                    );
                    egui::ComboBox::from_label(tr(language, "降噪", "Noise suppression"))
                        .selected_text(noise_suppression_label(
                            language,
                            self.audio_preferences.noise_suppression_mode,
                        ))
                        .show_ui(ui, |ui| {
                            for mode in [
                                NoiseSuppressionMode::Off,
                                NoiseSuppressionMode::Standard,
                                NoiseSuppressionMode::RnNoise,
                                NoiseSuppressionMode::DeepFilterNet,
                                NoiseSuppressionMode::DeepFilterNetStrong,
                            ] {
                                ui.selectable_value(
                                    &mut self.audio_preferences.noise_suppression_mode,
                                    mode,
                                    noise_suppression_label(language, mode),
                                );
                            }
                        });
                    if self.audio_preferences.noise_suppression_mode
                        == NoiseSuppressionMode::Standard
                    {
                        egui::ComboBox::from_label(tr(
                            language,
                            "降噪强度",
                            "Suppression strength",
                        ))
                            .selected_text(noise_strength_label(
                                language,
                                self.audio_preferences.noise_suppression_strength,
                            ))
                            .show_ui(ui, |ui| {
                                for strength in [
                                    NoiseSuppressionStrength::Low,
                                    NoiseSuppressionStrength::Medium,
                                    NoiseSuppressionStrength::High,
                                ] {
                                    ui.selectable_value(
                                        &mut self.audio_preferences.noise_suppression_strength,
                                        strength,
                                        noise_strength_label(language, strength),
                                    );
                                }
                            });
                    }
                    ui.add_space(ui::theme::SPACE_4);
                    if ui.button(tr(language, "保存", "Save")).clicked() {
                        self.application
                            .dispatch(AppCommand::UpdateAudioPreferences(
                                self.audio_preferences.clone(),
                            ));
                    }
                });
            self.settings_open = settings_open;
            if !self.settings_open {
                self.capturing_ptt_key = false;
                if !show_connected_ui {
                    self.application.dispatch(AppCommand::StopLocalAudio);
                }
            }
        }
        if show_connected_ui {
            self.user_popover(root.ctx());
        }
    }

    fn settings_navigation(&mut self, ui: &mut egui::Ui) {
        let language = self.application.language();
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.settings_section,
                SettingsSection::Appearance,
                tr(language, "外观", "Appearance"),
            );
            ui.selectable_value(
                &mut self.settings_section,
                SettingsSection::Audio,
                tr(language, "音频", "Audio"),
            );
            ui.selectable_value(
                &mut self.settings_section,
                SettingsSection::Diagnostics,
                tr(language, "诊断", "Diagnostics"),
            );
        });
    }

    fn appearance_settings(&mut self, ui: &mut egui::Ui) {
        let language = self.application.language();
        ui.heading(tr(language, "外观", "Appearance"));
        ui.label(
            RichText::new(tr(
                language,
                "跟随系统，或选择固定的浅色/深色主题。",
                "Follow the system or choose a dedicated light or dark palette.",
            ))
            .color(ui::theme::current(ui.ctx()).text_secondary),
        );
        ui.add_space(ui::theme::SPACE_3);
        let mut theme = self.application.theme();
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut theme,
                ThemeMode::System,
                tr(language, "跟随系统", "System"),
            );
            ui.selectable_value(&mut theme, ThemeMode::Light, tr(language, "浅色", "Light"));
            ui.selectable_value(&mut theme, ThemeMode::Dark, tr(language, "深色", "Dark"));
        });
        if theme != self.application.theme() {
            self.application.dispatch(AppCommand::SetTheme(theme));
            ui::configure(ui.ctx(), theme);
        }
        ui.add_space(ui::theme::SPACE_5);
        ui.label(RichText::new(tr(language, "界面语言", "Language")).strong());
        let mut selected_language = language;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut selected_language, Language::Chinese, "中文");
            ui.selectable_value(&mut selected_language, Language::English, "English");
        });
        if selected_language != language {
            self.application
                .dispatch(AppCommand::SetLanguage(selected_language));
        }
    }

    fn diagnostics_settings(&mut self, ui: &mut egui::Ui) {
        let language = self.application.language();
        ui.heading(tr(language, "诊断", "Diagnostics"));
        ui.label(
            RichText::new(tr(
                language,
                "用于排查音频与网络质量问题的实时技术状态。",
                "Live technical state for troubleshooting audio and network quality.",
            ))
            .color(ui::theme::current(ui.ctx()).text_secondary),
        );
        ui.add_space(ui::theme::SPACE_3);
        let state = self.application.state();
        let received_or_lost = state.packets_received.saturating_add(state.packets_lost);
        let loss_percent = if received_or_lost == 0 {
            0.0
        } else {
            state.packets_lost as f64 * 100.0 / received_or_lost as f64
        };
        ui.label(RichText::new(tr(language, "连接", "Connection")).strong());
        ui.label(format!(
            "State: {:?} · RTT {}",
            state.connection,
            state
                .rtt_ms
                .map_or_else(|| "—".to_owned(), |rtt| format!("{rtt} ms")),
        ));
        ui.label(format!(
            "Topology: {} · Transport: {} · Reason: {}",
            state.room_topology, state.current_transport, state.route_reason,
        ));
        ui.label(
            RichText::new(format!(
                "Actual path: {} · Route: {}",
                state.actual_transport, state.selected_route,
            ))
            .strong(),
        );
        ui.label(format!(
            "Endpoint: {}",
            if state.route_endpoint.is_empty() {
                "—"
            } else {
                &state.route_endpoint
            }
        ));
        ui.label(format!("P2P qualification: {}", state.p2p_qualification));
        ui.label(format!(
            "Qualification {} ms · migration cooldown {} ms",
            state.qualification_duration_ms, state.migration_cooldown_ms,
        ));
        ui.label(format!(
            "Packets: sent {} · received {} · lost {} ({loss_percent:.2}%) · reordered {}",
            state.packets_sent, state.packets_received, state.packets_lost, state.packets_reordered,
        ));
        ui.label(format!(
            "Jitter buffered {} frames · Opus {} kbps",
            state.jitter_depth,
            state.opus_bitrate_bps / 1_000,
        ));
        ui.add_space(ui::theme::SPACE_4);
        ui.label(RichText::new(tr(language, "音频", "Audio")).strong());
        ui.label(format!(
            "Input RMS {:.3} · peak {:.3} · output RMS {:.3}",
            state.input_level, state.input_peak, state.output_level,
        ));
        ui.label(format!(
            "Processing: {} · AEC {} · AGC {}",
            noise_suppression_label(language, state.active_noise_suppression),
            if state.active_echo_cancellation {
                "on"
            } else {
                "off"
            },
            if state.active_automatic_gain {
                "on"
            } else {
                "off"
            },
        ));
        ui.label(format!(
            "DSP average {:.2} ms · peak {:.2} ms · deadline misses {}",
            state.dsp_average_ms, state.dsp_peak_ms, state.dsp_deadline_misses,
        ));
        ui.label(format!(
            "Capture overruns {} · playback underruns {}",
            state.capture_overruns, state.playback_underruns,
        ));
        if let Some(mode) = state.dsp_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Preparing {} in the background; current processing remains active…",
                    noise_suppression_label(language, mode)
                ));
            });
        }
        if let (Some(input), Some(output)) =
            (&state.active_input_device, &state.active_output_device)
        {
            ui.label(format!("Devices: {input} → {output}"));
        }
        if let (Some(input_rate), Some(output_rate)) = (
            state.active_input_sample_rate,
            state.active_output_sample_rate,
        ) {
            ui.label(format!(
                "Device PCM: {} input → {} output (internal 48 kHz)",
                format_sample_rate(input_rate),
                format_sample_rate(output_rate),
            ));
        }
        if state.device_fallback_active {
            ui.label(
                RichText::new("A requested device was unavailable; using a system fallback.")
                    .color(ui::theme::current(ui.ctx()).warning),
            );
        }
        if let Some(error) = &state.device_error {
            ui.label(RichText::new(error).color(ui::theme::current(ui.ctx()).warning));
        }
        if let Some(error) = &state.dsp_error {
            ui.label(RichText::new(error).color(ui::theme::current(ui.ctx()).warning));
        }
        if let Some(warning) = &state.dsp_performance_warning {
            ui.label(RichText::new(warning).color(ui::theme::current(ui.ctx()).warning));
        }
        let current_mode = state.transport_mode.clone();
        let current_quality = state.audio_quality_mode.clone();
        let network_quality = state.network_quality.clone();
        let network_trend = state.network_trend.clone();
        let mut selected_mode = current_mode.clone();
        egui::ComboBox::from_label(tr(language, "网络模式", "Network mode"))
            .selected_text(&selected_mode)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_mode, "auto".into(), "Auto");
                ui.selectable_value(&mut selected_mode, "force_sfu".into(), "Force SFU");
                ui.selectable_value(&mut selected_mode, "force_p2p".into(), "Force P2P");
            });
        if selected_mode != current_mode {
            let policy = match selected_mode.as_str() {
                "force_sfu" => crate::network::RoomNetworkPolicy::ForceSfu,
                "force_p2p" => crate::network::RoomNetworkPolicy::ForceP2p,
                _ => crate::network::RoomNetworkPolicy::Auto,
            };
            self.application
                .dispatch(AppCommand::SetRoomNetworkPolicy(policy));
        }
        ui.label(format!(
            "Voice quality policy: {}",
            audio_quality_label(language, &current_quality)
        ));
        ui.small(format!("Network: {} / {}", network_quality, network_trend));
        if current_quality.starts_with("fixed_")
            && matches!(network_quality.as_str(), "poor" | "critical")
        {
            ui.label(
                RichText::new(tr(
                    language,
                    "当前网络较差，固定音质可能导致语音不稳定",
                    "The current network may be too weak for fixed quality",
                ))
                .color(ui::theme::current(ui.ctx()).warning),
            );
        } else if network_quality == "critical" {
            ui.label(
                RichText::new(tr(
                    language,
                    "网络连接不稳定，正在优化语音",
                    "Network unstable; optimizing voice",
                ))
                .color(ui::theme::current(ui.ctx()).warning),
            );
        }
    }

    fn user_popover(&mut self, context: &egui::Context) {
        let language = self.application.language();
        let Some(session_id) = self.selected_user else {
            return;
        };
        if self.application.state().self_session == Some(session_id) {
            self.selected_user = None;
            return;
        }
        let Some(user) = self.application.state().users.get(&session_id) else {
            self.selected_user = None;
            return;
        };
        let nickname = user.nickname.clone();
        let speaking = self.application.state().is_talking(session_id);
        let mut volume = self.application.state().user_volume(session_id);
        let mut locally_muted = self.application.state().is_user_locally_muted(session_id);
        let previous_volume = volume;
        let previous_muted = locally_muted;
        let mut open = true;
        egui::Window::new(nickname)
            .id(egui::Id::new(("user-audio", session_id)))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(context, |ui| {
                let palette = ui::theme::current(ui.ctx());
                ui.label(
                    RichText::new(if speaking {
                        tr(language, "正在说话", "Speaking")
                    } else {
                        tr(language, "未说话", "Not speaking")
                    })
                    .color(if speaking {
                        palette.success
                    } else {
                        palette.text_secondary
                    }),
                );
                ui.add_space(ui::theme::SPACE_4);
                ui.label(RichText::new(tr(language, "音量", "Volume")).strong());
                ui.add(
                    egui::Slider::new(&mut volume, 0.0..=2.0)
                        .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                );
                ui.checkbox(
                    &mut locally_muted,
                    tr(language, "仅在本地静音", "Mute locally"),
                );
            });
        if volume != previous_volume {
            self.application
                .dispatch(AppCommand::SetUserVolume { session_id, volume });
        }
        if locally_muted != previous_muted {
            self.application.dispatch(AppCommand::SetUserLocalMute {
                session_id,
                muted: locally_muted,
            });
        }
        if !open {
            self.selected_user = None;
        }
    }
}

fn noise_suppression_label(language: Language, mode: NoiseSuppressionMode) -> &'static str {
    match mode {
        NoiseSuppressionMode::Off => tr(language, "关闭", "Off"),
        NoiseSuppressionMode::Standard => {
            tr(language, "标准 (WebRTC APM)", "Standard (WebRTC APM)")
        }
        NoiseSuppressionMode::RnNoise => "RNNoise",
        NoiseSuppressionMode::DeepFilterNet => {
            tr(language, "DeepFilterNet · 自然", "DeepFilterNet · Natural")
        }
        NoiseSuppressionMode::DeepFilterNetStrong => {
            tr(language, "DeepFilterNet · 强力", "DeepFilterNet · Strong")
        }
    }
}

fn noise_strength_label(language: Language, strength: NoiseSuppressionStrength) -> &'static str {
    match strength {
        NoiseSuppressionStrength::Low => tr(language, "低", "Low"),
        NoiseSuppressionStrength::Medium => tr(language, "中", "Medium"),
        NoiseSuppressionStrength::High => tr(language, "高", "High"),
    }
}

fn noise_suppression_description(language: Language, mode: NoiseSuppressionMode) -> &'static str {
    match mode {
        NoiseSuppressionMode::Off => tr(
            language,
            "不进行降噪 · 性能影响：无",
            "No noise processing · Performance impact: None",
        ),
        NoiseSuppressionMode::Standard => tr(
            language,
            "WebRTC 瞬态抑制，适合键盘和碰撞声 · 性能影响：极低",
            "WebRTC transient suppression for typing and impacts · Performance impact: Very low",
        ),
        NoiseSuppressionMode::RnNoise => tr(
            language,
            "轻量 AI 降噪 · 性能影响：低",
            "Lightweight AI noise reduction · Performance impact: Low",
        ),
        NoiseSuppressionMode::DeepFilterNet => tr(
            language,
            "自然人声增强，固定约 70 ms 处理延迟 · 性能影响：较高",
            "Natural speech enhancement with about 70 ms fixed processing delay · Performance impact: Higher",
        ),
        NoiseSuppressionMode::DeepFilterNetStrong => tr(
            language,
            "强力人声隔离，抑制更多非语音声音，固定约 70 ms 处理延迟 · 性能影响：较高",
            "Strong voice isolation with more non-speech suppression and about 70 ms fixed processing delay · Performance impact: Higher",
        ),
    }
}

fn tr(language: Language, chinese: &'static str, english: &'static str) -> &'static str {
    match language {
        Language::Chinese => chinese,
        Language::English => english,
    }
}

fn ptt_key_label(code: &str) -> &str {
    match code {
        "Backquote" => "~",
        "Space" => "Space",
        value => value
            .strip_prefix("Key")
            .or_else(|| value.strip_prefix("Digit"))
            .unwrap_or(value),
    }
}

fn newly_pressed_key(context: &egui::Context) -> Option<egui::Key> {
    context.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                ..
            } => Some(physical_key.unwrap_or(*key)),
            _ => None,
        })
    })
}

fn ptt_code_from_egui_key(key: egui::Key) -> Option<&'static str> {
    Some(match key {
        egui::Key::Backtick => "Backquote",
        egui::Key::Backslash | egui::Key::Pipe => "Backslash",
        egui::Key::OpenBracket | egui::Key::OpenCurlyBracket => "BracketLeft",
        egui::Key::CloseBracket | egui::Key::CloseCurlyBracket => "BracketRight",
        egui::Key::Comma => "Comma",
        egui::Key::Period => "Period",
        egui::Key::Slash | egui::Key::Questionmark => "Slash",
        egui::Key::Semicolon | egui::Key::Colon => "Semicolon",
        egui::Key::Quote => "Quote",
        egui::Key::Minus => "Minus",
        egui::Key::Equals | egui::Key::Plus => "Equal",
        egui::Key::Space => "Space",
        egui::Key::Tab => "Tab",
        egui::Key::Enter => "Enter",
        egui::Key::Backspace => "Backspace",
        egui::Key::Insert => "Insert",
        egui::Key::Delete => "Delete",
        egui::Key::Home => "Home",
        egui::Key::End => "End",
        egui::Key::PageUp => "PageUp",
        egui::Key::PageDown => "PageDown",
        egui::Key::ArrowUp => "ArrowUp",
        egui::Key::ArrowDown => "ArrowDown",
        egui::Key::ArrowLeft => "ArrowLeft",
        egui::Key::ArrowRight => "ArrowRight",
        egui::Key::Num0 => "Digit0",
        egui::Key::Num1 => "Digit1",
        egui::Key::Num2 => "Digit2",
        egui::Key::Num3 => "Digit3",
        egui::Key::Num4 => "Digit4",
        egui::Key::Num5 => "Digit5",
        egui::Key::Num6 => "Digit6",
        egui::Key::Num7 => "Digit7",
        egui::Key::Num8 => "Digit8",
        egui::Key::Num9 => "Digit9",
        egui::Key::A => "KeyA",
        egui::Key::B => "KeyB",
        egui::Key::C => "KeyC",
        egui::Key::D => "KeyD",
        egui::Key::E => "KeyE",
        egui::Key::F => "KeyF",
        egui::Key::G => "KeyG",
        egui::Key::H => "KeyH",
        egui::Key::I => "KeyI",
        egui::Key::J => "KeyJ",
        egui::Key::K => "KeyK",
        egui::Key::L => "KeyL",
        egui::Key::M => "KeyM",
        egui::Key::N => "KeyN",
        egui::Key::O => "KeyO",
        egui::Key::P => "KeyP",
        egui::Key::Q => "KeyQ",
        egui::Key::R => "KeyR",
        egui::Key::S => "KeyS",
        egui::Key::T => "KeyT",
        egui::Key::U => "KeyU",
        egui::Key::V => "KeyV",
        egui::Key::W => "KeyW",
        egui::Key::X => "KeyX",
        egui::Key::Y => "KeyY",
        egui::Key::Z => "KeyZ",
        egui::Key::F1 => "F1",
        egui::Key::F2 => "F2",
        egui::Key::F3 => "F3",
        egui::Key::F4 => "F4",
        egui::Key::F5 => "F5",
        egui::Key::F6 => "F6",
        egui::Key::F7 => "F7",
        egui::Key::F8 => "F8",
        egui::Key::F9 => "F9",
        egui::Key::F10 => "F10",
        egui::Key::F11 => "F11",
        egui::Key::F12 => "F12",
        egui::Key::F13 => "F13",
        egui::Key::F14 => "F14",
        egui::Key::F15 => "F15",
        egui::Key::F16 => "F16",
        egui::Key::F17 => "F17",
        egui::Key::F18 => "F18",
        egui::Key::F19 => "F19",
        egui::Key::F20 => "F20",
        egui::Key::F21 => "F21",
        egui::Key::F22 => "F22",
        egui::Key::F23 => "F23",
        egui::Key::F24 => "F24",
        _ => return None,
    })
}

fn ptt_egui_key(code: &str) -> egui::Key {
    match code {
        "Backquote" => egui::Key::Backtick,
        "Backslash" => egui::Key::Backslash,
        "BracketLeft" => egui::Key::OpenBracket,
        "BracketRight" => egui::Key::CloseBracket,
        "Comma" => egui::Key::Comma,
        "Period" => egui::Key::Period,
        "Slash" => egui::Key::Slash,
        "Semicolon" => egui::Key::Semicolon,
        "Quote" => egui::Key::Quote,
        "Minus" => egui::Key::Minus,
        "Equal" => egui::Key::Equals,
        "Space" => egui::Key::Space,
        "Tab" => egui::Key::Tab,
        "Enter" => egui::Key::Enter,
        "Backspace" => egui::Key::Backspace,
        "Insert" => egui::Key::Insert,
        "Delete" => egui::Key::Delete,
        "Home" => egui::Key::Home,
        "End" => egui::Key::End,
        "PageUp" => egui::Key::PageUp,
        "PageDown" => egui::Key::PageDown,
        "ArrowUp" => egui::Key::ArrowUp,
        "ArrowDown" => egui::Key::ArrowDown,
        "ArrowLeft" => egui::Key::ArrowLeft,
        "ArrowRight" => egui::Key::ArrowRight,
        "Digit0" => egui::Key::Num0,
        "Digit1" => egui::Key::Num1,
        "Digit2" => egui::Key::Num2,
        "Digit3" => egui::Key::Num3,
        "Digit4" => egui::Key::Num4,
        "Digit5" => egui::Key::Num5,
        "Digit6" => egui::Key::Num6,
        "Digit7" => egui::Key::Num7,
        "Digit8" => egui::Key::Num8,
        "Digit9" => egui::Key::Num9,
        "F1" => egui::Key::F1,
        "F2" => egui::Key::F2,
        "F3" => egui::Key::F3,
        "F4" => egui::Key::F4,
        "F5" => egui::Key::F5,
        "F6" => egui::Key::F6,
        "F7" => egui::Key::F7,
        "F8" => egui::Key::F8,
        "F9" => egui::Key::F9,
        "F10" => egui::Key::F10,
        "F11" => egui::Key::F11,
        "F12" => egui::Key::F12,
        "F13" => egui::Key::F13,
        "F14" => egui::Key::F14,
        "F15" => egui::Key::F15,
        "F16" => egui::Key::F16,
        "F17" => egui::Key::F17,
        "F18" => egui::Key::F18,
        "F19" => egui::Key::F19,
        "F20" => egui::Key::F20,
        "F21" => egui::Key::F21,
        "F22" => egui::Key::F22,
        "F23" => egui::Key::F23,
        "F24" => egui::Key::F24,
        "KeyA" => egui::Key::A,
        "KeyB" => egui::Key::B,
        "KeyC" => egui::Key::C,
        "KeyD" => egui::Key::D,
        "KeyE" => egui::Key::E,
        "KeyF" => egui::Key::F,
        "KeyG" => egui::Key::G,
        "KeyH" => egui::Key::H,
        "KeyI" => egui::Key::I,
        "KeyJ" => egui::Key::J,
        "KeyK" => egui::Key::K,
        "KeyL" => egui::Key::L,
        "KeyM" => egui::Key::M,
        "KeyN" => egui::Key::N,
        "KeyO" => egui::Key::O,
        "KeyP" => egui::Key::P,
        "KeyQ" => egui::Key::Q,
        "KeyR" => egui::Key::R,
        "KeyS" => egui::Key::S,
        "KeyT" => egui::Key::T,
        "KeyU" => egui::Key::U,
        "KeyV" => egui::Key::V,
        "KeyW" => egui::Key::W,
        "KeyX" => egui::Key::X,
        "KeyY" => egui::Key::Y,
        "KeyZ" => egui::Key::Z,
        _ => egui::Key::Backtick,
    }
}

fn format_sample_rate(rate: u32) -> String {
    if rate.is_multiple_of(1_000) {
        format!("{} kHz", rate / 1_000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1_000.0)
    }
}

fn audio_quality_label(language: Language, value: &str) -> String {
    value
        .strip_prefix("fixed_")
        .and_then(|value| value.strip_suffix('k'))
        .map_or_else(
            || tr(language, "自动（推荐）", "Auto (recommended)").into(),
            |value| format!("{value} kbps"),
        )
}

fn parse_audio_quality_policy(value: &str) -> crate::network::AudioQualityPolicy {
    value
        .strip_prefix("fixed_")
        .and_then(|value| value.strip_suffix('k'))
        .and_then(|value| value.parse::<i32>().ok())
        .map_or(crate::network::AudioQualityPolicy::Auto, |kbps| {
            crate::network::AudioQualityPolicy::Fixed {
                target_bitrate_bps: kbps * 1_000,
            }
        })
}

impl eframe::App for EdenVoiceApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.application.update();
        if self.capturing_ptt_key
            && let Some(key) = newly_pressed_key(root.ctx())
        {
            if key == egui::Key::Escape {
                self.capturing_ptt_key = false;
            } else if let Some(code) = ptt_code_from_egui_key(key) {
                self.audio_preferences.ptt_key = code.to_owned();
                self.application
                    .dispatch(AppCommand::SetPttKey(code.to_owned()));
                self.capturing_ptt_key = false;
            }
        }
        let push_to_talk_pressed = self.application.input_mode() == InputMode::PushToTalk
            && !root.ctx().egui_wants_keyboard_input()
            && root
                .ctx()
                .input(|input| input.key_down(ptt_egui_key(self.application.ptt_key())));
        if push_to_talk_pressed != self.push_to_talk_pressed {
            self.push_to_talk_pressed = push_to_talk_pressed;
            self.application
                .dispatch(AppCommand::SetPushToTalkPressed(push_to_talk_pressed));
        }
        if matches!(
            self.application.state().connection,
            ConnectionState::Connected
        ) {
            self.main_screen(root, true);
        } else {
            self.connection_screen(root);
            self.main_screen(root, false);
        }
        root.ctx().request_repaint_after(Duration::from_millis(50));
    }
    fn on_exit(&mut self) {
        self.application.dispatch(AppCommand::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_ptt_keys_use_global_hotkey_codes() {
        assert_eq!(
            ptt_code_from_egui_key(egui::Key::Backtick),
            Some("Backquote")
        );
        assert_eq!(ptt_code_from_egui_key(egui::Key::A), Some("KeyA"));
        assert_eq!(ptt_code_from_egui_key(egui::Key::Num7), Some("Digit7"));
        assert_eq!(ptt_code_from_egui_key(egui::Key::F12), Some("F12"));
        assert_eq!(ptt_code_from_egui_key(egui::Key::Escape), None);
    }
}
