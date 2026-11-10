use std::{env, fs::File, io::Write, path::Path, time::Duration};

use edenvoice_client::{
    AppWindow, ChatItem, CompactChannelItem, MemberItem, SavedServerItem, TreeItem,
};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

fn main() {
    let mut native = false;
    let mut positional = Vec::new();
    for argument in env::args().skip(1) {
        if argument == "--native" {
            native = true;
        } else {
            positional.push(argument);
        }
    }
    if native {
        edenvoice_client::install_native_platform().expect("install native preview backend");
    } else {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .expect("install headless preview backend");
    }
    let window = AppWindow::new().expect("create preview window");
    let mode = positional
        .first()
        .cloned()
        .unwrap_or_else(|| "default".into());
    let output = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| format!("/tmp/edenvoice-{mode}.ppm"));

    configure_preview(&window, &mode);
    window.show().expect("show preview window");
    if native {
        if output != "-" {
            let weak_window = window.as_weak();
            let output_path = output.clone();
            slint::Timer::single_shot(Duration::from_millis(350), move || {
                let Some(window) = weak_window.upgrade() else {
                    return;
                };
                let snapshot = window
                    .window()
                    .take_snapshot()
                    .expect("render native preview");
                write_ppm(Path::new(&output_path), &snapshot).expect("write native preview image");
                println!("{output_path}");
                slint::quit_event_loop().expect("stop native preview");
            });
        }
        window.run().expect("run native preview");
        return;
    }
    let snapshot = window.window().take_snapshot().expect("render preview");
    write_ppm(Path::new(&output), &snapshot).expect("write preview image");
    println!("{output}");
}

