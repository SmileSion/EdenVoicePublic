use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use edenvoice_client::{AppWindow, BanItem, CompactChannelItem, SavedServerItem, TreeItem};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle, ModelRc, VecModel,
    platform::{Key, PointerEventButton, WindowEvent},
};

#[test]
fn channel_row_mouse_click_reaches_rust_callback() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    slint::spawn_local(async move {
        let window = AppWindow::new().unwrap();
        window.set_server_address("voice.test:9000".into());
        window.set_nickname("Alice".into());
        let login_editors: Vec<_> =
            ElementHandle::find_by_element_id(&window, "FormLineEdit::editor").collect();
        assert_eq!(login_editors.len(), 2);
        login_editors[0]
            .single_click(PointerEventButton::Left)
            .await;
        send_shortcut(&window, Key::Meta, false, "a");
        send_shortcut(&window, Key::Meta, false, "c");
        window.set_server_address("".into());
        send_shortcut(&window, Key::Meta, false, "v");
        assert_eq!(window.get_server_address(), "voice.test:9000");
        let connect_remember = Rc::new(Cell::new(true));
        let connect_remember_for_callback = connect_remember.clone();
        window.on_connect(move |_, _, remember| connect_remember_for_callback.set(remember));
        let remember_checks: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::remember-server-check")
                .collect();
        assert_eq!(remember_checks.len(), 1);
        remember_checks[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(!window.get_remember_server());
        let connect_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::connect-touch").collect();
        assert_eq!(connect_buttons.len(), 1);
        connect_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(!connect_remember.get());

        let trust_cancelled = Rc::new(Cell::new(false));
        let trust_cancelled_for_callback = trust_cancelled.clone();
        window.on_disconnect(move || trust_cancelled_for_callback.set(true));
        window.set_trust_endpoint("voice.test:9000".into());
        window.set_trust_fingerprint("test-fingerprint".into());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::trust-dialog").count(),
            1
        );
        let trust_cancel_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::trust-cancel-touch").collect();
        assert_eq!(trust_cancel_buttons.len(), 1);
        trust_cancel_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(trust_cancelled.get());

        let approved_trust = Rc::new(RefCell::new(None));
        let approved_trust_for_callback = approved_trust.clone();
        window.on_trust_server(move |endpoint, fingerprint| {
            *approved_trust_for_callback.borrow_mut() =
                Some((endpoint.to_string(), fingerprint.to_string()));
        });
        let trust_confirm_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::trust-confirm-touch").collect();
        assert_eq!(trust_confirm_buttons.len(), 1);
        trust_confirm_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            approved_trust.borrow().as_ref(),
            Some(&("voice.test:9000".into(), "test-fingerprint".into()))
        );
        window.set_trust_endpoint("".into());
        window.set_trust_fingerprint("".into());

        window.set_show_main(true);
        window.set_channel_name("Lobby".into());
        let sent_chat = Rc::new(RefCell::new(String::new()));
        let sent_chat_for_callback = sent_chat.clone();
        window.on_send_chat(move |text| *sent_chat_for_callback.borrow_mut() = text.to_string());
        window.set_chat_draft("hello from ui".into());
        window.invoke_send_chat(window.get_chat_draft());
        window.set_chat_draft("".into());
        assert_eq!(sent_chat.borrow().as_str(), "hello from ui");
        assert!(window.get_chat_draft().is_empty());

        let avatar_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::avatar-button").collect();
        assert_eq!(avatar_buttons.len(), 1);
        avatar_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(window.get_show_avatar_picker());
        window.set_show_avatar_picker(false);
        window.set_connection_error("没有权限执行此操作。".into());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::operation-error-toast").count(),
            1
        );
        window.set_connection_error("".into());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::operation-error-toast").count(),
            0
        );
        window.set_can_create_channels(true);
        send_shortcut(&window, Key::Control, false, "b");
        assert_eq!(window.get_window_mode(), 0, "shortcuts default to unbound");
        window.on_shortcut_key_pressed(|text, control, _alt, shift, meta| {
            let text = text.to_ascii_lowercase();
            if (control || meta) && !shift && text == "b" {
                1
            } else if (control || meta) && !shift && text == "m" {
                2
            } else if (control || meta) && shift && text == "s" {
                3
            } else if (control || meta) && !shift && text == "u" {
                5
            } else {
                0
            }
        });
        let top_level_channel_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::top-level-channel-button")
                .collect();
        assert_eq!(top_level_channel_buttons.len(), 1);
        top_level_channel_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(window.get_channel_editor_mode(), 3);
        assert_eq!(window.get_channel_editor_index(), -1);
        window.set_channel_editor_mode(0);
        send_shortcut(&window, Key::Control, false, "b");
        assert_eq!(window.get_window_mode(), 1);
        send_shortcut(&window, Key::Meta, false, "b");
        assert_eq!(window.get_window_mode(), 0);
        send_shortcut(&window, Key::Control, false, "u");
        assert!(window.get_show_member_list());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::member-list-card").count(),
            1
        );
        send_shortcut(&window, Key::Meta, false, "u");
        assert!(!window.get_show_member_list());
        window.set_tree_items(ModelRc::new(VecModel::from(vec![
            TreeItem {
                label: "Lobby".into(),
                detail: "".into(),
                count: "0".into(),
                avatar: "".into(),
                avatar_image: Default::default(),
                custom_avatar: false,
                channel_index: 7,
                session_id: "".into(),
                user_limit: "0".into(),
                depth: 0,
                is_channel: true,
                selected: false,
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
            },
            TreeItem {
                label: "Private".into(),
                detail: "".into(),
                count: "0".into(),
                avatar: "".into(),
                avatar_image: Default::default(),
                custom_avatar: false,
                channel_index: 6,
                session_id: "".into(),
                user_limit: "0".into(),
                depth: 1,
                is_channel: true,
                selected: false,
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
            },
        ])));

        let received_index = Rc::new(Cell::new(-1));
        let received_index_for_callback = received_index.clone();
        window.on_join_channel(move |index| received_index_for_callback.set(index));

        let left_panels: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::left-panel").collect();
        let main_bodies: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::main-body").collect();
        let control_bars: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::control-bar").collect();
        let message_inputs: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::message-input").collect();
        let right_panels: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::right-panel").collect();
        let content_layouts: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::content-layout").collect();
        let activity_areas: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::activity-area").collect();
        assert_eq!(left_panels.len(), 1);
        assert_eq!(main_bodies.len(), 1);
        assert_eq!(control_bars.len(), 1);
        assert_eq!(message_inputs.len(), 1);
        assert_eq!(right_panels.len(), 1);
        assert_eq!(content_layouts.len(), 1);
        assert_eq!(activity_areas.len(), 1);
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::temporary-channel-row").count(),
            1
        );
        assert_eq!(left_panels[0].size().width, 140.0);
        assert_eq!(main_bodies[0].size().height, 352.0);
        assert_eq!(control_bars[0].absolute_position().y, 380.0);
        assert_eq!(right_panels[0].size().height, 352.0);
        assert_eq!(content_layouts[0].size().height, 352.0);
        assert_eq!(activity_areas[0].size().height, 271.0);
        assert_eq!(message_inputs[0].absolute_position().y, 348.0);

        window.set_macos_titlebar_overlay(true);
        assert_eq!(main_bodies[0].size().height, 370.0);
        assert_eq!(content_layouts[0].size().height, 370.0);
        assert_eq!(activity_areas[0].size().height, 289.0);
        assert_eq!(message_inputs[0].absolute_position().y, 378.0);
        assert_eq!(control_bars[0].absolute_position().y, 410.0);

        window.set_window_mode(1);
        window.set_compact_channels(ModelRc::new(VecModel::from(vec![CompactChannelItem {
            label: "Lobby".into(),
            count: "1".into(),
            channel_index: 3,
            depth: 1,
            selected: true,
        }])));
        let compact_bodies: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::compact-body").collect();
        let compact_left_panels: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::compact-left-panel").collect();
        let compact_control_bars: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::compact-control-bar").collect();
        assert_eq!(compact_bodies.len(), 1);
        assert_eq!(compact_left_panels.len(), 1);
        assert_eq!(compact_control_bars.len(), 1);
        assert_eq!(compact_left_panels[0].size().width, 138.0);
        assert_eq!(compact_bodies[0].size().height, 370.0);
        assert_eq!(compact_control_bars[0].absolute_position().y, 410.0);
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::compact-server-picker").count(),
            0,
            "compact mode must not repeat the default-mode server picker"
        );
        let compact_channel_rows: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::compact-channel-touch")
                .collect();
        assert_eq!(compact_channel_rows.len(), 1);
        compact_channel_rows[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(received_index.get(), -1);
        compact_channel_rows[0]
            .double_click(PointerEventButton::Left)
            .await;
        assert_eq!(received_index.get(), 3);
        window.set_window_mode(0);

        window.set_show_settings(true);
        window.set_settings_page(1);
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::settings-tabs").count(),
            0,
            "room network policy must be an independent context dialog"
        );
        window.set_settings_page(0);
        let audio_settings_tabs: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::audio-settings-tab").collect();
        let room_settings_tabs: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::room-settings-tab").collect();
        let shortcuts_settings_tabs: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::shortcuts-settings-tab")
                .collect();
        assert_eq!(audio_settings_tabs.len(), 1);
        assert_eq!(room_settings_tabs.len(), 0);
        assert_eq!(shortcuts_settings_tabs.len(), 1);
        audio_settings_tabs[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(window.get_settings_page(), 0);
        assert_eq!(window.get_settings_page(), 0);
        window.set_input_devices(ModelRc::new(VecModel::from(vec![
            "System default".into(),
            "USB microphone".into(),
        ])));
        window.set_input_device_index(0);
        let audio_combo_boxes: Vec<_> =
            ElementHandle::find_by_element_id(&window, "FormComboBox::combo-touch").collect();
        assert!(
            audio_combo_boxes.len() >= 5,
            "audio settings must render the custom platform-independent combo boxes"
        );
        audio_combo_boxes[0]
            .single_click(PointerEventButton::Left)
            .await;
        let input_device_options: Vec<_> =
            ElementHandle::find_by_element_id(&window, "FormComboBox::option-touch").collect();
        assert_eq!(input_device_options.len(), 2);
        input_device_options[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "FormComboBox::option-touch").count(),
            0,
            "choosing an audio device must close the custom combo-box popup"
        );
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::play-output-test-button")
                .count(),
            1,
            "audio test button must use the visible custom button"
        );
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::save-settings-button").count(),
            1,
            "settings save action must use the visible custom button"
        );
        let echo_checks: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::echo-cancellation-check")
                .collect();
        assert_eq!(echo_checks.len(), 1);
        window.set_echo_cancellation(false);
        window.set_automatic_gain(false);
        echo_checks[0].single_click(PointerEventButton::Left).await;
        assert!(window.get_echo_cancellation());
        shortcuts_settings_tabs[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(window.get_settings_page(), 2);
        let shortcut_rows: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::row-layout").collect();
        let shortcut_icons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::icon-tile").collect();
        let shortcut_titles: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::title-text").collect();
        let shortcut_descriptions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::description-text").collect();
        let shortcut_bindings: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::binding-control").collect();
        let shortcut_clears: Vec<_> =
            ElementHandle::find_by_element_id(&window, "ShortcutRow::clear-text").collect();
        assert_eq!(shortcut_rows.len(), 5);
        assert_eq!(shortcut_icons.len(), 5);
        assert_eq!(shortcut_titles.len(), 5);
        assert_eq!(shortcut_descriptions.len(), 5);
        assert_eq!(shortcut_bindings.len(), 5);
        assert_eq!(shortcut_clears.len(), 5);
        for index in 0..shortcut_rows.len() {
            let row_center = shortcut_rows[index].absolute_position().y
                + shortcut_rows[index].size().height / 2.0;
            for element in [&shortcut_icons[index], &shortcut_bindings[index], &shortcut_clears[index]] {
                let center = element.absolute_position().y + element.size().height / 2.0;
                assert!(
                    (center - row_center).abs() <= 0.5,
                    "shortcut row {index} child is not vertically centered: row={row_center}, child={center}"
                );
            }
            let label_top = shortcut_titles[index].absolute_position().y;
            let label_bottom = shortcut_descriptions[index].absolute_position().y
                + shortcut_descriptions[index].size().height;
            assert!(
                ((label_top + label_bottom) / 2.0 - row_center).abs() <= 0.5,
                "shortcut row {index} text block is not vertically centered"
            );
        }
        let settings_cards: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::settings-card").collect();
        assert_eq!(settings_cards.len(), 1);
        let settings_bottom = settings_cards[0].absolute_position().y + settings_cards[0].size().height;
        let last_row_bottom = shortcut_rows[4].absolute_position().y + shortcut_rows[4].size().height;
        assert!(
            last_row_bottom <= settings_bottom,
            "last shortcut row overflows settings card"
        );
        window.set_show_settings(false);

        let rows: Vec<_> =
            ElementHandle::find_by_element_id(&window, "TreeRow::row-touch").collect();
        assert_eq!(
            rows.len(),
            2,
            "expected Lobby and Private channel TouchAreas"
        );
        let tree_labels: Vec<_> =
            ElementHandle::find_by_element_id(&window, "TreeRow::tree-row-label").collect();
        let tree_branches: Vec<_> =
            ElementHandle::find_by_element_id(&window, "TreeRow::tree-row-branch").collect();
        assert_eq!(tree_labels.len(), 2);
        assert_eq!(tree_branches.len(), 1);
        assert!(
            tree_labels[1].absolute_position().x > tree_labels[0].absolute_position().x + 10.0,
            "child channel label must be visibly indented"
        );
        received_index.set(-1);
        rows[0].single_click(PointerEventButton::Left).await;
        assert_eq!(received_index.get(), -1);
        assert_eq!(window.get_selected_channel_index(), 7);
        rows[0].double_click(PointerEventButton::Left).await;
        assert_eq!(received_index.get(), 7);

        let muted_channel = Rc::new(Cell::new(-1));
        let muted_channel_for_callback = muted_channel.clone();
        window.on_mute_channel(move |index| muted_channel_for_callback.set(index));
        rows[0].single_click(PointerEventButton::Right).await;
        assert_eq!(window.get_context_menu_kind(), 1);
        let channel_context_mute: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::channel-context-mute").collect();
        assert_eq!(channel_context_mute.len(), 1);
        channel_context_mute[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(muted_channel.get(), 7);
        assert_eq!(window.get_context_menu_kind(), 0);

        let mut network_policy_access = vec![false; 8];
        network_policy_access[7] = true;
        window.set_network_policy_channel_access(ModelRc::new(VecModel::from(
            network_policy_access.clone(),
        )));
        window.set_channel_administration_access(ModelRc::new(VecModel::from(
            network_policy_access,
        )));
        let room_settings_opened = Rc::new(Cell::new(false));
        let room_settings_opened_for_callback = room_settings_opened.clone();
        window.on_open_settings(move || room_settings_opened_for_callback.set(true));
        rows[0].single_click(PointerEventButton::Right).await;
        let network_policy_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::channel-context-network-policy")
                .collect();
        assert_eq!(network_policy_actions.len(), 1);
        network_policy_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(room_settings_opened.get());
        assert_eq!(window.get_settings_page(), 1);
        assert_eq!(window.get_room_network_channel_index(), 7);
        assert_eq!(window.get_room_network_channel_name(), "Lobby");

        room_settings_opened.set(false);
        window.set_room_network_channel_index(-1);
        rows[1].single_click(PointerEventButton::Right).await;
        let unauthorized_network_policy_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::channel-context-network-policy")
                .collect();
        assert_eq!(unauthorized_network_policy_actions.len(), 1);
        unauthorized_network_policy_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(
            !room_settings_opened.get(),
            "an unassigned room must keep network-policy editing disabled"
        );
        assert_eq!(window.get_room_network_channel_index(), -1);

        window.set_can_edit_channels(true);
        window.set_channel_editor_mode(0);
        rows[1].single_click(PointerEventButton::Right).await;
        let out_of_scope_edit_actions: Vec<_> = ElementHandle::find_by_element_id(
            &window,
            "AppWindow::channel-context-edit",
        )
        .collect();
        assert_eq!(out_of_scope_edit_actions.len(), 1);
        out_of_scope_edit_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            window.get_channel_editor_mode(),
            0,
            "an out-of-scope channel must keep editing disabled"
        );

        rows[0].single_click(PointerEventButton::Right).await;
        let in_scope_edit_actions: Vec<_> = ElementHandle::find_by_element_id(
            &window,
            "AppWindow::channel-context-edit",
        )
        .collect();
        assert_eq!(in_scope_edit_actions.len(), 1);
        in_scope_edit_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(window.get_channel_editor_mode(), 2);
        assert_eq!(window.get_channel_editor_index(), 7);
        window.set_channel_editor_mode(0);

        window.set_context_user_session("42".into());
        window.set_self_session_id("42".into());
        window.set_own_channel_index(7);
        window.set_context_user_channel_index(7);
        window.set_can_admin_mute_users(true);
        window.set_can_kick_channel_users(true);
        window.set_can_kick_server_users(true);
        window.set_context_user_name("Alice".into());
        window.set_context_user_avatar("A".into());
        window.set_context_user_channel("Lobby".into());
        window.set_context_menu_kind(2);
        let local_mute_session = Rc::new(Cell::new(-1_i64));
        let local_mute_session_for_callback = local_mute_session.clone();
        window.on_set_user_local_mute(move |session, muted| {
            if muted {
                local_mute_session_for_callback.set(session.parse::<i64>().unwrap_or(-1));
            }
        });
        let user_context_mute: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::user-context-mute").collect();
        assert_eq!(user_context_mute.len(), 1);
        user_context_mute[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(local_mute_session.get(), 42);

        window.set_context_menu_kind(2);
        let kicked_channel_session = Rc::new(Cell::new(-1_i64));
        let kicked_channel_session_for_callback = kicked_channel_session.clone();
        window.on_kick_user_channel(move |session| {
            kicked_channel_session_for_callback.set(session.parse::<i64>().unwrap_or(-1));
        });
        let kick_channel_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::user-context-kick-channel")
                .collect();
        assert_eq!(kick_channel_actions.len(), 1);
        kick_channel_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            kicked_channel_session.get(),
            -1,
            "self channel-kick action must be disabled"
        );

        window.set_context_menu_kind(2);
        let kicked_server_session = Rc::new(Cell::new(-1_i64));
        let kicked_server_session_for_callback = kicked_server_session.clone();
        window.on_kick_user_server(move |session| {
            kicked_server_session_for_callback.set(session.parse::<i64>().unwrap_or(-1));
        });
        let kick_server_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::user-context-kick-server")
                .collect();
        assert_eq!(kick_server_actions.len(), 1);
        kick_server_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            kicked_server_session.get(),
            -1,
            "self server-kick action must be disabled"
        );

        window.set_context_user_session("84".into());
        window.set_context_user_channel_index(6);
        window.set_context_user_muted(true);
        window.set_context_user_admin_muted(false);
        window.set_context_menu_kind(2);
        let administrative_mute = Rc::new(Cell::new(None::<(i64, bool)>));
        let administrative_mute_for_callback = administrative_mute.clone();
        window.on_admin_mute_user(move |session, muted| {
            administrative_mute_for_callback
                .set(Some((session.parse::<i64>().unwrap_or(-1), muted)));
        });
        let administrative_mute_actions: Vec<_> = ElementHandle::find_by_element_id(
            &window,
            "AppWindow::user-context-admin-mute",
        )
        .collect();
        assert_eq!(administrative_mute_actions.len(), 1);
        administrative_mute_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            administrative_mute.get(),
            None,
            "a target outside the administrator's channel scope must stay disabled"
        );

        window.set_context_user_channel_index(7);
        window.set_context_menu_kind(2);
        let scoped_administrative_mute_actions: Vec<_> = ElementHandle::find_by_element_id(
            &window,
            "AppWindow::user-context-admin-mute",
        )
        .collect();
        assert_eq!(scoped_administrative_mute_actions.len(), 1);
        scoped_administrative_mute_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            administrative_mute.get(),
            Some((84, true)),
            "a self-muted user must still receive an administrative mute request"
        );

        window.set_context_user_admin_muted(true);
        window.set_context_menu_kind(2);
        let administrative_unmute_actions: Vec<_> = ElementHandle::find_by_element_id(
            &window,
            "AppWindow::user-context-admin-mute",
        )
        .collect();
        assert_eq!(administrative_unmute_actions.len(), 1);
        administrative_unmute_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(
            administrative_mute.get(),
            Some((84, false)),
            "an administratively muted user must receive an unmute request"
        );

        window.set_context_menu_kind(2);
        let user_context_info: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::user-context-info").collect();
        assert_eq!(user_context_info.len(), 1);
        user_context_info[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(window.get_show_user_card());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::user-info-card").count(),
            1
        );
        window.set_show_user_card(false);

        let mic_toggled = Rc::new(Cell::new(false));
        let mic_toggled_for_callback = mic_toggled.clone();
        window.on_toggle_mic(move || mic_toggled_for_callback.set(true));
        let mic_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::mic-touch").collect();
        assert_eq!(mic_buttons.len(), 1, "expected one microphone button");
        mic_buttons[0].single_click(PointerEventButton::Left).await;
        assert!(mic_toggled.get(), "microphone click did not reach Rust");
        window.set_mic_admin_muted(true);
        mic_toggled.set(false);
        mic_buttons[0].single_click(PointerEventButton::Left).await;
        assert!(
            !mic_toggled.get(),
            "administrator mute must disable the microphone control"
        );
        window.set_mic_admin_muted(false);
        mic_toggled.set(false);
        send_shortcut(&window, Key::Control, false, "m");
        assert!(mic_toggled.get(), "microphone shortcut did not reach Rust");

        let speaker_toggled = Rc::new(Cell::new(false));
        let speaker_toggled_for_callback = speaker_toggled.clone();
        window.on_toggle_speaker(move || speaker_toggled_for_callback.set(true));
        let speaker_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::speaker-touch").collect();
        assert_eq!(speaker_buttons.len(), 1, "expected one speaker button");
        speaker_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(speaker_toggled.get(), "speaker click did not reach Rust");
        speaker_toggled.set(false);
        send_shortcut(&window, Key::Meta, true, "s");
        assert!(speaker_toggled.get(), "speaker shortcut did not reach Rust");

        let more_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::main-more-button").collect();
        assert_eq!(more_buttons.len(), 1);
        more_buttons[0].single_click(PointerEventButton::Left).await;
        assert!(window.get_show_main_menu());
        assert_eq!(
            ElementHandle::find_by_element_id(&window, "AppWindow::mini-mode-menu-action").count(),
            0,
            "the removed mini-window mode must not leave a hidden menu entry"
        );

        let network_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::network-details-menu-action")
                .collect();
        assert_eq!(network_actions.len(), 1);
        network_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(!window.get_show_main_menu());
        assert!(window.get_show_network_details());
        let network_cards: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::network-details-card").collect();
        assert_eq!(network_cards.len(), 1);
        assert_eq!(network_cards[0].size().width, 220.0);
        assert_eq!(network_cards[0].absolute_position().x, 222.0);

        window.set_show_network_details(false);
        window.set_can_unban_users(true);
        let requested_ban_page = Rc::new(Cell::new(-1));
        let requested_ban_page_for_callback = requested_ban_page.clone();
        window.on_request_bans(move |page| requested_ban_page_for_callback.set(page));
        more_buttons[0].single_click(PointerEventButton::Left).await;
        let ban_manager_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::ban-manager-menu-action")
                .collect();
        assert_eq!(ban_manager_actions.len(), 1);
        ban_manager_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(window.get_show_ban_manager());
        assert_eq!(requested_ban_page.get(), 0);
        window.set_bans(ModelRc::new(VecModel::from(vec![BanItem {
            client_id: "client-42".into(),
            display_name: "Blocked user".into(),
            reason: "abuse".into(),
            created_at: "2026-08-25T00:00:00Z".into(),
        }])));
        window.set_ban_total(1);
        let unbanned_client = Rc::new(Cell::new(false));
        let unbanned_client_for_callback = unbanned_client.clone();
        window.on_unban_client(move |client_id, page| {
            unbanned_client_for_callback.set(client_id == "client-42" && page == 0);
        });
        let unban_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::ban-unban-touch").collect();
        assert_eq!(unban_buttons.len(), 1);
        unban_buttons[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(unbanned_client.get());
        window.set_show_ban_manager(false);

        let saved_server = SavedServerItem {
            name: "Vocal Hub".into(),
            address: "voice.test:9000".into(),
            nickname: "Alice".into(),
            avatar: "V".into(),
            avatar_image: Default::default(),
            custom_avatar: false,
        };
        window.set_saved_servers(ModelRc::new(VecModel::from(vec![saved_server.clone()])));
        window.set_recent_servers(ModelRc::new(VecModel::from(vec![saved_server])));
        window.set_server_address("current.test:9000".into());
        let switched_index = Rc::new(Cell::new(-1));
        let switched_index_for_callback = switched_index.clone();
        window.on_switch_saved_server(move |index| switched_index_for_callback.set(index));
        let server_pickers: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::server-picker").collect();
        assert_eq!(server_pickers.len(), 1);
        server_pickers[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert!(window.get_show_server_menu());
        let saved_server_actions: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::saved-server-touch").collect();
        assert_eq!(saved_server_actions.len(), 1);
        saved_server_actions[0]
            .single_click(PointerEventButton::Left)
            .await;
        assert_eq!(switched_index.get(), 0);

        window.set_show_main(false);
        let recent_index = Rc::new(Cell::new(-1));
        let recent_index_for_callback = recent_index.clone();
        window.on_connect_saved_server(move |index| recent_index_for_callback.set(index));
        let recent_rows: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::recent-touch").collect();
        assert_eq!(recent_rows.len(), 1);
        recent_rows[0].single_click(PointerEventButton::Left).await;
        assert_eq!(recent_index.get(), 0);
        let edit_buttons: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::edit-recent").collect();
        assert_eq!(edit_buttons.len(), 1);
        edit_buttons[0].single_click(PointerEventButton::Left).await;
        assert_eq!(window.get_editing_server_index(), 0);
        assert_eq!(window.get_editing_server_name(), "Vocal Hub");
        let edit_cards: Vec<_> =
            ElementHandle::find_by_element_id(&window, "AppWindow::server-edit-card").collect();
        assert_eq!(edit_cards.len(), 1);
        assert_eq!(edit_cards[0].size().width, 234.0);
        assert_eq!(edit_cards[0].absolute_position().x, 8.0);

        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}

fn send_shortcut(window: &AppWindow, modifier: Key, shift: bool, key: &str) {
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: modifier.into(),
    });
    if shift {
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    }
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    if shift {
        window.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: modifier.into(),
    });
}