fn configure_preview(window: &AppWindow, mode: &str) {
    window.set_macos_titlebar_overlay(true);
    window.set_app_version("0.6.6".into());
    window.set_show_main(true);
    window.set_visual_demo(true);
    window.set_server_name("voice.example.com".into());
    window.set_server_address("voice.example.com:9000".into());
    window.set_channel_name("游戏频道".into());
    window.set_topology("SFU".into());
    window.set_latency("24ms".into());
    window.set_network_quality("good".into());
    window.set_codec_label("OPUS  48kHz".into());
    window.set_output_device("扬声器 (Realtek Audio)".into());
    window.set_packet_loss_label("0.2%".into());
    window.set_jitter_label("9ms".into());
    window.set_uplink_label("64 kbps".into());
    window.set_downlink_label("48 kbps".into());
    window.set_connection_duration_label("01:23:45".into());
    window.set_input_device("Studio Microphone".into());
    window.set_input_sample_rate_label("48 kHz".into());
    window.set_output_sample_rate_label("48 kHz".into());
    window.set_dsp_label("RNNoise（自动）".into());
    window.set_aec_label("开启".into());
    window.set_agc_label("开启".into());
    window.set_input_level(0.42);
    window.set_nickname("Lumi".into());
    window.set_self_initials("L".into());
    window.set_self_avatar("L".into());
    if mode == "avatar-crop" {
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(640, 360);
        for (index, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
            let x = (index % 640) as u8;
            let y = (index / 640) as u8;
            *pixel = Rgba8Pixel {
                r: x,
                g: y,
                b: 180,
                a: 255,
            };
        }
        window.set_avatar_crop_image(Image::from_rgba8(pixels));
        window.set_avatar_crop_width(640.0);
        window.set_avatar_crop_height(360.0);
        window.set_avatar_crop_zoom(1.25);
        window.set_show_avatar_crop(true);
    }

    let members = preview_members();
    window.set_members(ModelRc::new(VecModel::from(members.clone())));
    window.set_compact_avatar_members(ModelRc::new(VecModel::from(
        members.iter().take(4).cloned().collect::<Vec<_>>(),
    )));
    window.set_compact_member_overflow(1);
    window.set_tree_items(ModelRc::new(VecModel::from(preview_tree())));
    window.set_compact_channels(ModelRc::new(VecModel::from(vec![
        compact_channel("大厅", "", 3, 0, false),
        compact_channel("游戏频道", "5", 0, 1, true),
        compact_channel("音乐分享", "2", 1, 0, false),
        compact_channel("AFK 区", "1", 2, 0, false),
        compact_channel("临时频道", "", -1, 0, false),
    ])));
    let preview_server = SavedServerItem {
        name: "EdenVoice".into(),
        address: "voice.example.com:9000".into(),
        nickname: "Lumi".into(),
        avatar: "E".into(),
        avatar_image: Default::default(),
        custom_avatar: false,
    };
    window.set_saved_servers(ModelRc::new(VecModel::from(vec![preview_server.clone()])));
    window.set_recent_servers(ModelRc::new(VecModel::from(vec![
        preview_server,
        SavedServerItem {
            name: "EdenVoice".into(),
            address: "127.0.0.1:9000".into(),
            nickname: "Lumi".into(),
            avatar: "E".into(),
            avatar_image: Default::default(),
            custom_avatar: false,
        },
    ])));
    window.set_output_devices(ModelRc::new(VecModel::from(vec![
        "系统默认输出".into(),
        "扬声器 (Realtek Audio)".into(),
    ])));

    match mode {
        "runtime" => {
            window.set_visual_demo(false);
            window.set_server_name("EdenVoice".into());
            window.set_server_address("voice.example.com:9000".into());
            window.set_channel_name("Gaming".into());
            window.set_latency("141ms".into());
            let member = member("ABC", "A", "1", true, false, false);
            window.set_members(ModelRc::new(VecModel::from(vec![member])));
            window.set_tree_items(ModelRc::new(VecModel::from(vec![
                channel("Lobby", "0", -1, false),
                channel("Gaming", "1", 0, true),
                user("ABC", "A", "1", true),
                channel("AFK", "0", 1, false),
            ])));
            window.set_chat_items(ModelRc::new(VecModel::from(vec![
                ChatItem {
                    name: "ABC".into(),
                    avatar: "A".into(),
                    avatar_image: Default::default(),
                    custom_avatar: false,
                    time: "刚刚".into(),
                    text: "真实频道消息已经接通。".into(),
                    own: false,
                },
                ChatItem {
                    name: "Lumi".into(),
                    avatar: "L".into(),
                    avatar_image: Default::default(),
                    custom_avatar: false,
                    time: "刚刚".into(),
                    text: "收到，文字聊天正常。".into(),
                    own: true,
                },
            ])));
        }
        "connection" => {
            window.set_show_main(false);
            window.set_nickname("Lumi".into());
        }
        "connection-runtime" => {
            window.set_show_main(false);
            window.set_visual_demo(false);
            window.set_nickname("Lumi".into());
        }
        "server-edit" => {
            window.set_show_main(false);
            window.set_editing_server_index(0);
            window.set_editing_server_name("EdenVoice".into());
            window.set_editing_server_address("voice.example.com:9000".into());
            window.set_editing_server_nickname("Lumi".into());
        }
        "server-edit-runtime" => {
            window.set_show_main(false);
            window.set_visual_demo(false);
            window.set_editing_server_index(0);
            window.set_editing_server_name("EdenVoice".into());
            window.set_editing_server_address("203.0.113.11:9001".into());
            window.set_editing_server_nickname("ABC".into());
        }
        "compact" => {
            window.set_window_mode(1);
            window.set_topology("P2P".into());
        }
        "channel-menu" => window.set_context_menu_kind(1),
        "user-menu" => {
            set_context_user(window);
            window.set_context_menu_kind(2);
        }
        "user-card" => {
            set_context_user(window);
            window.set_show_user_card(true);
        }
        "member-list" => window.set_show_member_list(true),
        "info-panel" => {
            window.set_show_main(false);
            window.set_show_info_panel(true);
            window.set_info_panel_only(true);
            window.set_topology("P2P".into());
        }
        "notifications" => window.set_show_notifications(true),
        "network" => window.set_show_network_details(true),
        "settings-network" => {
            window.set_visual_demo(false);
            window.set_room_network_channel_index(0);
            window.set_room_network_channel_name("大厅".into());
            window.set_network_policy_channel_access(ModelRc::new(VecModel::from(vec![true])));
            window.set_show_settings(true);
            window.set_settings_page(1);
        }
        "settings-audio" => {
            window.set_visual_demo(false);
            window.set_show_settings(true);
            window.set_settings_page(0);
        }
        "settings-shortcuts" => {
            window.set_visual_demo(false);
            window.set_show_settings(true);
            window.set_settings_page(2);
        }
        "admin-muted" => {
            window.set_visual_demo(false);
            window.set_mic_muted(true);
            window.set_mic_admin_muted(true);
        }
        _ => {}
    }
}

fn set_context_user(window: &AppWindow) {
    window.set_context_user_session("2".into());
    window.set_context_user_name("Moon".into());
    window.set_context_user_avatar("M".into());
    window.set_context_user_channel("游戏频道".into());
    window.set_context_user_speaking(true);
}

fn preview_members() -> Vec<MemberItem> {
    vec![
        member("Lumi", "L", "1", false, false, false),
        member("Kira", "K", "2", true, false, false),
        member("Moon", "M", "3", true, false, false),
        member("Ray", "R", "4", false, true, false),
        member("Echo", "E", "5", false, false, false),
    ]
}

fn member(
    name: &str,
    avatar: &str,
    session: &str,
    speaking: bool,
    muted: bool,
    locally_muted: bool,
) -> MemberItem {
    MemberItem {
        name: name.into(),
        avatar: avatar.into(),
        avatar_image: Default::default(),
        custom_avatar: false,
        session_id: session.into(),
        speaking,
        muted,
        admin_muted: false,
        server_muted: false,
        channel_muted: false,
        deafened: false,
        locally_muted,
        volume: 1.0,
        server_role: "".into(),
        channel_role: "".into(),
    }
}

fn preview_tree() -> Vec<TreeItem> {
    vec![
        channel("大厅", "3", -1, false),
        user("Sion", "S", "11", true),
        user("Mio", "M", "12", false),
        user("Alice", "A", "13", false),
        child_channel("游戏频道", "5", 0, true),
        child_user("Lumi", "L", "1", false),
        child_user("Kira", "K", "2", true),
        child_user("Moon", "M", "3", true),
        child_user("Ray", "R", "4", false),
        child_user("Echo", "E", "5", false),
        channel("音乐分享", "2", 1, false),
        channel("AFK 区", "1", 2, false),
    ]
}

fn channel(label: &str, count: &str, index: i32, selected: bool) -> TreeItem {
    TreeItem {
        label: label.into(),
        detail: "".into(),
        count: count.into(),
        avatar: "".into(),
        avatar_image: Default::default(),
        custom_avatar: false,
        channel_index: index,
        session_id: "".into(),
        user_limit: "0".into(),
        depth: 0,
        is_channel: true,
        selected,
        speaking: false,
        muted: false,
        admin_muted: false,
        server_muted: false,
        channel_muted: false,
        deafened: false,
        locally_muted: false,
        volume: 1.0,
        server_role: "".into(),
        channel_role: "".into(),
    }
}

fn child_channel(label: &str, count: &str, index: i32, selected: bool) -> TreeItem {
    let mut item = channel(label, count, index, selected);
    item.depth = 1;
    item
}

fn user(label: &str, avatar: &str, session: &str, speaking: bool) -> TreeItem {
    TreeItem {
        label: label.into(),
        detail: "游戏频道".into(),
        count: "".into(),
        avatar: avatar.into(),
        avatar_image: Default::default(),
        custom_avatar: false,
        channel_index: -1,
        session_id: session.into(),
        user_limit: "".into(),
        depth: 0,
        is_channel: false,
        selected: false,
        speaking,
        muted: false,
        admin_muted: false,
        server_muted: false,
        channel_muted: false,
        deafened: false,
        locally_muted: false,
        volume: 1.0,
        server_role: "".into(),
        channel_role: "".into(),
    }
}

fn child_user(label: &str, avatar: &str, session: &str, speaking: bool) -> TreeItem {
    let mut item = user(label, avatar, session, speaking);
    item.depth = 1;
    item
}

fn compact_channel(
    label: &str,
    count: &str,
    channel_index: i32,
    depth: i32,
    selected: bool,
) -> CompactChannelItem {
    CompactChannelItem {
        label: label.into(),
        count: count.into(),
        channel_index,
        depth,
        selected,
    }
}

fn write_ppm(
    path: &Path,
    buffer: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    write!(file, "P6\n{} {}\n255\n", buffer.width(), buffer.height())?;
    for pixel in buffer.as_bytes().chunks_exact(4) {
        file.write_all(&pixel[..3])?;
    }
    Ok(())
}
