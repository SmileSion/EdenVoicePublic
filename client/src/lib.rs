pub mod application;
pub mod audio;
pub mod avatar;
pub mod backup;
pub mod codec;
pub mod config;
pub mod game_catalog;
pub mod control;
pub mod devices;
pub mod dsp;
pub mod jitter;
pub mod metrics;
pub mod mixer;
pub mod model;
pub mod network;
pub mod peer;
pub mod platform;
pub mod protocol;
pub mod security;
pub mod shortcuts;
pub mod transport;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use application::{
    AppCommand, Application, AudioPreferences, AudioState, ConnectionState, InputMode,
    MicTestStatus, NoiseSuppressionMode, NoiseSuppressionStrength,
};
use avatar::{CropDraft, image_for_avatar, is_custom_avatar};
use config::{SavedServer, ShortcutBindings};
use network::RoomNetworkPolicy;
use shortcuts::{
    CAPTURE_WINDOW_MS, CaptureProgress, ShortcutRecorder, binding_from_key_event, display_binding,
};
use slint::{Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

const NETWORK_POLICY_CAPABILITY: &str = "network.policy.set";

// Keep the UI inline so Rust callbacks and Slint properties are checked together.

slint::slint! {
    import { Slider } from "std-widgets.slint";

    // Shared visual tokens for the compact, voice-first reference layout.
    global EvTheme {
        out property <color> canvas: #ffffff;
        out property <color> sidebar: #fbfbfc;
        out property <color> divider: #ececf0;
        out property <color> accent: #5261e6;
        out property <color> text-primary: #20232c;
        out property <color> text-secondary: #7d8390;
        out property <length> radius-sm: 6px;
        out property <length> radius-md: 7px;
        out property <length> control-height: 40px;
    }

    struct TreeItem { label: string, detail: string, count: string, avatar: string, avatar-image: image, custom-avatar: bool, channel-index: int, session-id: string, user-limit: string, depth: int, is-channel: bool, selected: bool, speaking: bool, muted: bool, admin-muted: bool, server-muted: bool, channel-muted: bool, deafened: bool, locally-muted: bool, volume: float, server-role: string, channel-role: string }
    struct MemberItem { name: string, avatar: string, avatar-image: image, custom-avatar: bool, session-id: string, speaking: bool, muted: bool, admin-muted: bool, server-muted: bool, channel-muted: bool, deafened: bool, locally-muted: bool, volume: float, server-role: string, channel-role: string }
    struct CompactChannelItem { label: string, count: string, channel-index: int, depth: int, selected: bool }
    struct SavedServerItem { name: string, address: string, nickname: string, avatar: string, avatar-image: image, custom-avatar: bool }
    struct BanItem { client-id: string, display-name: string, reason: string, created-at: string }
    struct ChatItem { name: string, avatar: string, avatar-image: image, custom-avatar: bool, time: string, text: string, own: bool }

    component Icon inherits Path {
        in property <string> name;
        stroke: #505664; stroke-width: 1.8px; fill: transparent;
        commands: name == "search" ? "M11 4a7 7 0 1 0 0 14a7 7 0 1 0 0-14M16 16l5 5" :
                  name == "mic" ? "M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3M5 11a7 7 0 0 0 14 0M12 18v3M8 21h8" :
                  name == "mic-off" ? "M12 19v3M8 22h8M19 10v2a7 7 0 0 1-.6 2.8M15 9.4V6a3 3 0 0 0-5.1-2.1M9 9v3a3 3 0 0 0 5.1 2.1M5 11a7 7 0 0 0 9.8 6.4M2 2l20 20" :
                  name == "lock" ? "M7 11V8a5 5 0 0 1 10 0v3M5 11h14v10H5z" :
                  name == "volume" ? "M4 10h4l5-4v12l-5-4H4zM16 9a4 4 0 0 1 0 6M19 6a8 8 0 0 1 0 12" :
                  name == "volume-off" ? "M11 5L6 9H2v6h4l5 4V5M22 9l-6 6M16 9l6 6" :
                  name == "settings" ? "M12 2l1.1 2.3 2.5.6 2-1.5 2.1 2.1-1.5 2 .6 2.5L22 12l-2.3 1.1-.6 2.5 1.5 2-2.1 2.1-2-1.5-2.5.6L12 22l-1.1-2.3-2.5-.6-2 1.5-2.1-2.1 1.5-2-.6-2.5L2 12l2.3-1.1.6-2.5-1.5-2 2.1-2.1 2 1.5 2.5-.6zM12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6" :
                  name == "signal" ? "M5 20v-3M10 20v-7M15 20v-11M20 20V4" :
                  name == "users" ? "M9 11a4 4 0 1 0 0-8a4 4 0 1 0 0 8M2 21v-2a6 6 0 0 1 12 0v2M16 3a4 4 0 0 1 0 8M16 15a6 6 0 0 1 6 6" :
                  name == "topology" ? "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 8v8M8 12h8" :
                  name == "sfu" ? "M12 3v18M3 12h18M6 6l12 12M18 6L6 18" :
                  name == "p2p" ? "M5 5a2 2 0 1 0 0 4a2 2 0 1 0 0-4M17 5a2 2 0 1 0 0 4a2 2 0 1 0 0-4M10 17a2 2 0 1 0 0 4a2 2 0 1 0 0-4M7 9l4 7M17 9l-4 7M9 7h6" :
                  name == "hybrid" ? "M5 4v5a5 5 0 0 0 5 5h9M16 10l4 4-4 4M19 4h-4a5 5 0 0 0-5 5v5" :
                  name == "star" ? "M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8-6.2-3.2L5.8 22 7 15.2 2 10.3l6.9-1z" :
                  name == "more" ? "M12 5a1 1 0 1 0 0 2a1 1 0 1 0 0-2M12 11a1 1 0 1 0 0 2a1 1 0 1 0 0-2M12 17a1 1 0 1 0 0 2a1 1 0 1 0 0-2" :
                  name == "headphones" ? "M4 14v-2a8 8 0 0 1 16 0v2M4 14h3v6H5a2 2 0 0 1-2-2v-2a2 2 0 0 1 1-2M20 14h-3v6h2a2 2 0 0 0 2-2v-2a2 2 0 0 0-1-2" :
                  name == "refresh" ? "M20 7v5h-5M4 17v-5h5M6.1 8a7 7 0 0 1 11.4-2L20 9M4 15l2.5 3a7 7 0 0 0 11.4-2" :
                  name == "play" ? "M8 5v14l11-7z" :
                  name == "close" ? "M6 6l12 12M18 6L6 18" :
                  name == "expand" ? "M8 3H3v5M16 3h5v5M8 21H3v-5M16 21h5v-5M3 8l6-6M21 8l-6-6M3 16l6 6M21 16l-6 6" :
                  name == "edit" ? "M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4z" :
                  name == "plus" ? "M12 5v14M5 12h14" :
                  name == "check" ? "M5 12l4 4L19 6" :
                  "M6 9l6 6l6-6";
    }

    component RoundIconButton inherits TouchArea {
        in property <string> icon-name;
        in property <color> icon-color: #505664;
        in property <length> icon-size: 18px;
        width: 34px; height: 34px;
        Rectangle { border-radius: 7px; background: root.pressed ? #eceef5 : root.has-hover ? #f4f5f8 : transparent; Icon { name: root.icon-name; stroke: root.icon-color; x: (root.width - self.width) / 2; y: (root.height - self.height) / 2; width: root.icon-size; height: root.icon-size; } }
    }

    component FormLineEdit inherits Rectangle {
        in-out property <string> text;
        in property <string> placeholder-text;
        in property <bool> enabled: true;
        callback accepted();

        height: 39px;
        border-radius: 7px;
        border-width: 1px;
        border-color: !root.enabled ? #e1e3e9 : editor.has-focus ? rgb(98, 104, 238) : #cfd3dc;
        background: root.enabled ? #ffffff : #f5f6f8;
        editor := TextInput {
            x: 12px;
            width: parent.width - 24px;
            height: parent.height;
            text <=> root.text;
            enabled: root.enabled;
            color: root.enabled ? #272b35 : #9297a2;
            selection-background-color: rgb(98, 104, 238);
            selection-foreground-color: #ffffff;
            font-size: 12px;
            vertical-alignment: center;
            single-line: true;
            key-pressed(event) => {
                if (event.modifiers.meta && !event.modifiers.control) {
                    if (!event.modifiers.shift && (event.text == "a" || event.text == "A")) {
                        editor.select-all();
                        return accept;
                    }
                    if (!event.modifiers.shift && (event.text == "c" || event.text == "C")) {
                        editor.copy();
                        return accept;
                    }
                    if (!event.modifiers.shift && (event.text == "x" || event.text == "X")) {
                        editor.cut();
                        return accept;
                    }
                    if (!event.modifiers.shift && (event.text == "v" || event.text == "V")) {
                        editor.paste();
                        return accept;
                    }
                    if (!event.modifiers.shift && (event.text == "z" || event.text == "Z")) {
                        editor.undo();
                        return accept;
                    }
                    if (event.modifiers.shift && (event.text == "z" || event.text == "Z")) {
                        editor.redo();
                        return accept;
                    }
                }
                reject
            }
            accepted => { root.accepted(); }
        }

        Text {
            x: 12px;
            width: parent.width - 24px;
            height: parent.height;
            visible: root.text == "";
            text: root.placeholder-text;
            color: root.enabled ? #9297a2 : #b0b4bd;
            font-size: 12px;
            vertical-alignment: center;
        }
    }

    component FormComboBox inherits Rectangle {
        in property <[string]> model;
        in-out property <int> current-index: -1;
        in property <bool> enabled: true;

        height: 34px;
        border-radius: 7px;
        border-width: 1px;
        border-color: root.enabled ? #cfd3dc : #e1e3e9;
        background: root.enabled ? (combo-touch.pressed ? #f1f2f6 : combo-touch.has-hover ? #f7f8fa : #ffffff) : #f5f6f8;

        Text {
            x: 11px;
            width: parent.width - 38px;
            height: parent.height;
            text: root.current-index >= 0 && root.current-index < root.model.length ? root.model[root.current-index] : "";
            color: root.enabled ? #272b35 : #9297a2;
            font-size: 11px;
            vertical-alignment: center;
            overflow: elide;
        }
        Icon {
            name: "chevron";
            stroke: root.enabled ? #737987 : #adb1ba;
            width: 15px;
            height: 15px;
            x: parent.width - 25px;
            y: (parent.height - self.height) / 2;
        }
        combo-touch := TouchArea {
            enabled: root.enabled;
            clicked => { choices-popup.show(); }
        }

        choices-popup := PopupWindow {
            x: 0px;
            y: root.height + 4px;
            width: root.width;
            height: root.model.length * 32px + 8px;
            Rectangle {
                background: #ffffff;
                border-radius: 7px;
                border-width: 1px;
                border-color: #d6d9e1;
                drop-shadow-color: #1f243026;
                drop-shadow-blur: 14px;
                drop-shadow-offset-y: 4px;
                VerticalLayout {
                    padding-top: 4px;
                    padding-bottom: 4px;
                    for option[index] in root.model : Rectangle {
                        height: 32px;
                        border-radius: 5px;
                        background: index == root.current-index ? #e8ebff : option-touch.has-hover ? #f2f3f7 : transparent;
                        Text {
                            x: 10px;
                            width: parent.width - 20px;
                            text: option;
                            color: index == root.current-index ? #4d5bd5 : #303540;
                            font-size: 11px;
                            font-weight: index == root.current-index ? 600 : 400;
                            vertical-alignment: center;
                            overflow: elide;
                        }
                        option-touch := TouchArea {
                            clicked => {
                                root.current-index = index;
                                choices-popup.close();
                            }
                        }
                    }
                }
            }
        }
    }

    component FormButton inherits Rectangle {
        in property <string> text;
        in property <bool> enabled: true;
        in property <bool> primary: false;
        callback clicked();

        height: 31px;
        border-radius: 6px;
        border-width: root.primary ? 0px : 1px;
        border-color: root.enabled ? #d5d8e0 : #e5e7eb;
        background: !root.enabled ? #f7f8fa : root.primary ? (button-touch.pressed ? #4f55d5 : button-touch.has-hover ? #565de0 : #5e64e9) : button-touch.pressed ? #e9ebf1 : button-touch.has-hover ? #f3f4f7 : #ffffff;

        Text {
            text: root.text;
            color: !root.enabled ? #a9adb6 : root.primary ? #ffffff : #303540;
            font-size: 11px;
            font-weight: root.primary ? 600 : 500;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        button-touch := TouchArea {
            enabled: root.enabled;
            accessible-role: button;
            accessible-label: root.text;
            clicked => { root.clicked(); }
        }
    }

    component FormCheckBox inherits Rectangle {
        in property <string> text;
        in-out property <bool> checked: false;
        in property <bool> enabled: true;

        height: 28px;
        background: transparent;
        check-mark := Rectangle {
            x: 0px;
            width: 17px;
            height: 17px;
            y: (parent.height - self.height) / 2;
            border-radius: 4px;
            border-width: root.checked ? 0px : 1px;
            border-color: root.enabled ? #b8bdc8 : #d4d7de;
            background: !root.enabled ? #eceef2 : root.checked ? rgb(98, 104, 238) : #ffffff;
            Icon {
                name: "check";
                stroke: root.checked && root.enabled ? #ffffff : transparent;
                stroke-width: 2.2px;
                width: 11px;
                height: 11px;
                x: 3px;
                y: 3px;
            }
        }
        Text {
            x: 27px;
            width: parent.width - 27px;
            text: root.text;
            color: root.enabled ? #343944 : #9da2ad;
            font-size: 11px;
            vertical-alignment: center;
            overflow: elide;
        }
        check-touch := TouchArea {
            enabled: root.enabled;
            accessible-role: checkbox;
            accessible-label: root.text;
            accessible-checked: root.checked;
            clicked => { root.checked = !root.checked; }
        }
    }

    component ShortcutRow inherits Rectangle {
        in property <string> icon-name;
        in property <string> label;
        in property <string> description;
        in property <string> shortcut;
        in property <bool> recording: false;
        callback record(); callback clear();
        height: 43px; border-radius: 7px;
        background: #fafbfc;
        row-layout := HorizontalLayout { padding-left: 9px; padding-right: 9px; spacing: 8px;
            icon-container := Rectangle { width: 25px; background: transparent;
                icon-tile := Rectangle { y: (parent.height - 25px) / 2; width: 25px; height: 25px; border-radius: 6px; background: #e8ebff; Icon { name: root.icon-name; stroke: #5261df; width: 14px; height: 14px; x: 5.5px; y: 5.5px; } }
            }
            label-stack := VerticalLayout { alignment: center; spacing: 1px;
                title-text := Text { height: 13px; text: root.label; font-size: 10px; font-weight: 600; color: #303540; vertical-alignment: center; }
                description-text := Text { height: 11px; text: root.description; font-size: 8px; color: #858b97; vertical-alignment: center; }
            }
            Rectangle { horizontal-stretch: 1; }
            binding-container := Rectangle { width: 106px; background: transparent;
                binding-control := Rectangle { y: (parent.height - 23px) / 2; width: parent.width; height: 23px; border-radius: 6px; border-width: 1px; border-color: root.recording ? rgb(113, 128, 238) : #dfe2e9; background: root.recording ? #f0f2ff : #ffffff;
                    Text { text: root.recording ? "请按键…" : root.shortcut; font-size: 8px; font-weight: 600; color: root.recording ? #5261df : #555c69; horizontal-alignment: center; vertical-alignment: center; }
                    TouchArea { accessible-role: button; accessible-label: "绑定 " + root.label; clicked => { root.record(); } }
                }
            }
            clear-container := Rectangle { width: 29px; background: transparent;
                clear-text := Text { text: root.shortcut == "未绑定" ? "" : "清除"; font-size: 8px; color: #8a909c; horizontal-alignment: center; vertical-alignment: center; }
                TouchArea { accessible-role: button; accessible-label: "清除 " + root.label; enabled: root.shortcut != "未绑定"; clicked => { root.clear(); } }
            }
        }
    }

    component MenuAction inherits Rectangle {
        in property <string> label;
        in property <bool> danger: false;
        in property <bool> enabled: true;
        callback selected();
        height: 34px; border-radius: 6px;
        background: root.enabled ? (menu-touch.pressed ? #e8eaf0 : menu-touch.has-hover ? #f1f2f6 : transparent) : transparent;
        Text { x: 10px; width: parent.width - 20px; text: root.label; font-size: 11px; color: !root.enabled ? #a7abb4 : root.danger ? #d65359 : #343944; vertical-alignment: center; }
        menu-touch := TouchArea { enabled: root.enabled; clicked => { root.selected(); } }
    }

    component DetailRow inherits HorizontalLayout {
        in property <string> label;
        in property <string> value;
        height: 28px;
        Text { text: root.label; font-size: 11px; color: #737986; vertical-alignment: center; }
        Rectangle { horizontal-stretch: 1; }
        Text { text: root.value; font-size: 11px; font-weight: 600; color: #303540; vertical-alignment: center; horizontal-alignment: right; }
    }

    component NetworkPolicyCard inherits Rectangle {
        in property <bool> selected: false;
        in property <bool> enabled: true;
        in property <string> title;
        in property <string> description;
        callback chosen();
        height: 56px; border-radius: 7px;
        background: root.enabled && policy-touch.has-hover ? #f7f8fc : transparent;
        policy-touch := TouchArea { enabled: root.enabled; clicked => { root.chosen(); } }
        Rectangle { width: 16px; height: 16px; x: 8px; y: 10px; border-radius: 8px; border-width: 2px; border-color: !root.enabled ? #d3d6dd : root.selected ? #5967e8 : #a8adb8;
            Rectangle { width: 8px; height: 8px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2; border-radius: 4px; background: root.enabled && root.selected ? #5967e8 : transparent; }
        }
        Text { x: 34px; y: 6px; width: parent.width - 42px; height: 22px; text: root.title; font-size: 11px; font-weight: 700; color: root.enabled ? rgb(48, 52, 62) : #a4a9b3; vertical-alignment: center; overflow: elide; }
        Text { x: 34px; y: 29px; width: parent.width - 42px; height: 20px; text: root.description; font-size: 9px; color: root.enabled ? #707684 : #b9bdc5; vertical-alignment: top; wrap: word-wrap; }
    }

    component EvAvatar inherits Rectangle {
        in property <string> avatar;
        in property <image> avatar-image;
        in property <bool> custom-avatar: false;
        in property <bool> speaking: false;
        in property <length> avatar-size: 20px;
        width: root.avatar-size; height: root.avatar-size;
        border-radius: root.avatar-size / 2;
        clip: true;
        background: root.speaking ? #d9f3e5 : root.avatar == "L" ? #f2dfd6 : root.avatar == "K" ? #dcebf6 : root.avatar == "M" ? #e7dff2 : root.avatar == "R" ? #e2eee1 : root.avatar == "E" ? #f2ead7 : #dce3f7;
        if (root.custom-avatar) : Image { source: root.avatar-image; width: parent.width; height: parent.height; image-fit: cover; }
        if (root.avatar == "L") : Image { source: @image-url("../assets/ui/avatar-lumi-v1.png"); width: parent.width; height: parent.height; image-fit: cover; }
        if (root.avatar == "K") : Image { source: @image-url("../assets/ui/avatar-kira-v1.png"); width: parent.width; height: parent.height; image-fit: cover; }
        if (root.avatar == "M") : Image { source: @image-url("../assets/ui/avatar-moon-v1.png"); width: parent.width; height: parent.height; image-fit: cover; }
        if (root.avatar == "R") : Image { source: @image-url("../assets/ui/avatar-ray-v1.png"); width: parent.width; height: parent.height; image-fit: cover; }
        if (root.avatar == "E") : Image { source: @image-url("../assets/ui/avatar-echo-v1.png"); width: parent.width; height: parent.height; image-fit: cover; }
        if (!root.custom-avatar && root.avatar != "L" && root.avatar != "K" && root.avatar != "M" && root.avatar != "R" && root.avatar != "E") : Text { text: root.avatar; font-size: root.avatar-size * 0.4; color: root.speaking ? #258b56 : #5260c8; horizontal-alignment: center; vertical-alignment: center; }
    }

    component ReferenceMetricSlider inherits Rectangle {
        in property <string> label;
        in property <string> value-label;
        in property <float> progress;
        height: 35px;
        Text { x: 0; y: 0; height: 13px; text: root.label; font-size: 8px; color: #626875; vertical-alignment: center; }
        Text { x: parent.width - 34px; y: 0; width: 34px; height: 13px; text: root.value-label; font-size: 7px; color: #737987; horizontal-alignment: right; vertical-alignment: center; }
        Rectangle { x: 0; y: 22px; width: parent.width; height: 3px; border-radius: 2px; background: #e1e4eb;
            Rectangle { width: parent.width * root.progress; height: parent.height; border-radius: 2px; background: #5261e6; }
            Rectangle { x: parent.width * root.progress - 4px; y: -3px; width: 9px; height: 9px; border-radius: 5px; background: #aab2ed; border-width: 1px; border-color: #ffffff; }
        }
    }

    component ReferenceShortcutRow inherits Rectangle {
        in property <string> label;
        in property <string> shortcut;
        height: 25px;
        Text { x: 2px; width: parent.width * 0.46; height: parent.height - 1px; text: root.label; font-size: 8px; color: #4b505c; vertical-alignment: center; }
        Text { x: parent.width * 0.48; width: parent.width * 0.5 - 2px; height: parent.height - 1px; text: root.shortcut; font-size: 8px; color: #626875; vertical-alignment: center; }
        Rectangle { y: parent.height - 1px; width: parent.width; height: 1px; background: #eceef2; }
    }

    component TreeRow inherits Rectangle {
        in property <TreeItem> item;
        in property <bool> pending: false;
        in property <bool> focused: false;
        callback selected(int);
        callback activated(int);
        callback context-requested(TreeItem);
        height: root.item.is-channel ? 28px : 26px;
        border-radius: 6px;
        accessible-role: button;
        accessible-label: root.item.label;
        accessible-action-default => { if (root.item.is-channel && root.item.channel-index >= 0) { root.activated(root.item.channel-index); } }
        background: root.item.selected || root.pending || root.focused ? #e7eaff : row-touch.has-hover ? #f1f2f6 : transparent;
        tree-row-layout := HorizontalLayout { padding-left: (root.item.is-channel ? 9px : 18px) + root.item.depth * 8px; padding-right: 7px; spacing: 7px;
            if (root.item.is-channel && root.item.depth > 0) : tree-row-branch := Rectangle { width: 8px; height: parent.height; background: transparent;
                Rectangle { x: 0px; y: 0px; width: 1px; height: parent.height / 2; background: #c9cdd7; }
                Rectangle { x: 0px; y: parent.height / 2; width: 8px; height: 1px; background: #c9cdd7; }
            }
            if (!root.item.is-channel) : Rectangle { width: 16px; height: parent.height; background: transparent;
                EvAvatar { avatar: root.item.avatar; avatar-image: root.item.avatar-image; custom-avatar: root.item.custom-avatar; speaking: root.item.speaking; avatar-size: 16px; y: (parent.height - self.height) / 2; }
            }
            if (root.item.is-channel) : Rectangle { width: 13px; height: parent.height; background: transparent;
                Icon { name: root.item.selected ? "plus" : "chevron"; stroke: root.item.selected ? #5765df : #747a88; width: 13px; height: 13px; y: (parent.height - self.height) / 2; }
            }
            tree-row-label := Text { text: root.item.label; height: parent.height; font-size: 10px; font-weight: root.item.is-channel ? (root.item.depth == 0 ? 650 : 550) : 500; color: root.item.selected || root.pending ? #4355d2 : root.item.is-channel && root.item.depth > 0 ? #555b68 : #3b404b; vertical-alignment: center; }
            Rectangle { horizontal-stretch: 1; }
            if (root.item.is-channel && root.item.muted) : Icon { name: "volume-off"; stroke: #8d929e; width: 13px; height: 13px; }
            if (root.item.is-channel && root.item.count != "" && root.item.count != "0") : Rectangle { width: 18px; height: parent.height; background: transparent;
                Rectangle { width: 18px; height: 18px; y: (parent.height - self.height) / 2; border-radius: 9px; background: root.item.selected || root.pending ? #ccd3ff : #eff0f3; Text { text: root.item.count; font-size: 8px; color: #6170d4; horizontal-alignment: center; vertical-alignment: center; } }
            }
            if (!root.item.is-channel && root.item.speaking) : Rectangle { width: 15px; height: parent.height; background: transparent;
                Icon { name: "signal"; stroke: rgb(47, 168, 104); width: 15px; height: 15px; y: (parent.height - self.height) / 2 + 2px; }
            }
            if (!root.item.is-channel && root.item.locally-muted) : Rectangle { width: 14px; height: parent.height; background: transparent;
                Icon { name: "volume-off"; stroke: #d05f65; width: 14px; height: 14px; y: (parent.height - self.height) / 2; }
            }
            if (!root.item.is-channel && root.item.muted) : Rectangle { width: 14px; height: parent.height; background: transparent;
                Icon { name: "mic-off"; stroke: #979ca8; width: 14px; height: 14px; y: (parent.height - self.height) / 2; }
            }
        }
        row-touch := TouchArea {
            clicked => { if (root.item.is-channel && root.item.channel-index >= 0) { root.selected(root.item.channel-index); } }
            double-clicked => { if (root.item.is-channel && root.item.channel-index >= 0) { root.activated(root.item.channel-index); } }
            pointer-event(event) => {
                if (event.button == PointerEventButton.right && event.kind == PointerEventKind.down) {
                    root.context-requested(root.item);
                }
            }
        }
    }

    export component AppWindow inherits Window {
        in property <string> ui-font-family: "";
        default-font-family: root.ui-font-family;
        in property <bool> macos-titlebar-overlay: false;
        width: root.info-panel-only ? 206px : (!root.show-main && !root.show-settings ? 250px : root.show-settings ? 456px : root.window-mode == 1 ? 306px : 456px);
        height: root.info-panel-only ? 450px : (root.show-settings ? (root.visual-demo ? 280px : (root.macos-titlebar-overlay ? 450px : 420px)) : root.show-main ? (root.macos-titlebar-overlay ? 450px : 420px) : 450px);
        title: "";
        in property <[TreeItem]> tree-items;
        in property <[MemberItem]> members;
        in property <[MemberItem]> compact-avatar-members;
        in property <[CompactChannelItem]> compact-channels;
        in property <int> compact-member-overflow: 0;
        in property <[SavedServerItem]> saved-servers;
        in property <[SavedServerItem]> recent-servers;
        in property <[BanItem]> bans;
        in property <[ChatItem]> chat-items;
        in property <int> ban-total: 0;
        in property <string> topology: "SFU";
        in property <string> latency: "24ms";
        in property <string> control-latency: "--ms";
        in property <string> network-quality: "good";
        in property <string> codec-label: "OPUS  48kHz";
        property <length> topbar-height: root.macos-titlebar-overlay ? 40px : 28px;
        property <color> quality-color: root.network-quality == "critical" ? rgb(224, 72, 78) : root.network-quality == "poor" ? rgb(232, 142, 43) : root.network-quality == "fair" ? rgb(216, 172, 42) : rgb(53, 173, 104);
        in property <string> server-name: "voice.example.com";
        in property <string> app-version: "0.8.5";
        in property <string> channel-name: "游戏频道";
        in property <string> output-device: "系统默认输出";
        in-out property <string> server-address: "127.0.0.1:9000";
        in-out property <string> nickname: "";
        in-out property <string> self-avatar: "";
        in-out property <image> self-avatar-image;
        in-out property <bool> self-custom-avatar: false;
        in property <string> self-initials: "?";
        in-out property <string> chat-draft: "";
        in-out property <bool> show-avatar-picker: false;
        in-out property <bool> show-avatar-crop: false;
        in-out property <image> avatar-crop-image;
        in-out property <float> avatar-crop-width: 1;
        in-out property <float> avatar-crop-height: 1;
        in-out property <float> avatar-crop-zoom: 1;
        in-out property <float> avatar-crop-offset-x: 0;
        in-out property <float> avatar-crop-offset-y: 0;
        in-out property <bool> avatar-crop-dragging: false;
        in-out property <float> avatar-crop-last-x: 0;
        in-out property <float> avatar-crop-last-y: 0;
        in-out property <bool> remember-server: true;
        in property <bool> show-main: false;
        in property <bool> can-create-channels: false;
        in property <bool> can-edit-channels: false;
        in property <bool> can-delete-channels: false;
        in property <bool> can-admin-mute-users: false;
        in property <bool> can-kick-channel-users: false;
        in property <bool> can-kick-server-users: false;
        in property <bool> can-ban-users: false;
        in property <bool> can-unban-users: false;
        in property <bool> can-move-users: false;
        in property <bool> can-set-server-roles: false;
        in property <bool> can-set-channel-roles: false;
        in property <[bool]> network-policy-channel-access;
        in property <[bool]> channel-administration-access;
        in property <string> server-role: "";
        in-out property <bool> show-master-provision: false;
        in-out property <bool> show-ban-manager: false;
        in-out property <int> ban-page: 0;
        in-out property <string> master-provision-token: "";
        in property <string> trust-endpoint: "";
        in property <string> trust-fingerprint: "";
        in-out property <int> channel-editor-mode: 0;
        in-out property <int> channel-editor-index: -1;
        in-out property <string> channel-editor-name: "";
        in-out property <string> channel-editor-user-limit: "0";
        // Development-only presentation state. It is set by ui_preview and
        // never changes the live voice or network state.
        in property <bool> visual-demo: false;
        in property <bool> connecting: false;
        in property <string> connection-status: "";
        in property <string> connection-error: "";
        in-out property <int> pending-channel-index: -1;
        in-out property <int> selected-channel-index: -1;
        in property <bool> mic-muted: false;
        in property <bool> mic-admin-muted: false;
        in property <bool> speaker-muted: false;
        in-out property <int> window-mode: 0;
        in-out property <bool> show-main-menu: false;
        in-out property <int> context-menu-kind: 0;
        in-out property <int> context-channel-index: -1;
        in-out property <string> context-channel-name: "";
        in-out property <string> context-channel-user-limit: "0";
        in-out property <string> context-user-session: "";
        in property <string> self-session-id: "";
        in property <int> own-channel-index: -1;
        in-out property <int> context-user-channel-index: -1;
        in-out property <string> context-user-name: "";
        in-out property <string> context-user-avatar: "";
        in-out property <image> context-user-avatar-image;
        in-out property <bool> context-user-custom-avatar: false;
        in-out property <string> context-user-channel: "";
        in-out property <bool> context-user-speaking: false;
        in-out property <bool> context-user-muted: false;
        in-out property <bool> context-user-admin-muted: false;
        in-out property <bool> context-user-server-muted: false;
        in-out property <bool> context-user-channel-muted: false;
        in-out property <bool> context-user-deafened: false;
        in-out property <bool> context-user-locally-muted: false;
        in-out property <float> context-user-volume: 1.0;
        in-out property <string> context-user-server-role: "";
        in-out property <string> context-user-channel-role: "";
        in-out property <bool> show-user-card: false;
        in-out property <bool> show-member-list: false;
        in-out property <bool> show-info-panel: false;
        in-out property <bool> info-panel-only: false;
        in-out property <bool> show-notifications: false;
        in-out property <bool> show-server-menu: false;
        in-out property <bool> show-output-menu: false;
        in-out property <int> editing-server-index: -1;
        in-out property <string> editing-server-name: "";
        in-out property <string> editing-server-address: "";
        in-out property <string> editing-server-nickname: "";
        in-out property <bool> show-network-details: false;
        in-out property <int> network-details-tab: 0;
        in property <string> packet-loss-label: "--";
        in property <string> jitter-label: "--";
        in property <string> uplink-label: "--";
        in property <string> downlink-label: "--";
        in property <string> connection-duration-label: "--";
        in property <string> input-device: "系统默认麦克风";
        in property <string> input-sample-rate-label: "自动";
        in property <string> output-sample-rate-label: "自动";
        in property <string> dsp-label: "关闭";
        in property <string> aec-label: "关闭";
        in property <string> agc-label: "关闭";
        in-out property <bool> show-settings: false;
        in-out property <int> settings-page: 0;
        in-out property <int> room-network-policy-index: 0;
        in-out property <int> room-network-channel-index: -1;
        in-out property <string> room-network-channel-name: "";
        out property <bool> can-set-context-network-policy: root.context-channel-index >= 0 && root.context-channel-index < root.network-policy-channel-access.length && root.network-policy-channel-access[root.context-channel-index];
        out property <bool> can-set-target-network-policy: root.room-network-channel-index >= 0 && root.room-network-channel-index < root.network-policy-channel-access.length && root.network-policy-channel-access[root.room-network-channel-index];
        out property <bool> can-administer-context-channel: root.context-channel-index >= 0 && root.context-channel-index < root.channel-administration-access.length && root.channel-administration-access[root.context-channel-index];
        out property <bool> can-administer-context-user: root.context-user-channel-index >= 0 && root.context-user-channel-index < root.channel-administration-access.length && root.channel-administration-access[root.context-user-channel-index];
        out property <bool> can-administer-own-channel: root.own-channel-index >= 0 && root.own-channel-index < root.channel-administration-access.length && root.channel-administration-access[root.own-channel-index];
        in property <[string]> input-devices;
        in property <[string]> output-devices;
        in-out property <int> input-device-index: 0;
        in-out property <int> output-device-index: 0;
        in-out property <int> input-mode-index: 0;
        in-out property <int> noise-mode-index: 1;
        in-out property <int> noise-strength-index: 1;
        in-out property <float> input-gain: 1.0;
        in-out property <float> output-volume: 1.0;
        in-out property <bool> echo-cancellation: true;
        in-out property <bool> automatic-gain: true;
        in-out property <string> ptt-key: "";
        in-out property <string> shortcut-toggle-window: "未绑定";
        in-out property <string> shortcut-toggle-mic: "未绑定";
        in-out property <string> shortcut-toggle-speaker: "未绑定";
        in-out property <string> shortcut-push-to-talk: "未绑定";
        in-out property <string> shortcut-toggle-members: "未绑定";
        in-out property <int> recording-shortcut: -1;
        in property <float> input-level: 0.0;
        in property <string> mic-test-status: "空闲";
        in property <string> audio-status: "";
        in property <string> settings-error: "";
        callback connect(string, string, bool); callback disconnect();
        callback connect-saved-server(int); callback switch-saved-server(int);
        callback save-saved-server(int, string, string, string);
        callback toggle-mic(); callback toggle-speaker(); callback open-settings(); callback join-channel(int);
        callback set-user-volume(string, float); callback set-user-local-mute(string, bool); callback mute-channel(int);
        callback admin-mute-user(string, bool); callback kick-user-channel(string); callback kick-user-server(string); callback ban-user(string); callback delete-channel(int);
        callback request-bans(int); callback unban-client(string, int);
        callback move-user-to-current-channel(string); callback set-user-server-admin(string, bool); callback set-user-channel-admin(string, bool);
        callback provision-master(string);
        callback send-chat(string);
        callback set-avatar(string);
        callback choose-avatar-image();
        callback save-avatar-crop(float, float, float);
        callback trust-server(string, string);
        callback save-channel-editor(int, int, string, string);
        callback refresh-devices(); callback save-audio-settings(); callback close-settings();
        callback select-output-device(int);
        callback play-output-test(); callback start-mic-test(); callback play-mic-original(); callback play-mic-processed(); callback stop-mic-test();
        callback begin-shortcut-capture(int); callback clear-shortcut(int);
        callback shortcut-key-pressed(string, bool, bool, bool, bool) -> int;

        background: #ffffff;
        forward-focus: app-shortcuts;
        app-shortcuts := FocusScope {
            width: 0px; height: 0px;
            capture-key-pressed(event) => {
                if (root.show-main) {
                    let action = root.shortcut-key-pressed(event.text, event.modifiers.control, event.modifiers.alt, event.modifiers.shift, event.modifiers.meta);
                    if (action == 1) {
                    root.window-mode = root.window-mode == 0 ? 1 : 0;
                    return accept;
                    }
                    if (action == 2) {
                    root.toggle-mic();
                    return accept;
                    }
                    if (action == 3) {
                    root.toggle-speaker();
                    return accept;
                    }
                    if (action == 5 && root.window-mode == 0) {
                    root.show-member-list = !root.show-member-list;
                    return accept;
                    }
                    if (action >= 90) { return accept; }
                }
                reject
            }
        }
        changed show-main => {
            if (show-main) {
                app-shortcuts.focus();
            }
        }
        if (!root.show-main && !root.info-panel-only) : Rectangle {
            width: parent.width; height: parent.height;
            background: #ffffff;
            Rectangle {
                width: parent.width; height: parent.height;
                background: #ffffff;
                VerticalLayout {
                    padding-left: 16px; padding-right: 16px; padding-top: 52px; padding-bottom: 14px; spacing: 6px;
                    Rectangle { height: 46px; background: transparent;
                        Rectangle { width: 44px; height: 44px; x: (parent.width - self.width) / 2; border-radius: 11px; background: rgb(98, 104, 238);
                            Icon { name: "signal"; stroke: #ffffff; stroke-width: 2.4px; width: 24px; height: 24px; x: 10px; y: 10px; }
                        }
                    }
                    Text { text: "EdenVoice"; font-size: 20px; font-weight: 700; color: #20232c; horizontal-alignment: center; }
                    Text { text: "v" + root.app-version; font-size: 9px; color: #9a9eaa; horizontal-alignment: center; }
                    Rectangle { height: 6px; }
                    FormLineEdit { height: 32px; text <=> root.server-address; placeholder-text: "服务器地址"; enabled: !root.connecting; }
                    FormLineEdit { height: 32px; text <=> root.nickname; placeholder-text: "用户名"; enabled: !root.connecting; accepted => { if (root.server-address != "" && root.nickname != "") { root.connect(root.server-address, root.nickname, root.remember-server); } } }
                    HorizontalLayout { height: 22px; spacing: 6px;
                        remember-server-check := Rectangle { width: 14px; height: 14px; border-radius: 3px; border-width: root.remember-server ? 0px : 1px; border-color: #b9bdc7; background: root.remember-server ? rgb(98, 104, 238) : #ffffff; Icon { name: "check"; stroke: root.remember-server ? #ffffff : transparent; stroke-width: 2.2px; width: 10px; height: 10px; x: 2px; y: 2px; } TouchArea { accessible-role: checkbox; accessible-label: "记住服务器"; accessible-checked: root.remember-server; clicked => { root.remember-server = !root.remember-server; } } }
                        Text { text: "记住我"; font-size: 11px; color: #424754; vertical-alignment: center; TouchArea { clicked => { root.remember-server = !root.remember-server; } } }
                        Rectangle { horizontal-stretch: 1; }
                        RoundIconButton { width: 27px; height: 27px; icon-name: "settings"; clicked => { root.settings-page = 0; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    }
                    connect-button := Rectangle { height: 34px; border-radius: 6px; background: connect-touch.enabled ? (connect-touch.pressed ? #4f55d5 : #5e64e9) : #b7bae8;
                        Text { text: root.connecting ? "正在连接…" : "连接"; color: #ffffff; font-size: 11px; font-weight: 600; horizontal-alignment: center; vertical-alignment: center; }
                        connect-touch := TouchArea { enabled: !root.connecting && root.server-address != "" && root.nickname != ""; clicked => { root.connect(root.server-address, root.nickname, root.remember-server); } }
                    }
                    Text { height: root.connection-status == "" ? 0px : 20px; text: root.connection-status; font-size: 11px; color: #636978; horizontal-alignment: center; }
                    Text { height: root.connection-error == "" ? 0px : 36px; text: root.connection-error; wrap: word-wrap; font-size: 11px; color: #d65359; horizontal-alignment: center; }
                    HorizontalLayout { height: 13px; spacing: 9px;
                        Rectangle { height: parent.height; horizontal-stretch: 1; background: transparent; Rectangle { width: parent.width; height: 1px; y: (parent.height - self.height) / 2; background: #eceef3; } }
                        Text { text: "最近连接"; font-size: 8px; color: #a0a4ae; vertical-alignment: center; }
                        Rectangle { height: parent.height; horizontal-stretch: 1; background: transparent; Rectangle { width: parent.width; height: 1px; y: (parent.height - self.height) / 2; background: #eceef3; } }
                    }
                    for server[index] in root.recent-servers : Rectangle { height: 40px; border-radius: 7px; background: recent-touch.pressed ? #f0f1f7 : recent-touch.has-hover ? #f6f7fa : #fafbfc;
                        recent-touch := TouchArea { enabled: !root.connecting; clicked => { root.connect-saved-server(index); } }
                        HorizontalLayout { padding-left: 9px; padding-right: 5px; spacing: 9px;
                            Rectangle { width: 28px; height: parent.height; background: transparent;
                                Rectangle { width: 28px; height: 28px; y: (parent.height - self.height) / 2; border-radius: 7px; background: #e4e7ff; Text { text: server.avatar; color: #5660df; font-size: 11px; font-weight: 700; horizontal-alignment: center; vertical-alignment: center; } }
                            }
                            VerticalLayout { padding-top: 7px; padding-bottom: 7px; spacing: 1px; Text { text: server.name; font-size: 12px; font-weight: 600; color: rgb(48, 52, 62); overflow: elide; } Text { text: server.address + (server.nickname == "" ? "" : "  ·  " + server.nickname); font-size: 10px; color: #858b98; overflow: elide; } }
                            Rectangle { horizontal-stretch: 1; }
                            Rectangle { width: 28px; height: parent.height; background: transparent;
                                edit-recent := RoundIconButton { width: 28px; height: 28px; y: (parent.height - self.height) / 2; icon-name: "edit"; clicked => { root.editing-server-index = index; root.editing-server-name = server.name; root.editing-server-address = server.address; root.editing-server-nickname = server.nickname; } }
                            }
                        }
                    }
                    Rectangle { vertical-stretch: 1; }
                }
                if (root.visual-demo) : Rectangle { x: 15px; y: 15px; width: 40px; height: 11px; background: transparent;
                    Rectangle { x: 0px; width: 10px; height: 10px; border-radius: 5px; background: #ff6259; }
                    Rectangle { x: 15px; width: 10px; height: 10px; border-radius: 5px; background: #ffbe2f; }
                    Rectangle { x: 30px; width: 10px; height: 10px; border-radius: 5px; background: #29c941; }
                }
            }
        }
        if (root.show-main && root.window-mode == 0) : VerticalLayout {
            width: parent.width; height: parent.height;
            spacing: 0px;
            Rectangle { height: root.topbar-height; background: EvTheme.canvas;
                default-topbar-inner := Rectangle { width: parent.width; height: 28px; y: (parent.height - self.height) / 2; background: transparent;
                    HorizontalLayout { padding-left: root.macos-titlebar-overlay ? 76px : 14px; padding-right: 10px; spacing: 4px;
                        Text { text: "EdenVoice"; font-size: 10px; font-weight: 700; color: #171923; vertical-alignment: center; }
                        Rectangle { width: root.macos-titlebar-overlay ? 36px : 8px; }
                        Rectangle { width: root.topology == "CONNECTING" || root.topology == "NETWORK ERROR" ? 88px : root.topology == "HYBRID" ? 60px : 48px;
                            Rectangle { width: parent.width; height: 23px; y: (parent.height - self.height) / 2; border-radius: 6px; border-width: 1px; border-color: root.topology == "P2P" ? rgb(61, 187, 118) : root.topology == "HYBRID" ? #f09a3e : root.topology == "NETWORK ERROR" ? rgb(224, 72, 78) : root.topology == "CONNECTING" ? #a0a4ae : rgb(101, 112, 237); background: #ffffff; HorizontalLayout { padding-left: 5px; padding-right: 5px; spacing: 4px; Rectangle { width: 11px; height: 11px; border-radius: 3px; background: root.topology == "P2P" ? #35ad68 : root.topology == "HYBRID" ? #f09a3e : root.topology == "NETWORK ERROR" ? #e0484e : root.topology == "CONNECTING" ? #9095a0 : #5967e8; Icon { name: root.topology == "P2P" ? "p2p" : root.topology == "HYBRID" ? "hybrid" : root.topology == "NETWORK ERROR" ? "close" : root.topology == "CONNECTING" ? "refresh" : "sfu"; stroke: #ffffff; stroke-width: 2px; width: 7px; height: 7px; x: 2px; y: 2px; } } Text { text: root.topology; color: root.topology == "P2P" ? rgb(38, 148, 84) : root.topology == "HYBRID" ? #d87a16 : root.topology == "NETWORK ERROR" ? rgb(204, 55, 62) : root.topology == "CONNECTING" ? #777c87 : rgb(76, 92, 222); font-size: 10px; font-weight: 700; vertical-alignment: center; } } }
                        }
                        Rectangle { width: 62px;
                            HorizontalLayout { spacing: 5px; Rectangle { width: 16px; Icon { name: "signal"; stroke: root.quality-color; width: 16px; height: 16px; y: (parent.height - self.height) / 2; } } Text { text: root.latency; font-size: 10px; color: #4d5361; vertical-alignment: center; } }
                        }
                        Rectangle { horizontal-stretch: 1; }
                        RoundIconButton { width: 24px; height: 24px; icon-name: "search"; icon-size: 14px; icon-color: #a5a9b2; enabled: false; accessible-role: button; accessible-label: "搜索功能暂未开放"; }
                        RoundIconButton { width: 24px; height: 24px; icon-name: "users"; icon-size: 14px; accessible-role: button; accessible-label: "显示或隐藏用户列表"; clicked => { root.show-member-list = !root.show-member-list; root.show-main-menu = false; } }
                        avatar-button := Rectangle { width: 24px; height: 24px; EvAvatar { avatar: root.self-avatar; avatar-image: root.self-avatar-image; custom-avatar: root.self-custom-avatar; avatar-size: 20px; x: 2px; y: 2px; } TouchArea { accessible-role: button; accessible-label: "更换头像"; clicked => { root.show-avatar-picker = true; root.show-main-menu = false; } } }
                        RoundIconButton { width: 24px; height: 24px; icon-name: "settings"; icon-size: 14px; clicked => { root.settings-page = 0; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    }
                }
                Rectangle { y: parent.height - 1px; width: parent.width; height: 1px; background: EvTheme.divider; }
            }
            main-body := HorizontalLayout { height: root.height - root.topbar-height - EvTheme.control-height; spacing: 0px;
                left-panel := Rectangle { min-width: 140px; max-width: 140px; background: EvTheme.sidebar; border-width: 1px; border-color: EvTheme.divider;
                    VerticalLayout { padding: 8px; spacing: 0px;
                        server-picker := Rectangle { height: 30px; border-radius: 7px; background: server-picker-touch.pressed ? #e9ebf1 : server-picker-touch.has-hover ? #eff1f5 : #f5f6f8; HorizontalLayout { padding-left: 8px; padding-right: 7px; spacing: 6px; Rectangle { width: 7px; height: 7px; border-radius: 4px; background: #37b96f; } Text { text: root.server-address; font-size: 9px; font-weight: 500; color: #2c303a; vertical-alignment: center; overflow: elide; } Rectangle { horizontal-stretch: 1; } Icon { name: "chevron"; width: 14px; height: 14px; } } server-picker-touch := TouchArea { clicked => { root.show-server-menu = !root.show-server-menu; root.show-main-menu = false; } } }
                        Rectangle { height: 5px; }
                        for item in root.tree-items : TreeRow {
                            item: item;
                            pending: item.is-channel && root.pending-channel-index == item.channel-index;
                            focused: item.is-channel && root.selected-channel-index == item.channel-index;
                            selected(index) => {
                                root.selected-channel-index = index;
                            }
                            activated(index) => {
                                root.selected-channel-index = index;
                                root.pending-channel-index = index;
                                root.join-channel(index);
                            }
                            context-requested(item) => {
                                root.show-main-menu = false;
                                root.show-server-menu = false;
                                root.show-output-menu = false;
                                root.show-user-card = false;
                                if (item.is-channel) {
                                    root.context-menu-kind = 1;
                                    root.context-channel-index = item.channel-index;
                                    root.context-channel-name = item.label;
                                    root.context-channel-user-limit = item.user-limit;
                                } else {
                                    root.context-menu-kind = 2;
                                    root.context-user-session = item.session-id;
                                    root.context-user-channel-index = item.channel-index;
                                    root.context-user-name = item.label;
                                    root.context-user-avatar = item.avatar;
                                    root.context-user-avatar-image = item.avatar-image;
                                    root.context-user-custom-avatar = item.custom-avatar;
                                    root.context-user-channel = item.detail;
                                    root.context-user-speaking = item.speaking;
                                    root.context-user-muted = item.muted;
                                    root.context-user-admin-muted = item.admin-muted;
                                    root.context-user-server-muted = item.server-muted;
                                    root.context-user-channel-muted = item.channel-muted;
                                    root.context-user-deafened = item.deafened;
                                    root.context-user-locally-muted = item.locally-muted;
                                    root.context-user-volume = item.volume;
                                    root.context-user-server-role = item.server-role;
                                    root.context-user-channel-role = item.channel-role;
                                }
                            }
                        }
                        temporary-channel-row := Rectangle { height: 27px; border-radius: 6px; background: transparent;
                            HorizontalLayout { padding-left: 9px; padding-right: 7px; spacing: 7px;
                                Icon { name: "chevron"; stroke: #a0a5af; width: 13px; height: 13px; }
                                Text { text: "临时频道"; font-size: 11px; font-weight: 600; color: #989da8; vertical-alignment: center; }
                                Rectangle { horizontal-stretch: 1; }
                                Text { text: "暂未开放"; font-size: 8px; color: #b0b4bd; vertical-alignment: center; }
                            }
                            TouchArea { enabled: false; accessible-role: button; accessible-label: "临时频道功能暂未开放"; }
                        }
                        Rectangle { vertical-stretch: 1; }
                        Rectangle { height: 31px; background: transparent;
                            Rectangle { x: 0px; y: 1px; width: 29px; height: 29px; border-radius: 7px; background: top-level-channel-button.pressed ? #dfe2e9 : top-level-channel-button.has-hover ? #e7e9ef : #f4f5f7; Icon { name: "plus"; stroke: root.can-create-channels ? #5967e8 : #a4a9b3; width: 17px; height: 17px; x: 6px; y: 6px; } top-level-channel-button := TouchArea { accessible-role: button; accessible-label: "创建顶层频道"; enabled: root.can-create-channels; clicked => { root.channel-editor-mode = 3; root.channel-editor-index = -1; root.channel-editor-name = ""; root.channel-editor-user-limit = "0"; } } }
                        }
                    }
                }
                right-panel := Rectangle { background: EvTheme.canvas; horizontal-stretch: 1;
                    content-layout := VerticalLayout { width: parent.width; height: root.height - root.topbar-height - EvTheme.control-height; padding-left: 20px; padding-right: 15px; padding-top: 6px; padding-bottom: 4px; spacing: 0px;
                        HorizontalLayout { height: 25px; spacing: 8px; Text { text: root.channel-name; font-size: 12px; font-weight: 600; color: #20232c; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } RoundIconButton { width: 25px; height: 25px; icon-name: "star"; icon-size: 14px; icon-color: #a5a9b2; enabled: false; accessible-role: button; accessible-label: "收藏频道功能暂未开放"; } RoundIconButton { width: 25px; height: 25px; icon-name: "users"; icon-size: 14px; accessible-role: button; accessible-label: "显示或隐藏用户列表"; clicked => { root.show-member-list = !root.show-member-list; root.show-main-menu = false; } } main-more-button := RoundIconButton { width: 25px; height: 25px; icon-name: "more"; icon-size: 14px; clicked => { root.show-main-menu = !root.show-main-menu; root.show-member-list = false; } } }
                        Text { height: 17px; text: root.members.length + " 人在线" + (root.channel-name == "未加入频道" ? "" : "  ·  语音频道"); font-size: 9px; color: #7d8390; }
                        Rectangle { height: 1px; background: #ececf0; }
                        activity-area := Rectangle { height: root.height - root.topbar-height - 121px; background: transparent;
                            VerticalLayout { visible: false; padding-top: 12px; spacing: 3px;
                                if (root.members.length == 0) : Rectangle { height: 112px; background: transparent;
                                    VerticalLayout { padding-top: 28px; spacing: 6px;
                                        Rectangle { height: 34px; background: transparent; Rectangle { width: 34px; height: 34px; x: (parent.width - self.width) / 2; border-radius: 17px; background: #f0f2f7; Icon { name: "users"; stroke: #9aa0ad; width: 17px; height: 17px; x: 8px; y: 8px; } } }
                                        Text { text: root.channel-name == "未加入频道" ? "选择一个频道开始语音" : "频道里还没有其他成员"; font-size: 11px; color: #9196a2; horizontal-alignment: center; }
                                    }
                                }
                                for member in root.members : Rectangle { height: 40px; background: member-touch.has-hover ? #fafafe : transparent; border-radius: EvTheme.radius-md;
                                    HorizontalLayout { padding-left: 2px; padding-right: 4px; spacing: 10px;
                                        EvAvatar { avatar: member.avatar; avatar-image: member.avatar-image; custom-avatar: member.custom-avatar; speaking: member.speaking; avatar-size: 18px; }
                                        VerticalLayout { padding-top: 5px; padding-bottom: 5px; spacing: 1px;
                                            HorizontalLayout { height: 16px; spacing: 6px;
                                                Text { text: member.name; font-size: 10px; font-weight: 700; color: #292d37; vertical-alignment: center; }
                                                Text { text: "现在"; font-size: 8px; color: #a0a4ae; vertical-alignment: center; }
                                                if (member.speaking) : Icon { name: "signal"; stroke: #2fa868; width: 12px; height: 12px; }
                                                Rectangle { horizontal-stretch: 1; }
                                            }
                                            Text { text: member.speaking ? "正在说话" : member.locally-muted ? "已在本地静音" : member.muted ? "麦克风已静音" : "已加入语音频道"; font-size: 10px; color: member.speaking ? #279c60 : member.locally-muted ? #d05f65 : #4b505c; }
                                        }
                                        if (member.locally-muted) : Icon { name: "volume-off"; stroke: #d05f65; width: 15px; height: 15px; }
                                        if (member.muted) : Icon { name: "mic-off"; stroke: rgb(153, 158, 170); width: 15px; height: 15px; }
                                    }
                                    member-touch := TouchArea { pointer-event(event) => {
                                        if (event.button == PointerEventButton.right && event.kind == PointerEventKind.down) {
                                            root.context-menu-kind = 2;
                                            root.context-user-session = member.session-id;
                                            root.context-user-channel-index = root.own-channel-index;
                                            root.context-user-name = member.name;
                                            root.context-user-avatar = member.avatar;
                                            root.context-user-avatar-image = member.avatar-image;
                                            root.context-user-custom-avatar = member.custom-avatar;
                                            root.context-user-channel = root.channel-name;
                                            root.context-user-speaking = member.speaking;
                                            root.context-user-muted = member.muted;
                                            root.context-user-admin-muted = member.admin-muted;
                                            root.context-user-server-muted = member.server-muted;
                                            root.context-user-channel-muted = member.channel-muted;
                                            root.context-user-deafened = member.deafened;
                                            root.context-user-locally-muted = member.locally-muted;
                                            root.context-user-volume = member.volume;
                                            root.context-user-server-role = member.server-role;
                                            root.context-user-channel-role = member.channel-role;
                                            root.show-user-card = false;
                                        }
                                    } }
                                }
                            }
                            if (root.visual-demo) : VerticalLayout { padding-top: 8px; spacing: 4px;
                                Rectangle { height: 32px; background: transparent;
                                    HorizontalLayout { spacing: 8px;
                                        EvAvatar { avatar: "L"; avatar-size: 18px; }
                                        VerticalLayout { spacing: 0px;
                                            HorizontalLayout { height: 14px; spacing: 5px; Text { text: "Lumi"; font-size: 9px; font-weight: 600; color: #2d313b; } Text { text: "20:15"; font-size: 8px; color: #a1a6b0; } }
                                            Text { text: "大家晚上好呀～"; font-size: 9px; color: #626875; }
                                        }
                                    }
                                }
                                Rectangle { height: 32px; background: transparent;
                                    HorizontalLayout { spacing: 8px;
                                        EvAvatar { avatar: "K"; avatar-size: 18px; }
                                        VerticalLayout { spacing: 0px;
                                            HorizontalLayout { height: 14px; spacing: 5px; Text { text: "Kira"; font-size: 9px; font-weight: 600; color: #2d313b; } Text { text: "20:15"; font-size: 8px; color: #a1a6b0; } }
                                            Text { text: "今天有什么安排？"; font-size: 9px; color: #626875; }
                                        }
                                    }
                                }
                                Rectangle { height: 32px; background: transparent;
                                    HorizontalLayout { spacing: 8px;
                                        EvAvatar { avatar: "M"; avatar-size: 18px; }
                                        VerticalLayout { spacing: 0px;
                                            HorizontalLayout { height: 14px; spacing: 5px; Text { text: "Moon"; font-size: 9px; font-weight: 600; color: #2d313b; } Text { text: "20:16"; font-size: 8px; color: #a1a6b0; } }
                                            Text { text: "来几把无尽峡谷吧！"; font-size: 9px; color: #626875; }
                                        }
                                    }
                                }
                                Rectangle { height: 30px; background: transparent;
                                    HorizontalLayout { spacing: 8px;
                                        EvAvatar { avatar: "R"; avatar-size: 18px; }
                                        VerticalLayout { spacing: 0px;
                                            HorizontalLayout { height: 14px; spacing: 5px; Text { text: "Ray"; font-size: 9px; font-weight: 600; color: #2d313b; } Text { text: "20:16"; font-size: 8px; color: #a1a6b0; } }
                                            Text { text: "好耶！"; font-size: 9px; color: #626875; }
                                        }
                                    }
                                }
                                Rectangle { height: 30px; background: transparent;
                                    HorizontalLayout { spacing: 8px;
                                        EvAvatar { avatar: "E"; avatar-size: 18px; }
                                        VerticalLayout { spacing: 0px;
                                            HorizontalLayout { height: 14px; spacing: 5px; Text { text: "Echo"; font-size: 9px; font-weight: 600; color: #2d313b; } Text { text: "20:17"; font-size: 8px; color: #a1a6b0; } }
                                            Text { text: "已收到，马上加入。"; font-size: 9px; color: #626875; }
                                        }
                                    }
                                }
                                HorizontalLayout { height: 96px;
                                    Rectangle { width: 30px; }
                                    Rectangle { width: 202px; height: 94px; border-radius: 7px; clip: true;
                                        Image { width: parent.width; height: parent.height; source: @image-url("../assets/ui/demo-landscape-v1.png"); image-fit: cover; }
                                    }
                                    Rectangle { horizontal-stretch: 1; }
                                }
                            }
                            if (!root.visual-demo) : Flickable {
                                width: parent.width; height: parent.height;
                                viewport-height: root.chat-items.length * 47px + 112px;
                                chat-content := VerticalLayout {
                                    width: parent.width; height: root.chat-items.length * 47px + 112px; padding-top: 10px; padding-bottom: 10px; spacing: 5px;
                                    if (root.chat-items.length == 0) : Rectangle {
                                        height: 92px;
                                        Text { text: root.channel-name == "未加入频道" ? "加入频道后即可发送消息" : "还没有消息，来打个招呼吧"; font-size: 10px; color: #969ba6; horizontal-alignment: center; vertical-alignment: center; }
                                    }
                                    for message in root.chat-items : Rectangle {
                                        height: 42px; background: message.own ? #fafafe : transparent; border-radius: 6px;
                                        HorizontalLayout { padding-left: 2px; padding-right: 6px; spacing: 8px;
                                            EvAvatar { avatar: message.avatar; avatar-image: message.avatar-image; custom-avatar: message.custom-avatar; avatar-size: 18px; }
                                            VerticalLayout { padding-top: 3px; padding-bottom: 3px; spacing: 1px;
                                                HorizontalLayout { height: 14px; spacing: 5px;
                                                    Text { text: message.name; font-size: 9px; font-weight: 600; color: #2d313b; }
                                                    Text { text: message.time; font-size: 8px; color: #a1a6b0; }
                                                }
                                                Text { text: message.text; font-size: 9px; color: #5d6370; wrap: word-wrap; overflow: elide; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        message-input := FormLineEdit { height: 30px; text <=> root.chat-draft; placeholder-text: root.channel-name == "未加入频道" ? "加入频道后即可发送消息" : "输入消息，按 Enter 发送"; enabled: root.channel-name != "未加入频道"; accepted => { if (root.chat-draft != "") { root.send-chat(root.chat-draft); root.chat-draft = ""; } } }
                    }
                }
            }
            control-bar := Rectangle { height: EvTheme.control-height; background: EvTheme.sidebar;
                default-control-inner := Rectangle { width: parent.width; height: 32px; y: (parent.height - self.height) / 2; background: transparent;
                HorizontalLayout { padding-left: 7px; padding-right: 14px; spacing: 7px;
                    mic-touch := TouchArea { width: 32px; height: 32px; enabled: !root.mic-admin-muted; accessible-role: button; accessible-label: root.mic-admin-muted ? "已被管理员禁言" : root.mic-muted ? "取消麦克风静音" : "麦克风静音"; clicked => { root.toggle-mic(); } Rectangle { border-radius: 16px; background: root.mic-admin-muted ? #d98a32 : root.mic-muted ? #e66d70 : EvTheme.accent; Icon { name: root.mic-muted ? "mic-off" : "mic"; stroke: #ffffff; x: 8px; y: 8px; width: 16px; height: 16px; } if (root.mic-admin-muted) : Rectangle { x: 20px; y: 20px; width: 11px; height: 11px; border-radius: 5.5px; background: #ffffff; Icon { name: "lock"; stroke: #b36c20; stroke-width: 2px; x: 2px; y: 2px; width: 7px; height: 7px; } } } }
                    Rectangle { width: 2px; }
                    speaker-touch := RoundIconButton { icon-name: root.speaker-muted ? "volume-off" : "volume"; icon-color: root.speaker-muted ? #d56468 : #424754; accessible-role: button; accessible-label: root.speaker-muted ? "取消扬声器静音" : "扬声器静音"; clicked => { root.toggle-speaker(); } }
                    Rectangle { width: 118px; height: 32px; background: transparent;
                        output-picker := Rectangle { x: 2px; y: 1px; width: parent.width - 2px; height: 30px; border-radius: 7px; background: output-picker-touch.pressed ? #dfe2e9 : output-picker-touch.has-hover ? #e7e9ef : #eef0f4;
                            HorizontalLayout { padding-left: 10px; padding-right: 8px; spacing: 6px; Text { text: root.output-device; font-size: 10px; font-weight: 500; color: #454b58; vertical-alignment: center; overflow: elide; } Rectangle { horizontal-stretch: 1; } Icon { name: "chevron"; width: 13px; height: 13px; } }
                            output-picker-touch := TouchArea { clicked => { root.show-output-menu = !root.show-output-menu; root.show-server-menu = false; } }
                        }
                    }
                    RoundIconButton { icon-name: "settings"; clicked => { root.settings-page = 0; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    Rectangle { horizontal-stretch: 1; }
                    Rectangle { width: 62px; HorizontalLayout { spacing: 5px; Rectangle { width: 16px; Icon { name: "signal"; stroke: root.quality-color; width: 16px; height: 16px; y: (parent.height - self.height) / 2; } } Text { text: root.latency; font-size: 9px; color: #4d5361; vertical-alignment: center; } } }
                    Text { width: 68px; text: root.codec-label; font-size: 9px; color: #676d7a; vertical-alignment: center; horizontal-alignment: right; }
                }
                }
            }
        }

        if (root.show-main && root.window-mode == 1) : VerticalLayout {
            width: parent.width; height: parent.height; spacing: 0px;
            Rectangle { height: root.topbar-height; background: #ffffff;
                compact-topbar-inner := Rectangle { width: parent.width; height: 28px; y: (parent.height - self.height) / 2; background: transparent;
                HorizontalLayout { padding-left: root.macos-titlebar-overlay ? 74px : 12px; padding-right: 10px; spacing: 6px;
                    Text { text: "EdenVoice"; font-size: 12px; font-weight: 700; color: #171923; vertical-alignment: center; }
                    Rectangle { width: root.macos-titlebar-overlay ? 60px : 8px; }
                    Rectangle { width: root.topology == "HYBRID" ? 58px : 48px; Rectangle { width: parent.width; height: 22px; y: (parent.height - self.height) / 2; border-radius: 6px; border-width: 1px; border-color: root.topology == "P2P" ? rgb(61, 187, 118) : root.topology == "HYBRID" ? #f09a3e : rgb(101, 112, 237); HorizontalLayout { padding-left: 5px; padding-right: 5px; spacing: 3px; Rectangle { width: 10px; height: 10px; border-radius: 3px; background: root.topology == "P2P" ? #35ad68 : root.topology == "HYBRID" ? #f09a3e : #5967e8; Icon { name: root.topology == "P2P" ? "p2p" : root.topology == "HYBRID" ? "hybrid" : "sfu"; stroke: #ffffff; stroke-width: 2px; width: 6px; height: 6px; x: 2px; y: 2px; } } Text { text: root.topology; font-size: 9px; font-weight: 700; color: root.topology == "P2P" ? rgb(38, 148, 84) : root.topology == "HYBRID" ? #d87a16 : rgb(76, 92, 222); vertical-alignment: center; } } } }
                    Rectangle { width: 60px; HorizontalLayout { spacing: 5px; Rectangle { width: 15px; Icon { name: "signal"; stroke: root.quality-color; width: 15px; height: 15px; y: (parent.height - self.height) / 2; } } Text { text: root.latency; font-size: 10px; color: #555b68; vertical-alignment: center; } } }
                    Rectangle { horizontal-stretch: 1; }
                    RoundIconButton { width: 28px; height: 28px; icon-name: "expand"; clicked => { root.window-mode = 0; } }
                }
                }
                Rectangle { y: parent.height - 1px; width: parent.width; height: 1px; background: EvTheme.divider; }
            }
            compact-body := HorizontalLayout { height: root.height - root.topbar-height - EvTheme.control-height; spacing: 0px;
                compact-left-panel := Rectangle { min-width: 138px; max-width: 138px; background: EvTheme.sidebar; border-width: 1px; border-color: EvTheme.divider;
                    VerticalLayout { padding-left: 8px; padding-right: 8px; padding-top: 10px; spacing: 4px;
                        for channel in root.compact-channels : Rectangle { height: channel.selected ? 154px : 32px; background: transparent;
                            compact-channel-row := Rectangle { y: 0px; height: 32px; border-radius: 6px; background: channel.selected || (channel.channel-index >= 0 && (root.pending-channel-index == channel.channel-index || root.selected-channel-index == channel.channel-index)) ? #e7eaff : compact-channel-touch.has-hover ? #f1f2f6 : transparent;
                                HorizontalLayout { padding-left: 8px + channel.depth * 6px; padding-right: 7px; spacing: 5px;
                                    if (channel.depth > 0) : compact-channel-branch := Rectangle { width: 5px; height: parent.height; background: transparent;
                                        Rectangle { x: 0px; y: 0px; width: 1px; height: parent.height / 2; background: #c9cdd7; }
                                        Rectangle { x: 0px; y: parent.height / 2; width: 5px; height: 1px; background: #c9cdd7; }
                                    }
                                    Icon { name: "chevron"; stroke: #747a88; width: 13px; height: 13px; }
                                    Text { text: channel.label; font-size: 10px; font-weight: channel.depth == 0 ? 650 : 550; color: channel.channel-index < 0 ? #8f95a1 : channel.selected ? #4355d2 : channel.depth > 0 ? #555b68 : #303540; vertical-alignment: center; overflow: elide; }
                                    Rectangle { horizontal-stretch: 1; }
                                    if (channel.count != "" && channel.count != "0") : Rectangle { width: 18px; height: 18px; border-radius: 9px; background: channel.selected ? #ccd3ff : #eff0f3; Text { text: channel.count; font-size: 9px; color: #6170d4; horizontal-alignment: center; vertical-alignment: center; } }
                                }
                                compact-channel-touch := TouchArea { enabled: channel.channel-index >= 0; clicked => { root.selected-channel-index = channel.channel-index; } double-clicked => { root.selected-channel-index = channel.channel-index; root.pending-channel-index = channel.channel-index; root.join-channel(channel.channel-index); } }
                            }
                            if (channel.selected) : VerticalLayout { x: 20px + channel.depth * 6px; y: 35px; width: parent.width - 27px - channel.depth * 6px; height: parent.height - 37px; spacing: 0px;
                                for member in root.members : Rectangle { height: 23px; background: transparent;
                                    HorizontalLayout { spacing: 7px;
                                        EvAvatar { avatar: member.avatar; avatar-image: member.avatar-image; custom-avatar: member.custom-avatar; speaking: member.speaking; avatar-size: 14px; }
                                        Text { text: member.name; font-size: 9px; font-weight: 500; color: #444a56; vertical-alignment: center; overflow: elide; }
                                        Rectangle { horizontal-stretch: 1; }
                                        if (member.speaking) : Icon { name: "signal"; stroke: #2faa68; width: 12px; height: 12px; }
                                        if (member.muted || member.locally-muted) : Icon { name: "mic-off"; stroke: #9399a5; width: 12px; height: 12px; }
                                    }
                                }
                            }
                        }
                        Rectangle { vertical-stretch: 1; }
                        Rectangle { width: 28px; height: 28px; border-radius: 6px; background: #f1f2f5; Icon { name: "plus"; stroke: #8f95a1; width: 13px; height: 13px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2; } }
                    }
                }
                Rectangle { horizontal-stretch: 1; background: #ffffff;
                    VerticalLayout { padding-left: 13px; padding-right: 16px; padding-top: 10px; spacing: 0px;
                        HorizontalLayout { height: 35px; spacing: 6px; Text { text: root.channel-name; font-size: 12px; font-weight: 600; color: #20232c; vertical-alignment: center; } Text { text: "· " + root.members.length + " 人在线"; font-size: 9px; color: #7c818e; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } }
                        Rectangle { height: 1px; background: #ececf0; }
                        VerticalLayout { padding-top: 9px; spacing: 1px;
                            for member in root.members : Rectangle { height: 30px; background: transparent;
                                HorizontalLayout { spacing: 9px;
                                    EvAvatar { avatar: member.avatar; avatar-image: member.avatar-image; custom-avatar: member.custom-avatar; speaking: member.speaking; avatar-size: 16px; }
                                    Text { text: member.name; font-size: 9px; font-weight: 500; color: #3d424d; vertical-alignment: center; }
                                    Rectangle { horizontal-stretch: 1; }
                                    if (member.speaking) : Icon { name: "signal"; stroke: #2fa868; width: 14px; height: 14px; }
                                    if (member.locally-muted) : Icon { name: "volume-off"; stroke: #d05f65; width: 14px; height: 14px; }
                                    Icon { name: member.muted ? "mic-off" : "mic"; stroke: member.muted ? #a0a5b0 : #59606d; width: 14px; height: 14px; }
                                }
                            }
                        }
                    }
                }
            }
            compact-control-bar := Rectangle { height: EvTheme.control-height; background: EvTheme.sidebar;
                compact-control-inner := Rectangle { width: parent.width; height: 32px; y: (parent.height - self.height) / 2; background: transparent;
                HorizontalLayout { padding-left: 12px; padding-right: 12px; spacing: 7px;
                    TouchArea { width: 36px; height: 36px; enabled: !root.mic-admin-muted; accessible-role: button; accessible-label: root.mic-admin-muted ? "已被管理员禁言" : root.mic-muted ? "取消麦克风静音" : "麦克风静音"; clicked => { root.toggle-mic(); } Rectangle { border-radius: 18px; background: root.mic-admin-muted ? #d98a32 : root.mic-muted ? #e66d70 : #5261e6; Icon { name: root.mic-muted ? "mic-off" : "mic"; stroke: #ffffff; x: 9px; y: 9px; width: 18px; height: 18px; } if (root.mic-admin-muted) : Rectangle { x: 23px; y: 23px; width: 11px; height: 11px; border-radius: 5.5px; background: #ffffff; Icon { name: "lock"; stroke: #b36c20; stroke-width: 2px; x: 2px; y: 2px; width: 7px; height: 7px; } } } }
                    Rectangle { width: 26px; }
                    RoundIconButton { icon-name: root.speaker-muted ? "volume-off" : "volume"; icon-color: root.speaker-muted ? #d56468 : #424754; clicked => { root.toggle-speaker(); } }
                    Rectangle { width: 8px; }
                    RoundIconButton { icon-name: "settings"; clicked => { root.settings-page = 0; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    Rectangle { horizontal-stretch: 1; }
                    Rectangle { width: 62px; HorizontalLayout { spacing: 5px; Rectangle { width: 16px; Icon { name: "signal"; stroke: root.quality-color; width: 16px; height: 16px; y: (parent.height - self.height) / 2; } } Text { text: root.latency; font-size: 10px; color: #4d5361; vertical-alignment: center; } } }
                    Text { width: 68px; text: root.codec-label; font-size: 9px; color: #676d7a; vertical-alignment: center; horizontal-alignment: right; }
                }
                }
            }
        }

        if (root.show-output-menu && root.show-main && root.window-mode == 0 && !root.show-settings) : Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-output-menu = false; } }
            Rectangle { width: 190px; height: max(44px, min(230px, 12px + root.output-devices.length * 34px)); x: 184px; y: parent.height - 45px - self.height - 5px; background: #ffffff; border-radius: 8px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f243018; drop-shadow-blur: 14px; drop-shadow-offset-y: 4px;
                VerticalLayout { padding: 6px; spacing: 1px;
                    for device[index] in root.output-devices : Rectangle { height: 33px; border-radius: 6px; background: device-touch.pressed ? #e7e9f2 : device-touch.has-hover ? #f1f2f6 : index == root.output-device-index ? #e9ecff : transparent;
                        device-touch := TouchArea { clicked => { root.output-device-index = index; root.select-output-device(index); root.show-output-menu = false; } }
                        Text { x: 9px; width: parent.width - 18px; text: device; font-size: 11px; font-weight: index == root.output-device-index ? 600 : 400; color: index == root.output-device-index ? #4e5bd5 : #3f4551; vertical-alignment: center; overflow: elide; }
                    }
                }
            }
        }

        if (root.show-server-menu && root.show-main && !root.show-settings && !root.show-network-details) : Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-server-menu = false; } }
            Rectangle {
                width: root.window-mode == 1 ? 146px : 176px;
                height: max(52px, min(268px, 14px + root.saved-servers.length * 46px));
                x: root.window-mode == 1 ? 8px : 10px; y: root.topbar-height + 48px;
                background: #ffffff; border-radius: 8px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f243018; drop-shadow-blur: 14px; drop-shadow-offset-y: 4px;
                VerticalLayout { padding: 7px; spacing: 2px;
                    for server[index] in root.saved-servers : Rectangle { height: 44px; border-radius: 6px; background: saved-server-touch.pressed ? #e8eaf3 : saved-server-touch.has-hover ? #f1f2f6 : server.address == root.server-address ? #eef0ff : transparent;
                        saved-server-touch := TouchArea { clicked => { root.show-server-menu = false; if (server.address != root.server-address) { root.switch-saved-server(index); } } }
                        HorizontalLayout { padding-left: 8px; padding-right: 7px; spacing: 7px;
                            Rectangle { width: 23px; height: 23px; border-radius: 6px; background: #e4e7ff; Text { text: server.avatar; font-size: 9px; font-weight: 700; color: #5660df; horizontal-alignment: center; vertical-alignment: center; } }
                            VerticalLayout { padding-top: 7px; padding-bottom: 6px; spacing: 0px; Text { text: server.name; font-size: 11px; font-weight: 600; color: #303540; overflow: elide; } Text { text: server.address; font-size: 9px; color: #8a909c; overflow: elide; } }
                        }
                    }
                }
            }
        }

        if (root.show-main-menu && root.show-main && !root.show-settings && !root.show-network-details) : Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-main-menu = false; } }
            Rectangle {
                width: 158px; height: root.server-role == "MASTER" ? 272px : 298px; x: parent.width - self.width - 14px; y: root.topbar-height + 38px;
                background: #ffffff; border-radius: 8px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f243018; drop-shadow-blur: 14px; drop-shadow-offset-y: 4px;
                VerticalLayout { padding: 7px; spacing: 2px;
                    compact-mode-menu-action := MenuAction { label: "切换到紧凑模式"; selected => { root.show-main-menu = false; root.window-mode = 1; } }
                    network-details-menu-action := MenuAction { label: "网络详情"; selected => { root.show-main-menu = false; root.network-details-tab = 0; root.show-network-details = true; } }
                    shortcuts-menu-action := MenuAction { label: "快捷键"; selected => { root.show-main-menu = false; root.settings-page = 2; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    MenuAction { label: "设置"; selected => { root.show-main-menu = false; root.settings-page = 0; root.room-network-channel-index = -1; root.room-network-channel-name = ""; root.open-settings(); } }
                    top-level-channel-menu-action := MenuAction { label: "创建顶层频道"; enabled: root.can-create-channels; selected => { root.show-main-menu = false; root.channel-editor-mode = 3; root.channel-editor-index = -1; root.channel-editor-name = ""; root.channel-editor-user-limit = "0"; } }
                    ban-manager-menu-action := MenuAction { label: "封禁管理"; enabled: root.can-unban-users; selected => { root.show-main-menu = false; root.ban-page = 0; root.show-ban-manager = true; root.request-bans(0); } }
                    if (root.server-role != "MASTER") : MenuAction { label: "认领 MASTER"; selected => { root.show-main-menu = false; root.master-provision-token = ""; root.show-master-provision = true; } }
                    Rectangle { height: 1px; background: #eceef2; }
                    MenuAction { label: "断开连接"; danger: true; selected => { root.show-main-menu = false; root.disconnect(); } }
                }
            }
        }

        if (root.show-member-list && root.show-main && root.window-mode == 0 && !root.show-settings && !root.show-network-details) : member-list-layer := Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-member-list = false; } }
            member-list-card := Rectangle {
                width: 200px; height: min(280px, 48px + root.members.length * 38px); x: parent.width - self.width - 14px; y: root.topbar-height + 36px;
                background: #ffffff; border-radius: 9px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f24301f; drop-shadow-blur: 16px; drop-shadow-offset-y: 5px;
                VerticalLayout { padding: 9px; spacing: 1px;
                    HorizontalLayout { height: 28px; Text { text: "频道成员"; font-size: 12px; font-weight: 700; color: #2c303a; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } Text { text: root.members.length + " 人"; font-size: 9px; color: #9297a2; vertical-alignment: center; } }
                    Rectangle { height: 1px; background: #eceef2; }
                    for member in root.members : Rectangle { height: 36px; border-radius: 6px; background: member-row-touch.has-hover ? #f6f7fa : transparent;
                        HorizontalLayout { padding-left: 3px; padding-right: 3px; spacing: 8px;
                            EvAvatar { avatar: member.avatar; avatar-image: member.avatar-image; custom-avatar: member.custom-avatar; speaking: member.speaking; avatar-size: 18px; }
                            Text { text: member.name; font-size: 10px; font-weight: 600; color: #303540; vertical-alignment: center; }
                            Rectangle { horizontal-stretch: 1; }
                            if (member.speaking) : Icon { name: "signal"; stroke: #2fa868; width: 13px; height: 13px; }
                            if (member.locally-muted) : Icon { name: "volume-off"; stroke: #d05f65; width: 13px; height: 13px; }
                            Icon { name: member.muted ? "mic-off" : "mic"; stroke: member.muted ? #a0a5b0 : #59606d; width: 13px; height: 13px; }
                        }
                        member-row-touch := TouchArea { }
                    }
                }
            }
        }

        // Secondary information layer from the reference board. It remains
        // opt-in so the voice-first main view retains its compact width.
        if (root.show-info-panel && (root.show-main || root.info-panel-only) && root.window-mode == 0 && !root.show-settings) : right-info-panel := Rectangle {
            width: root.info-panel-only ? parent.width : 210px; height: root.info-panel-only ? parent.height : root.height - root.topbar-height - 18px;
            x: root.info-panel-only ? 0px : parent.width - self.width - 10px; y: root.info-panel-only ? 0px : root.topbar-height + 8px;
            background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #e8eaf0;
            drop-shadow-color: #1f243010; drop-shadow-blur: 12px; drop-shadow-offset-y: 3px;
            VerticalLayout { padding: 12px; spacing: 8px;
                Text { height: 18px; text: "频道信息"; font-size: 11px; font-weight: 600; color: #3a3f4a; }
                Rectangle { height: 50px; border-radius: 7px; border-width: 1px; border-color: #eceef3; background: #ffffff;
                    HorizontalLayout { padding-left: 9px; padding-right: 9px; spacing: 8px;
                        Rectangle { width: 22px; height: 22px; border-radius: 6px; background: #e8eaff; Icon { name: "signal"; stroke: #5967e8; width: 13px; height: 13px; x: 5px; y: 5px; } }
                        VerticalLayout { padding-top: 7px; spacing: 1px; Text { text: root.channel-name; font-size: 10px; font-weight: 600; color: #313640; } Text { text: root.topology == "P2P" ? "P2P 模式 · " + root.members.length + " 人在线" : "SFU 模式 · " + root.members.length + " 人在线"; font-size: 8px; color: #828895; } }
                    }
                }
                Text { height: 17px; text: "网络质量"; font-size: 11px; font-weight: 600; color: #3a3f4a; }
                Rectangle { height: 104px; border-radius: 7px; border-width: 1px; border-color: #eceef3; background: #ffffff;
                    VerticalLayout { padding: 9px; spacing: 3px;
                        DetailRow { height: 17px; label: "延迟"; value: root.latency; }
                        DetailRow { height: 17px; label: "丢包率"; value: root.packet-loss-label; }
                        DetailRow { height: 17px; label: "抖动"; value: root.jitter-label; }
                        HorizontalLayout { height: 17px; Text { text: "带宽"; font-size: 9px; color: #737986; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } Icon { name: "signal"; stroke: #6f7683; width: 11px; height: 11px; } Text { text: root.uplink-label; font-size: 9px; color: #4b505c; vertical-alignment: center; } }
                    }
                }
                Text { height: 17px; text: "用户列表"; font-size: 11px; font-weight: 600; color: #3a3f4a; }
                HorizontalLayout { height: 28px; spacing: 4px;
                    for member in root.compact-avatar-members : EvAvatar { avatar: member.avatar; avatar-image: member.avatar-image; custom-avatar: member.custom-avatar; speaking: member.speaking; avatar-size: 20px; }
                    Rectangle { width: 27px; height: 25px; border-radius: 6px; border-width: 1px; border-color: #eceef3; Text { text: "+" + root.compact-member-overflow; font-size: 9px; color: #4f5562; horizontal-alignment: center; vertical-alignment: center; } }
                }
                Text { height: 17px; text: "快捷操作"; font-size: 11px; font-weight: 600; color: #3a3f4a; }
                VerticalLayout { spacing: 2px;
                    MenuAction { height: 24px; label: "频道设置"; enabled: false; }
                    MenuAction { height: 24px; label: "邀请好友"; enabled: false; }
                    MenuAction { height: 24px; label: "复制频道 ID"; enabled: false; }
                    Rectangle { height: 1px; background: #eceef2; }
                    MenuAction { height: 24px; label: "断开连接"; danger: true; selected => { root.disconnect(); } }
                }
            }
        }

        if (root.show-avatar-picker && root.show-main) : Rectangle {
            width: parent.width; height: parent.height; background: #11131866;
            TouchArea { clicked => { root.show-avatar-picker = false; } }
            Rectangle {
                width: 330px; height: 215px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 11px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 18px; spacing: 12px;
                    HorizontalLayout { height: 26px; Text { text: "选择头像"; font-size: 15px; font-weight: 700; color: #242832; } Rectangle { horizontal-stretch: 1; } RoundIconButton { width: 25px; height: 25px; icon-name: "close"; clicked => { root.show-avatar-picker = false; } } }
                    Text { height: 18px; text: "头像会绑定当前身份，并同步给服务器中的其他用户。"; font-size: 9px; color: #777d89; }
                    HorizontalLayout { height: 48px; spacing: 12px;
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar != "L" && root.self-avatar != "K" && root.self-avatar != "M" && root.self-avatar != "R" && root.self-avatar != "E" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: root.self-initials; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar(""); root.show-avatar-picker = false; } } }
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar == "L" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: "L"; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar("L"); root.show-avatar-picker = false; } } }
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar == "K" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: "K"; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar("K"); root.show-avatar-picker = false; } } }
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar == "M" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: "M"; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar("M"); root.show-avatar-picker = false; } } }
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar == "R" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: "R"; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar("R"); root.show-avatar-picker = false; } } }
                        Rectangle { width: 40px; height: 40px; border-radius: 20px; border-width: root.self-avatar == "E" ? 2px : 0px; border-color: #5967e8; EvAvatar { avatar: "E"; avatar-size: 34px; x: 3px; y: 3px; } TouchArea { clicked => { root.set-avatar("E"); root.show-avatar-picker = false; } } }
                    }
                    FormButton { height: 32px; text: "从本地图片上传…"; primary: true; clicked => { root.choose-avatar-image(); } }
                }
            }
        }

        if (root.show-avatar-crop && root.show-main) : Rectangle {
            width: parent.width; height: parent.height; background: #11131888;
            Rectangle {
                width: 350px; height: 390px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 11px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 18px; spacing: 10px;
                    HorizontalLayout { height: 26px; Text { text: "裁剪头像"; font-size: 15px; font-weight: 700; color: #242832; } Rectangle { horizontal-stretch: 1; } RoundIconButton { width: 25px; height: 25px; icon-name: "close"; clicked => { root.show-avatar-crop = false; } } }
                    Text { height: 28px; text: "拖动图片调整位置，使用滑杆放大；圆形预览线内是最终头像范围。"; wrap: word-wrap; font-size: 9px; color: #777d89; }
                    HorizontalLayout { height: 200px; Rectangle { horizontal-stretch: 1; } Rectangle { width: 200px; height: 200px; border-radius: 100px; clip: true; background: #eef0f5; border-width: 2px; border-color: #5967e8;
                        crop-image := Image {
                            source: root.avatar-crop-image; image-fit: fill;
                            width: root.avatar-crop-width >= root.avatar-crop-height ? 200px * root.avatar-crop-width / root.avatar-crop-height * root.avatar-crop-zoom : 200px * root.avatar-crop-zoom;
                            height: root.avatar-crop-height >= root.avatar-crop-width ? 200px * root.avatar-crop-height / root.avatar-crop-width * root.avatar-crop-zoom : 200px * root.avatar-crop-zoom;
                            x: (200px - self.width) / 2 + root.avatar-crop-offset-x * 1px;
                            y: (200px - self.height) / 2 + root.avatar-crop-offset-y * 1px;
                        }
                        Rectangle { width: parent.width; height: parent.height; border-radius: 100px; border-width: 2px; border-color: #5967e8; background: transparent; }
                        crop-touch := TouchArea { pointer-event(event) => {
                            if (event.kind == PointerEventKind.down && event.button == PointerEventButton.left) {
                                root.avatar-crop-dragging = true;
                                root.avatar-crop-last-x = self.mouse-x / 1px;
                                root.avatar-crop-last-y = self.mouse-y / 1px;
                            } else if (event.kind == PointerEventKind.move && root.avatar-crop-dragging) {
                                let next-x = root.avatar-crop-offset-x + self.mouse-x / 1px - root.avatar-crop-last-x;
                                let next-y = root.avatar-crop-offset-y + self.mouse-y / 1px - root.avatar-crop-last-y;
                                let max-x = (crop-image.width - 200px) / 2 / 1px;
                                let max-y = (crop-image.height - 200px) / 2 / 1px;
                                root.avatar-crop-offset-x = next-x > max-x ? max-x : next-x < -max-x ? -max-x : next-x;
                                root.avatar-crop-offset-y = next-y > max-y ? max-y : next-y < -max-y ? -max-y : next-y;
                                root.avatar-crop-last-x = self.mouse-x / 1px;
                                root.avatar-crop-last-y = self.mouse-y / 1px;
                            } else if (event.kind == PointerEventKind.up) {
                                root.avatar-crop-dragging = false;
                            }
                        } }
                    } Rectangle { horizontal-stretch: 1; } }
                    HorizontalLayout { height: 26px; spacing: 8px; Text { width: 36px; text: "缩放"; font-size: 10px; color: #555b68; vertical-alignment: center; } Slider { horizontal-stretch: 1; minimum: 1; maximum: 4; step: 0.05; value <=> root.avatar-crop-zoom; } Text { width: 34px; text: round(root.avatar-crop-zoom * 100) + "%"; font-size: 9px; vertical-alignment: center; horizontal-alignment: right; } }
                    HorizontalLayout { height: 32px; spacing: 8px; Rectangle { horizontal-stretch: 1; } FormButton { width: 78px; text: "取消"; clicked => { root.show-avatar-crop = false; } } FormButton { width: 94px; text: "保存头像"; primary: true; clicked => { root.save-avatar-crop(root.avatar-crop-zoom, root.avatar-crop-offset-x, root.avatar-crop-offset-y); } } }
                }
            }
        }

        if (root.channel-editor-mode != 0) : channel-editor-layer := Rectangle {
            width: parent.width; height: parent.height; background: #11131866;
            Rectangle {
                width: 360px; height: 214px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 18px; spacing: 12px;
                    Text { text: root.channel-editor-mode == 1 ? "创建子频道" : root.channel-editor-mode == 3 ? "创建顶层频道" : "编辑频道"; font-size: 15px; font-weight: 700; color: #242832; }
                    FormLineEdit { text <=> root.channel-editor-name; placeholder-text: "频道名称"; }
                    FormLineEdit { text <=> root.channel-editor-user-limit; placeholder-text: "人数上限（0 表示不限制）"; }
                    HorizontalLayout { spacing: 8px;
                        Rectangle { horizontal-stretch: 1; height: 32px; border-radius: 6px; background: #eef0f4; Text { text: "取消"; horizontal-alignment: center; vertical-alignment: center; color: #4d5360; } TouchArea { clicked => { root.channel-editor-mode = 0; } } }
                        Rectangle { horizontal-stretch: 1; height: 32px; border-radius: 6px; background: root.channel-editor-name == "" ? #aeb4c5 : #5261e6; Text { text: "保存"; horizontal-alignment: center; vertical-alignment: center; color: #ffffff; } TouchArea { enabled: root.channel-editor-name != ""; clicked => { root.save-channel-editor(root.channel-editor-mode, root.channel-editor-index, root.channel-editor-name, root.channel-editor-user-limit); root.channel-editor-mode = 0; } } }
                    }
                }
            }
        }

        if (root.show-ban-manager) : ban-manager-layer := Rectangle {
            width: parent.width; height: parent.height; background: #11131866;
            ban-manager-card := Rectangle {
                width: 420px; height: 378px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 14px; spacing: 7px;
                    HorizontalLayout { height: 25px;
                        Text { text: "封禁管理"; font-size: 15px; font-weight: 700; color: #242832; vertical-alignment: center; }
                        Rectangle { horizontal-stretch: 1; }
                        Text { text: root.ban-total + " 个身份"; font-size: 10px; color: #858b97; vertical-alignment: center; }
                    }
                    Rectangle { height: 1px; background: #eceef2; }
                    if (root.bans.length == 0) : Rectangle { height: 272px; Text { text: "当前没有已封禁身份"; font-size: 11px; color: #9297a2; horizontal-alignment: center; vertical-alignment: center; } }
                    if (root.bans.length > 0) : VerticalLayout { height: 272px; spacing: 2px;
                        for ban in root.bans : Rectangle { height: 32px; border-radius: 6px; background: #f8f9fb;
                            HorizontalLayout { padding-left: 8px; padding-right: 6px; spacing: 7px;
                                VerticalLayout { horizontal-stretch: 1; padding-top: 3px; padding-bottom: 3px; spacing: 0px;
                                    Text { text: ban.display-name == "" ? ban.client-id : ban.display-name; font-size: 10px; font-weight: 600; color: #343944; overflow: elide; }
                                    Text { text: ban.reason == "" ? ban.client-id : ban.reason + " · " + ban.client-id; font-size: 8px; color: #858b97; overflow: elide; }
                                }
                                Rectangle { width: 46px; height: 24px; border-radius: 6px; background: ban-unban-touch.pressed ? #e8eaf0 : ban-unban-touch.has-hover ? #f0f1f5 : #ffffff; border-width: 1px; border-color: #dfe2e9;
                                    Text { text: "解除"; font-size: 9px; color: #5261df; horizontal-alignment: center; vertical-alignment: center; }
                                    ban-unban-touch := TouchArea { clicked => { root.unban-client(ban.client-id, root.ban-page); } }
                                }
                            }
                        }
                    }
                    HorizontalLayout { height: 30px; spacing: 7px;
                        Rectangle { width: 62px; border-radius: 6px; background: root.ban-page <= 0 ? #f4f5f7 : #eef0f4; Text { text: "上一页"; font-size: 9px; color: root.ban-page <= 0 ? #adb1ba : #4d5360; horizontal-alignment: center; vertical-alignment: center; } TouchArea { enabled: root.ban-page > 0; clicked => { root.ban-page -= 1; root.request-bans(root.ban-page); } } }
                        Text { horizontal-stretch: 1; text: "第 " + (root.ban-page + 1) + " 页"; font-size: 9px; color: #737986; horizontal-alignment: center; vertical-alignment: center; }
                        Rectangle { width: 62px; border-radius: 6px; background: (root.ban-page + 1) * 8 >= root.ban-total ? #f4f5f7 : #eef0f4; Text { text: "下一页"; font-size: 9px; color: (root.ban-page + 1) * 8 >= root.ban-total ? #adb1ba : #4d5360; horizontal-alignment: center; vertical-alignment: center; } TouchArea { enabled: (root.ban-page + 1) * 8 < root.ban-total; clicked => { root.ban-page += 1; root.request-bans(root.ban-page); } } }
                        Rectangle { width: 62px; border-radius: 6px; background: #5261e6; Text { text: "关闭"; font-size: 9px; color: #ffffff; horizontal-alignment: center; vertical-alignment: center; } TouchArea { clicked => { root.show-ban-manager = false; } } }
                    }
                }
            }
        }

        if (root.trust-fingerprint != "") : trust-layer := Rectangle {
            width: parent.width; height: parent.height; background: #11131888;
            trust-dialog := Rectangle {
                width: 420px; height: 190px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 20px; spacing: 12px;
                    Text { text: "首次连接服务器"; font-size: 16px; font-weight: 700; color: #242832; }
                    Text { text: "请确认服务器身份指纹后再继续："; font-size: 11px; color: #646b78; }
                    Rectangle { height: 48px; border-radius: 6px; background: #f3f4f7; Text { x: 10px; width: parent.width - 20px; height: parent.height; text: root.trust-fingerprint; font-size: 10px; color: #303642; wrap: word-wrap; vertical-alignment: center; } }
                    HorizontalLayout { height: 34px; spacing: 8px;
                        Rectangle { horizontal-stretch: 1; border-radius: 7px; background: trust-cancel-touch.pressed ? #e8eaf0 : trust-cancel-touch.has-hover ? #f0f1f5 : #ffffff; border-width: 1px; border-color: #d9dde6;
                            Text { text: "取消连接"; color: #515866; font-size: 12px; horizontal-alignment: center; vertical-alignment: center; }
                            trust-cancel-touch := TouchArea { clicked => { root.disconnect(); } }
                        }
                        Rectangle { horizontal-stretch: 1; border-radius: 7px; background: trust-confirm-touch.pressed ? #4553d4 : trust-confirm-touch.has-hover ? #6471e9 : #5261e6;
                            Text { text: "信任并继续"; color: #ffffff; font-size: 12px; font-weight: 700; horizontal-alignment: center; vertical-alignment: center; }
                            trust-confirm-touch := TouchArea { clicked => { root.trust-server(root.trust-endpoint, root.trust-fingerprint); } }
                        }
                    }
                }
            }
        }

        if (root.show-notifications && root.show-main && !root.show-settings) : notification-stack := Rectangle {
            width: 246px; height: 174px; x: parent.width - self.width - 14px; y: root.topbar-height + 16px; background: transparent;
            VerticalLayout { spacing: 8px;
                Rectangle { height: 36px; border-radius: 7px; background: #ffffff; border-width: 1px; border-color: #d9eee1; drop-shadow-color: #1f243010; drop-shadow-blur: 8px; drop-shadow-offset-y: 2px;
                    HorizontalLayout { padding-left: 10px; padding-right: 10px; spacing: 8px; Rectangle { width: 15px; height: 15px; border-radius: 8px; background: #35ad68; Icon { name: "check"; stroke: #ffffff; width: 9px; height: 9px; x: 3px; y: 3px; } } Text { text: "已连接到 voice.example.com"; font-size: 9px; color: #3d5445; vertical-alignment: center; } }
                }
                Rectangle { height: 36px; border-radius: 7px; background: #ffffff; border-width: 1px; border-color: #f2e3c7; drop-shadow-color: #1f243010; drop-shadow-blur: 8px; drop-shadow-offset-y: 2px;
                    HorizontalLayout { padding-left: 10px; padding-right: 10px; spacing: 8px; Rectangle { width: 15px; height: 15px; border-radius: 8px; background: #f0a13d; Text { text: "!"; font-size: 10px; font-weight: 700; color: #ffffff; horizontal-alignment: center; vertical-alignment: center; } } Text { text: "网络延迟较高，请检查网络连接"; font-size: 9px; color: #66533b; vertical-alignment: center; } }
                }
                Rectangle { height: 36px; border-radius: 7px; background: #ffffff; border-width: 1px; border-color: #f2d7d9; drop-shadow-color: #1f243010; drop-shadow-blur: 8px; drop-shadow-offset-y: 2px;
                    HorizontalLayout { padding-left: 10px; padding-right: 10px; spacing: 8px; Rectangle { width: 15px; height: 15px; border-radius: 8px; background: #df5961; Icon { name: "close"; stroke: #ffffff; width: 9px; height: 9px; x: 3px; y: 3px; } } Text { text: "连接失败，请重试"; font-size: 9px; color: #69464a; vertical-alignment: center; } }
                }
                Rectangle { height: 36px; border-radius: 7px; background: #ffffff; border-width: 1px; border-color: #d9e4fb; drop-shadow-color: #1f243010; drop-shadow-blur: 8px; drop-shadow-offset-y: 2px;
                    HorizontalLayout { padding-left: 10px; padding-right: 10px; spacing: 8px; Rectangle { width: 15px; height: 15px; border-radius: 8px; background: #5d72e8; Icon { name: "p2p"; stroke: #ffffff; width: 9px; height: 9px; x: 3px; y: 3px; } } Text { text: "正在切换到 P2P 模式…"; font-size: 9px; color: #455379; vertical-alignment: center; } }
                }
            }
        }

        if (root.show-main && root.connection-error != "") : operation-error-toast := Rectangle {
            width: min(320px, parent.width - 28px); height: 44px;
            x: (parent.width - self.width) / 2; y: root.topbar-height + 12px;
            background: #fff5f5; border-radius: 8px; border-width: 1px; border-color: #efcfd2;
            drop-shadow-color: #1f243018; drop-shadow-blur: 10px; drop-shadow-offset-y: 3px;
            HorizontalLayout { padding-left: 11px; padding-right: 11px; spacing: 8px;
                Rectangle { width: 16px; height: 16px; border-radius: 8px; background: #df5961;
                    y: (parent.height - self.height) / 2;
                    Text { text: "!"; font-size: 10px; font-weight: 700; color: #ffffff; horizontal-alignment: center; vertical-alignment: center; }
                }
                Text { horizontal-stretch: 1; text: root.connection-error; font-size: 10px; color: #69464a; wrap: word-wrap; vertical-alignment: center; }
            }
        }

        if (root.context-menu-kind == 1 && root.show-main && root.window-mode == 0 && !root.show-settings && !root.show-network-details) : channel-context-layer := Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.context-menu-kind = 0; } }
            channel-context-menu := Rectangle {
                width: 132px; height: 191px; x: 82px; y: min(parent.height - self.height - 12px, root.topbar-height + 94px);
                background: #ffffff; border-radius: 8px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f243018; drop-shadow-blur: 14px; drop-shadow-offset-y: 4px;
                VerticalLayout { padding: 7px; spacing: 1px;
                    channel-context-join := MenuAction { height: 24px; label: "加入频道"; selected => { root.pending-channel-index = root.context-channel-index; root.join-channel(root.context-channel-index); root.context-menu-kind = 0; } }
                    MenuAction { height: 24px; label: "创建子频道"; enabled: root.can-create-channels && root.can-administer-context-channel; selected => { root.channel-editor-mode = 1; root.channel-editor-index = root.context-channel-index; root.channel-editor-name = ""; root.channel-editor-user-limit = "0"; root.context-menu-kind = 0; } }
                    channel-context-edit := MenuAction { height: 24px; label: "编辑频道"; enabled: root.can-edit-channels && root.can-administer-context-channel; selected => { root.channel-editor-mode = 2; root.channel-editor-index = root.context-channel-index; root.channel-editor-name = root.context-channel-name; root.channel-editor-user-limit = root.context-channel-user-limit; root.context-menu-kind = 0; } }
                    channel-context-network-policy := MenuAction { height: 24px; label: "设置网络模式"; enabled: root.can-set-context-network-policy; selected => { root.room-network-channel-index = root.context-channel-index; root.room-network-channel-name = root.context-channel-name; root.settings-page = 1; root.open-settings(); root.context-menu-kind = 0; } }
                    MenuAction { height: 24px; label: "复制频道 ID"; enabled: false; }
                    Rectangle { height: 1px; background: #eceef2; }
                    channel-context-mute := MenuAction { height: 24px; label: "静音频道"; selected => { root.mute-channel(root.context-channel-index); root.context-menu-kind = 0; } }
                    Rectangle { height: 1px; background: #eceef2; }
                    MenuAction { height: 24px; label: "删除频道"; danger: true; enabled: root.can-delete-channels; selected => { root.delete-channel(root.context-channel-index); root.context-menu-kind = 0; } }
                }
            }
        }

        if (root.context-menu-kind == 2 && root.show-main && root.window-mode == 0 && !root.show-settings && !root.show-network-details) : user-context-layer := Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.context-menu-kind = 0; } }
            user-context-menu := Rectangle {
                width: 190px; height: 300px; x: 96px; y: min(parent.height - self.height - 12px, root.topbar-height + 108px);
                background: #ffffff; border-radius: 8px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f243018; drop-shadow-blur: 14px; drop-shadow-offset-y: 4px;
                VerticalLayout { padding: 7px; spacing: 1px;
                    MenuAction { height: 24px; label: "私聊"; enabled: false; }
                    user-context-info := MenuAction { height: 24px; label: "查看信息"; selected => { root.context-menu-kind = 0; root.show-user-card = true; } }
                    HorizontalLayout { height: 34px; padding-left: 10px; padding-right: 8px; spacing: 6px;
                        Text { width: 25px; text: "音量"; font-size: 10px; color: #555b68; vertical-alignment: center; }
                        user-volume-slider := Slider { horizontal-stretch: 1; minimum: 0; maximum: 3; step: 0.05; value <=> root.context-user-volume; changed(value) => { root.set-user-volume(root.context-user-session, value); } }
                        Text { width: 31px; text: round(root.context-user-volume * 100) + "%"; font-size: 9px; color: #666c78; vertical-alignment: center; horizontal-alignment: right; }
                    }
                    user-context-mute := MenuAction { height: 24px; label: root.context-user-locally-muted ? "取消本地静音" : "静音"; selected => { root.context-user-locally-muted = !root.context-user-locally-muted; root.set-user-local-mute(root.context-user-session, root.context-user-locally-muted); root.context-menu-kind = 0; } }
                    Rectangle { height: 1px; background: #eceef2; }
                    user-context-admin-mute := MenuAction { height: 24px; label: root.context-user-server-muted ? ((root.server-role == "MASTER" || root.server-role == "ADMIN") ? "解除服务器禁言" : "服务器禁言（无权解除）") : root.context-user-channel-muted ? "解除房间禁言" : ((root.server-role == "MASTER" || root.server-role == "ADMIN") ? "服务器禁言" : "房间禁言"); enabled: root.can-admin-mute-users && root.can-administer-context-user && root.context-user-session != root.self-session-id && (!root.context-user-server-muted || root.server-role == "MASTER" || root.server-role == "ADMIN"); selected => { root.admin-mute-user(root.context-user-session, !root.context-user-admin-muted); root.context-menu-kind = 0; } }
                    user-context-move := MenuAction { height: 24px; label: "移动到我的频道"; enabled: root.can-move-users && root.can-administer-context-user && root.can-administer-own-channel && root.context-user-session != root.self-session-id; selected => { root.move-user-to-current-channel(root.context-user-session); root.context-menu-kind = 0; } }
                    user-context-channel-admin := MenuAction { height: 24px; label: root.context-user-channel-role == "CHANNEL_ADMIN" ? "取消频道管理员" : "设为频道管理员"; enabled: root.can-set-channel-roles && root.context-user-session != root.self-session-id; selected => { root.set-user-channel-admin(root.context-user-session, root.context-user-channel-role != "CHANNEL_ADMIN"); root.context-menu-kind = 0; } }
                    user-context-server-admin := MenuAction { height: 24px; label: root.context-user-server-role == "ADMIN" ? "取消服务器管理员" : "设为服务器管理员"; enabled: root.can-set-server-roles && root.context-user-server-role != "MASTER" && root.context-user-session != root.self-session-id; selected => { root.set-user-server-admin(root.context-user-session, root.context-user-server-role != "ADMIN"); root.context-menu-kind = 0; } }
                    user-context-kick-channel := MenuAction { height: 24px; label: "踢出频道"; danger: true; enabled: root.can-kick-channel-users && root.can-administer-context-user && root.context-user-session != root.self-session-id; selected => { root.kick-user-channel(root.context-user-session); root.context-menu-kind = 0; } }
                    user-context-kick-server := MenuAction { height: 24px; label: "踢出服务器"; danger: true; enabled: root.can-kick-server-users && root.context-user-session != root.self-session-id; selected => { root.kick-user-server(root.context-user-session); root.context-menu-kind = 0; } }
                    user-context-ban := MenuAction { height: 24px; label: "封禁身份"; danger: true; enabled: root.can-ban-users && root.context-user-session != root.self-session-id; selected => { root.ban-user(root.context-user-session); root.context-menu-kind = 0; } }
                }
            }
        }

        if (root.show-user-card && root.show-main && root.window-mode == 0 && !root.show-settings && !root.show-network-details) : user-card-layer := Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-user-card = false; } }
            user-info-card := Rectangle {
                width: 220px; height: 276px; x: parent.width - self.width - 18px; y: min(parent.height - self.height - 12px, root.topbar-height + 72px);
                background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f24301f; drop-shadow-blur: 18px; drop-shadow-offset-y: 5px;
                VerticalLayout { padding-left: 15px; padding-right: 15px; padding-top: 14px; padding-bottom: 11px; spacing: 3px;
                    HorizontalLayout { height: 39px; spacing: 9px;
                        EvAvatar { avatar: root.context-user-avatar; avatar-image: root.context-user-avatar-image; custom-avatar: root.context-user-custom-avatar; speaking: root.context-user-speaking; avatar-size: 30px; }
                        VerticalLayout { spacing: 1px; Text { text: root.context-user-name; font-size: 13px; font-weight: 700; color: #292d37; overflow: elide; } Text { text: root.context-user-speaking ? "正在说话" : "在线"; font-size: 10px; color: root.context-user-speaking ? #29a965 : #8b909b; } }
                        Rectangle { horizontal-stretch: 1; }
                        if (root.context-user-speaking) : Icon { name: "signal"; stroke: #2fa868; width: 15px; height: 15px; }
                    }
                    Rectangle { height: 1px; background: #eceef2; }
                    DetailRow { height: 18px; label: "语音路径 RTT"; value: root.latency; }
                    DetailRow { height: 18px; label: "丢包率"; value: root.packet-loss-label; }
                    DetailRow { height: 18px; label: "语音质量"; value: root.network-quality == "critical" ? "很差" : root.network-quality == "poor" ? "较差" : root.network-quality == "fair" ? "一般" : "良好"; }
                    DetailRow { height: 18px; label: "加入时间"; value: "--"; }
                    DetailRow { height: 18px; label: "当前频道"; value: root.context-user-channel == "" ? root.channel-name : root.context-user-channel; }
                    DetailRow { height: 18px; label: "网络模式"; value: root.topology; }
                    DetailRow { height: 18px; label: "麦克风"; value: root.context-user-muted ? "已静音" : "开启"; }
                    DetailRow { height: 18px; label: "扬声器"; value: root.context-user-deafened ? "已静音" : "开启"; }
                    Rectangle { height: 1px; background: #eceef2; }
                    HorizontalLayout { height: 28px; spacing: 14px;
                        Rectangle { horizontal-stretch: 1; }
                        Icon { name: root.context-user-muted ? "mic-off" : "mic"; stroke: root.context-user-muted ? #d45f65 : #4f5664; width: 17px; height: 17px; }
                        Icon { name: root.context-user-deafened ? "volume-off" : "headphones"; stroke: root.context-user-deafened ? #d45f65 : #4f5664; width: 17px; height: 17px; }
                    }
                }
            }
        }

        if (root.show-network-details) : Rectangle {
            width: parent.width; height: parent.height; background: transparent;
            TouchArea { clicked => { root.show-network-details = false; } }
            network-details-card := Rectangle {
                width: 220px; height: 310px; x: parent.width - self.width - 14px; y: root.topbar-height + 48px;
                background: #ffffff; border-radius: 11px; border-width: 1px; border-color: #dfe1e8; drop-shadow-color: #1f24301f; drop-shadow-blur: 20px; drop-shadow-offset-y: 6px;
                VerticalLayout { padding: 12px; spacing: 5px;
                    HorizontalLayout { height: 31px; spacing: 4px;
                        Rectangle { horizontal-stretch: 1; border-radius: 6px; background: root.network-details-tab == 0 ? #e8ebff : transparent; Text { text: "网络"; font-size: 11px; font-weight: root.network-details-tab == 0 ? 700 : 500; color: root.network-details-tab == 0 ? #4d5bd5 : #6f7582; horizontal-alignment: center; vertical-alignment: center; } TouchArea { clicked => { root.network-details-tab = 0; } } }
                        Rectangle { horizontal-stretch: 1; border-radius: 6px; background: root.network-details-tab == 1 ? #e8ebff : transparent; Text { text: "音频"; font-size: 11px; font-weight: root.network-details-tab == 1 ? 700 : 500; color: root.network-details-tab == 1 ? #4d5bd5 : #6f7582; horizontal-alignment: center; vertical-alignment: center; } TouchArea { clicked => { root.network-details-tab = 1; } } }
                        Rectangle { horizontal-stretch: 1; border-radius: 6px; background: root.network-details-tab == 2 ? #e8ebff : transparent; Text { text: "设备"; font-size: 11px; font-weight: root.network-details-tab == 2 ? 700 : 500; color: root.network-details-tab == 2 ? #4d5bd5 : #6f7582; horizontal-alignment: center; vertical-alignment: center; } TouchArea { clicked => { root.network-details-tab = 2; } } }
                    }
                    Rectangle { height: 1px; background: #eceef2; }
                    if (root.network-details-tab == 0) : VerticalLayout { vertical-stretch: 1; spacing: 4px;
                        DetailRow { height: 24px; label: "网络模式"; value: root.topology; }
                        DetailRow { height: 24px; label: "语音路径 RTT"; value: root.latency; }
                        DetailRow { height: 24px; label: "控制连接 RTT"; value: root.control-latency; }
                        DetailRow { height: 24px; label: "抖动缓冲"; value: root.jitter-label; }
                        DetailRow { height: 24px; label: "丢包率"; value: root.packet-loss-label; }
                        DetailRow { height: 24px; label: "上行带宽"; value: root.uplink-label; }
                        DetailRow { height: 24px; label: "下行带宽"; value: root.downlink-label; }
                        DetailRow { height: 24px; label: "连接时长"; value: root.connection-duration-label; }
                        Rectangle { vertical-stretch: 1; }
                        Text { height: 20px; text: "实时会话数据 · 暂缺指标显示为 --"; font-size: 8px; color: #a0a4ae; }
                    }
                    if (root.network-details-tab == 1) : VerticalLayout { vertical-stretch: 1; spacing: 4px;
                        DetailRow { label: "编解码器"; value: root.codec-label; }
                        DetailRow { label: "降噪"; value: root.dsp-label; }
                        DetailRow { label: "回声消除"; value: root.aec-label; }
                        DetailRow { label: "自动增益"; value: root.agc-label; }
                        DetailRow { label: "输入电平"; value: round(root.input-level * 100) + "%"; }
                        Rectangle { height: 8px; border-radius: 4px; background: #edf0f4; Rectangle { width: parent.width * root.input-level; height: parent.height; border-radius: 4px; background: root.input-level > 0.85 ? #e46669 : #35ad68; } }
                        Rectangle { vertical-stretch: 1; }
                    }
                    if (root.network-details-tab == 2) : VerticalLayout { vertical-stretch: 1; spacing: 4px;
                        Text { height: 18px; text: "输入设备"; font-size: 10px; color: #858b97; }
                        Text { height: 32px; text: root.input-device; overflow: elide; font-size: 11px; font-weight: 600; color: #303540; }
                        DetailRow { label: "输入采样率"; value: root.input-sample-rate-label; }
                        Rectangle { height: 1px; background: #eceef2; }
                        Text { height: 18px; text: "输出设备"; font-size: 10px; color: #858b97; }
                        Text { height: 32px; text: root.output-device; overflow: elide; font-size: 11px; font-weight: 600; color: #303540; }
                        DetailRow { label: "输出采样率"; value: root.output-sample-rate-label; }
                        Rectangle { vertical-stretch: 1; }
                    }
                }
            }
        }

        if (root.editing-server-index >= 0 && !root.show-main) : Rectangle {
            width: parent.width; height: parent.height; background: #55141724;
            TouchArea { clicked => { root.editing-server-index = -1; } }
            server-edit-card := Rectangle {
                width: parent.width - 16px; height: 350px; x: 8px; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 10px; border-width: 1px; border-color: #dfe1e7; drop-shadow-color: #1f24301f; drop-shadow-blur: 18px; drop-shadow-offset-y: 5px;
                VerticalLayout { width: parent.width; height: parent.height; padding: 14px; spacing: 8px;
                    HorizontalLayout { height: 28px; Text { text: "编辑服务器"; font-size: 15px; font-weight: 700; color: #242832; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } RoundIconButton { width: 26px; height: 26px; icon-size: 14px; icon-name: "close"; clicked => { root.editing-server-index = -1; } } }
                    Text { height: 16px; text: "显示名称"; font-size: 11px; color: #555b68; }
                    FormLineEdit { height: 36px; text <=> root.editing-server-name; placeholder-text: "由服务器提供，也可自定义"; }
                    Text { height: 16px; text: "服务器地址"; font-size: 11px; color: #555b68; }
                    FormLineEdit { height: 36px; text <=> root.editing-server-address; placeholder-text: "例如 voice.example.com:9000"; }
                    Text { height: 16px; text: "加入昵称"; font-size: 11px; color: #555b68; }
                    FormLineEdit { height: 36px; text <=> root.editing-server-nickname; placeholder-text: "每次连接使用的昵称"; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout { height: 34px; spacing: 8px; Rectangle { horizontal-stretch: 1; } FormButton { width: 78px; text: "取消"; clicked => { root.editing-server-index = -1; } } FormButton { width: 92px; text: "保存"; primary: true; enabled: root.editing-server-address != "" && root.editing-server-nickname != ""; clicked => { root.save-saved-server(root.editing-server-index, root.editing-server-name, root.editing-server-address, root.editing-server-nickname); root.editing-server-index = -1; } } }
                }
            }
        }

        if (root.show-settings && root.visual-demo) : Rectangle {
            width: parent.width; height: parent.height; background: #ffffff;
            Rectangle { width: parent.width - 16px; height: parent.height - 16px; x: 8px; y: 8px; background: #ffffff; border-radius: 9px; border-width: 1px; border-color: #e4e7ee;
                VerticalLayout { padding: 12px; spacing: 5px;
                    HorizontalLayout { height: 20px; Text { text: "设置"; font-size: 13px; font-weight: 600; color: #2d313b; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } Icon { name: "close"; stroke: #666d7a; width: 14px; height: 14px; } }
                    HorizontalLayout { height: 23px; spacing: 4px;
                        Rectangle { horizontal-stretch: 1; border-radius: 5px; background: root.settings-page == 0 ? #e8ebff : transparent; Text { text: "音频"; font-size: 9px; font-weight: 600; color: root.settings-page == 0 ? #4d5bd5 : #737987; horizontal-alignment: center; vertical-alignment: center; } }
                        Rectangle { horizontal-stretch: 1; border-radius: 5px; background: root.settings-page == 1 ? #e8ebff : transparent; Text { text: "房间网络"; font-size: 9px; font-weight: 600; color: root.settings-page == 1 ? #4d5bd5 : #737987; horizontal-alignment: center; vertical-alignment: center; } }
                        Rectangle { horizontal-stretch: 1; border-radius: 5px; background: root.settings-page == 2 ? #e8ebff : transparent; Text { text: "快捷键"; font-size: 9px; font-weight: 600; color: root.settings-page == 2 ? #4d5bd5 : #737987; horizontal-alignment: center; vertical-alignment: center; } }
                    }
                    if (root.settings-page == 0) : HorizontalLayout { vertical-stretch: 1; spacing: 14px;
                        VerticalLayout { horizontal-stretch: 1; spacing: 5px;
                            Text { height: 14px; text: "输入与输出"; font-size: 9px; font-weight: 600; color: #747a87; }
                            Text { height: 12px; text: "麦克风"; font-size: 8px; color: #626875; }
                            Rectangle { height: 22px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { x: 7px; text: "系统默认麦克风"; font-size: 8px; color: #4b505c; vertical-alignment: center; } Icon { name: "chevron"; width: 11px; height: 11px; x: parent.width - 17px; y: 5px; } }
                            HorizontalLayout { height: 19px; Text { width: 38px; text: "输入增益"; font-size: 8px; color: #626875; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; height: 3px; y: 8px; border-radius: 2px; background: #e1e4eb; Rectangle { width: parent.width * 0.58; height: parent.height; border-radius: 2px; background: #5261e6; } } Text { width: 24px; text: "100%"; font-size: 7px; color: #737987; vertical-alignment: center; horizontal-alignment: right; } }
                            Text { height: 12px; text: "扬声器"; font-size: 8px; color: #626875; }
                            Rectangle { height: 22px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { x: 7px; text: "系统默认输出"; font-size: 8px; color: #4b505c; vertical-alignment: center; } Icon { name: "chevron"; width: 11px; height: 11px; x: parent.width - 17px; y: 5px; } }
                            HorizontalLayout { height: 19px; Text { width: 38px; text: "输出音量"; font-size: 8px; color: #626875; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; height: 3px; y: 8px; border-radius: 2px; background: #e1e4eb; Rectangle { width: parent.width * 0.8; height: parent.height; border-radius: 2px; background: #5261e6; } } Text { width: 24px; text: "100%"; font-size: 7px; color: #737987; vertical-alignment: center; horizontal-alignment: right; } }
                            Rectangle { height: 23px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { text: "播放测试音"; font-size: 8px; color: #525866; horizontal-alignment: center; vertical-alignment: center; } }
                        }
                        Rectangle { width: 1px; background: #eceef2; }
                        VerticalLayout { horizontal-stretch: 1; spacing: 3px;
                            Text { height: 14px; text: "语音处理"; font-size: 9px; font-weight: 600; color: #747a87; }
                            Text { height: 12px; text: "输入模式"; font-size: 8px; color: #626875; }
                            Rectangle { height: 22px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { x: 7px; text: "语音活动检测"; font-size: 8px; color: #4b505c; vertical-alignment: center; } Icon { name: "chevron"; width: 11px; height: 11px; x: parent.width - 17px; y: 5px; } }
                            Text { height: 12px; text: "降噪模式"; font-size: 8px; color: #626875; }
                            Rectangle { height: 22px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { x: 7px; text: "RNNoise（自动）"; font-size: 8px; color: #4b505c; vertical-alignment: center; } Icon { name: "chevron"; width: 11px; height: 11px; x: parent.width - 17px; y: 5px; } }
                            FormCheckBox { height: 20px; text: "回声消除 (AEC)"; checked: true; }
                            FormCheckBox { height: 20px; text: "自动增益 (AGC)"; checked: true; }
                            Rectangle { height: 23px; border-radius: 5px; border-width: 1px; border-color: #dfe3eb; Text { text: "开始录音"; font-size: 8px; color: #525866; horizontal-alignment: center; vertical-alignment: center; } }
                        }
                    }
                    if (root.settings-page == 1) : VerticalLayout { vertical-stretch: 1; spacing: 4px;
                        HorizontalLayout { vertical-stretch: 1; spacing: 14px;
                            VerticalLayout { horizontal-stretch: 1; spacing: 3px;
                                Text { height: 14px; text: "网络模式"; font-size: 9px; font-weight: 600; color: #747a87; }
                                NetworkPolicyCard { height: 37px; selected: true; title: "自动（推荐）"; description: "根据人数和网络质量选择连接方式。"; }
                                NetworkPolicyCard { height: 37px; title: "强制 SFU（服务器转发）"; description: "所有语音由服务器转发。"; }
                                NetworkPolicyCard { height: 37px; title: "强制 P2P（点对点直连）"; description: "优先建立成员直接连接。"; }
                            }
                            Rectangle { width: 1px; background: #eceef2; }
                            VerticalLayout { horizontal-stretch: 1; spacing: 3px;
                                Text { height: 14px; text: "延迟信息"; font-size: 9px; font-weight: 600; color: #747a87; }
                                ReferenceMetricSlider { label: "延迟阈值"; value-label: "200ms"; progress: 0.72; }
                                ReferenceMetricSlider { label: "丢包率阈值"; value-label: "5%"; progress: 0.63; }
                                ReferenceMetricSlider { label: "抖动阈值"; value-label: "30ms"; progress: 0.68; }
                            }
                        }
                        Rectangle { height: 1px; background: #eceef2; }
                        HorizontalLayout { height: 23px; spacing: 6px;
                            Text { text: "高级选项"; font-size: 8px; font-weight: 600; color: #747a87; vertical-alignment: center; }
                            Rectangle { width: 24px; height: 13px; y: 5px; border-radius: 7px; background: #e1e4eb; Rectangle { width: 9px; height: 9px; x: 2px; y: 2px; border-radius: 5px; background: #ffffff; } }
                            Text { text: "启用带宽限制"; font-size: 8px; color: #8a909c; vertical-alignment: center; }
                            Rectangle { horizontal-stretch: 1; }
                            Text { text: "最大带宽   128 kbps"; font-size: 8px; color: #8a909c; vertical-alignment: center; }
                        }
                    }
                    if (root.settings-page == 2) : VerticalLayout { vertical-stretch: 1; spacing: 0px;
                        Text { height: 15px; text: "功能"; font-size: 8px; font-weight: 600; color: #747a87; }
                        ReferenceShortcutRow { label: "切换简 / 大窗口"; shortcut: "未绑定"; }
                        ReferenceShortcutRow { label: "静音 / 取消静音"; shortcut: "未绑定"; }
                        ReferenceShortcutRow { label: "扬声器静音"; shortcut: "未绑定"; }
                        ReferenceShortcutRow { label: "按键说话 (PTT)"; shortcut: "未绑定"; }
                        ReferenceShortcutRow { label: "显示 / 隐藏用户列表"; shortcut: "未绑定"; }
                        Rectangle { vertical-stretch: 1; }
                    }
                    HorizontalLayout { height: 25px; spacing: 6px; Rectangle { horizontal-stretch: 1; } FormButton { width: 63px; height: 25px; text: "取消"; } FormButton { width: 78px; height: 25px; text: "保存设置"; primary: true; } }
                }
            }
        }

        if (root.show-settings && !root.visual-demo) : Rectangle {
            width: parent.width; height: parent.height; background: #660f1220;
            settings-card := Rectangle {
                width: root.settings-page == 1 ? 330px : parent.width - 16px;
                height: root.settings-page == 1 ? 396px : parent.height - (root.macos-titlebar-overlay ? 32px : 16px);
                x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 12px; border-width: 1px; border-color: #dfe1e8; drop-shadow-color: #1f243020; drop-shadow-blur: 22px; drop-shadow-offset-y: 6px;
                VerticalLayout { width: parent.width; height: parent.height; padding-left: 14px; padding-right: 14px; padding-top: 10px; padding-bottom: 10px; spacing: 5px;
                    HorizontalLayout { height: 28px; Text { text: root.settings-page == 1 ? (root.room-network-channel-name == "" ? "房间设置" : "房间设置 · " + root.room-network-channel-name) : "设置"; font-size: 17px; font-weight: 700; color: #242832; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; } if (root.settings-page == 0) : RoundIconButton { icon-name: "refresh"; clicked => { root.refresh-devices(); } } RoundIconButton { icon-name: "close"; clicked => { root.close-settings(); } } }
                    if (root.settings-page != 1) : settings-tabs := HorizontalLayout { height: 26px; spacing: 4px;
                        Rectangle { horizontal-stretch: 1; border-radius: 6px; background: root.settings-page == 0 ? #e8ebff : transparent; Text { text: "音频"; font-size: 11px; font-weight: root.settings-page == 0 ? 700 : 500; color: root.settings-page == 0 ? #4d5bd5 : #626875; horizontal-alignment: center; vertical-alignment: center; } audio-settings-tab := TouchArea { accessible-role: tab; accessible-label: "音频设置"; clicked => { root.settings-page = 0; } } }
                        Rectangle { horizontal-stretch: 1; border-radius: 6px; background: root.settings-page == 2 ? #e8ebff : transparent; Text { text: "快捷键"; font-size: 11px; font-weight: root.settings-page == 2 ? 700 : 500; color: root.settings-page == 2 ? #4d5bd5 : #626875; horizontal-alignment: center; vertical-alignment: center; } shortcuts-settings-tab := TouchArea { accessible-role: tab; accessible-label: "快捷键设置"; clicked => { root.settings-page = 2; } } }
                    }
                    if (root.settings-page == 0) : HorizontalLayout { vertical-stretch: 1; spacing: 10px;
                        VerticalLayout { horizontal-stretch: 1; spacing: 3px;
                            Text { height: 15px; text: "输入与输出"; font-size: 11px; font-weight: 700; color: #737987; }
                            HorizontalLayout { height: 29px; spacing: 6px; Text { width: 42px; text: "麦克风"; font-size: 10px; color: #555b68; vertical-alignment: center; } FormComboBox { height: 25px; horizontal-stretch: 1; model: root.input-devices; current-index <=> root.input-device-index; } }
                            HorizontalLayout { height: 24px; spacing: 6px; Text { width: 42px; text: "增益"; font-size: 10px; color: #555b68; vertical-alignment: center; } Slider { horizontal-stretch: 1; minimum: 0; maximum: 3; step: 0.05; value <=> root.input-gain; } Text { width: 28px; text: round(root.input-gain * 100) + "%"; font-size: 9px; vertical-alignment: center; } }
                            HorizontalLayout { height: 16px; spacing: 6px; Text { width: 42px; text: "电平"; font-size: 10px; color: #555b68; vertical-alignment: center; } Rectangle { horizontal-stretch: 1; height: 7px; border-radius: 4px; background: #edf0f4; Rectangle { width: parent.width * root.input-level; height: parent.height; border-radius: 4px; background: root.input-level > 0.85 ? #e46669 : #35ad68; } } }
                            Rectangle { height: 1px; background: #eceef2; }
                            HorizontalLayout { height: 29px; spacing: 6px; Text { width: 42px; text: "扬声器"; font-size: 10px; color: #555b68; vertical-alignment: center; } FormComboBox { height: 25px; horizontal-stretch: 1; model: root.output-devices; current-index <=> root.output-device-index; } }
                            Text { height: 24px; text: "内部固定 48 kHz，设备采样率自动适配"; font-size: 9px; color: #7d8390; vertical-alignment: center; }
                            HorizontalLayout { height: 24px; spacing: 6px; Text { width: 42px; text: "音量"; font-size: 10px; color: #555b68; vertical-alignment: center; } Slider { horizontal-stretch: 1; minimum: 0.01; maximum: 3; step: 0.01; value <=> root.output-volume; } Text { width: 32px; text: round(root.output-volume * 100) + "%"; font-size: 9px; vertical-alignment: center; } }
                            play-output-test-button := FormButton { height: 27px; text: "播放测试音"; clicked => { root.play-output-test(); } }
                            Text { height: 12px; text: root.audio-status; font-size: 9px; color: #7d8390; horizontal-alignment: center; }
                        }
                        Rectangle { width: 1px; background: #eceef2; }
                        VerticalLayout { horizontal-stretch: 1; spacing: 5px;
                            Text { height: 15px; text: "语音处理"; font-size: 11px; font-weight: 700; color: #737987; }
                            HorizontalLayout { height: 29px; spacing: 6px; Text { width: 48px; text: "输入模式"; font-size: 10px; color: #555b68; vertical-alignment: center; } FormComboBox { height: 25px; horizontal-stretch: 1; model: ["语音活动检测", "按键说话"]; current-index <=> root.input-mode-index; } }
                            if (root.input-mode-index == 1) : Text { height: 25px; text: root.ptt-key == "" ? "请在快捷键页绑定按键说话" : "按键说话：" + root.ptt-key; font-size: 9px; color: root.ptt-key == "" ? #d28a35 : #69707d; vertical-alignment: center; }
                            HorizontalLayout { height: 29px; spacing: 6px; Text { width: 48px; text: "降噪模式"; font-size: 10px; color: #555b68; vertical-alignment: center; } FormComboBox { height: 25px; horizontal-stretch: 1; model: ["关闭", "标准降噪", "RNNoise", "DeepFilterNet · 自然", "DeepFilterNet · 强力"]; current-index <=> root.noise-mode-index; } }
                            if (root.noise-mode-index == 1) : HorizontalLayout { height: 29px; spacing: 6px; Text { width: 48px; text: "降噪强度"; font-size: 10px; color: #555b68; vertical-alignment: center; } FormComboBox { height: 25px; horizontal-stretch: 1; model: ["低", "中", "高"]; current-index <=> root.noise-strength-index; } }
                            if (root.noise-mode-index == 2 || root.noise-mode-index == 3 || root.noise-mode-index == 4) : HorizontalLayout { height: 27px; spacing: 6px; Text { width: 48px; text: "降噪强度"; font-size: 10px; color: #555b68; vertical-alignment: center; } Text { text: root.noise-mode-index == 4 ? "强力人声隔离（约 70 ms）" : (root.noise-mode-index == 3 ? "自然增强（约 70 ms）" : "自动（模型自适应）"); font-size: 9px; color: #79808d; vertical-alignment: center; } }
                            if (root.noise-mode-index == 0) : HorizontalLayout { height: 27px; spacing: 6px; Text { width: 48px; text: "降噪强度"; font-size: 10px; color: #9ca1ab; vertical-alignment: center; } Text { text: "不适用"; font-size: 10px; color: #a1a6b0; vertical-alignment: center; } }
                            echo-cancellation-check := FormCheckBox { height: 23px; text: "回声消除 (AEC)"; checked <=> root.echo-cancellation; }
                            automatic-gain-check := FormCheckBox { height: 23px; text: "自动增益 (AGC)"; checked <=> root.automatic-gain; }
                            Rectangle { height: 1px; background: #eceef2; }
                            Text { height: 13px; text: "麦克风测试 · " + root.mic-test-status; font-size: 10px; color: #555b68; }
                            mic-test-button := FormButton { height: 27px; text: root.mic-test-status == "录音中" ? "停止录音" : "开始录音"; clicked => { if (root.mic-test-status == "录音中") { root.stop-mic-test(); } else { root.start-mic-test(); } } }
                            HorizontalLayout { height: 27px; spacing: 6px; play-mic-original-button := FormButton { horizontal-stretch: 1; text: "播放原声"; enabled: root.mic-test-status == "可试听"; clicked => { root.play-mic-original(); } } play-mic-processed-button := FormButton { horizontal-stretch: 1; text: "播放处理后"; enabled: root.mic-test-status == "可试听"; clicked => { root.play-mic-processed(); } } }
                        }
                    }
                    if (root.settings-page == 1) : Rectangle { vertical-stretch: 1; background: transparent;
                        VerticalLayout { padding-left: 2px; padding-right: 2px; padding-top: 4px; spacing: 4px;
                            Text { height: 21px; text: "网络模式"; font-size: 14px; font-weight: 700; color: #2a2e38; }
                            Text { height: 32px; text: "决定房间使用的网络拓扑策略。切换后由服务器重新评估路由。"; wrap: word-wrap; font-size: 10px; color: #7b818e; }
                            NetworkPolicyCard { enabled: root.can-set-target-network-policy; selected: root.room-network-policy-index == 0; title: "自动（推荐）"; description: "根据人数和网络质量自动选择 SFU、P2P 或混合迁移。"; chosen => { root.room-network-policy-index = 0; } }
                            NetworkPolicyCard { enabled: root.can-set-target-network-policy; selected: root.room-network-policy-index == 1; title: "强制 SFU（服务器转发）"; description: "所有语音始终由服务器转发，连接行为最稳定。"; chosen => { root.room-network-policy-index = 1; } }
                            NetworkPolicyCard { enabled: root.can-set-target-network-policy; selected: root.room-network-policy-index == 2; title: "强制 P2P（点对点直连）"; description: "优先建立成员直连；条件不足时会暂时通过 SFU 恢复。"; chosen => { root.room-network-policy-index = 2; } }
                            Rectangle { vertical-stretch: 1; }
                        }
                    }
                    if (root.settings-page == 2) : Rectangle { vertical-stretch: 1; background: transparent;
                        VerticalLayout { padding-left: 8px; padding-right: 8px; padding-top: 5px; spacing: 5px;
                            Text { height: 18px; text: "快捷键"; font-size: 13px; font-weight: 700; color: #2a2e38; }
                            Text { height: 24px; text: "默认不绑定。点击右侧按键框后，可录入单键或 0.5 秒内完成的组合键。"; wrap: word-wrap; font-size: 9px; color: #7b818e; }
                            ShortcutRow { icon-name: "expand"; label: "切换简 / 大窗口"; description: "在默认模式和紧凑模式之间切换"; shortcut: root.shortcut-toggle-window; recording: root.recording-shortcut == 0; record => { app-shortcuts.focus(); root.begin-shortcut-capture(0); } clear => { root.clear-shortcut(0); } }
                            ShortcutRow { icon-name: "mic-off"; label: "静音 / 取消静音"; description: "切换本机麦克风状态"; shortcut: root.shortcut-toggle-mic; recording: root.recording-shortcut == 1; record => { app-shortcuts.focus(); root.begin-shortcut-capture(1); } clear => { root.clear-shortcut(1); } }
                            ShortcutRow { icon-name: "volume-off"; label: "扬声器静音"; description: "切换本机扬声器状态"; shortcut: root.shortcut-toggle-speaker; recording: root.recording-shortcut == 2; record => { app-shortcuts.focus(); root.begin-shortcut-capture(2); } clear => { root.clear-shortcut(2); } }
                            ShortcutRow { icon-name: "mic"; label: "按键说话"; description: "仅在按键说话模式下生效"; shortcut: root.shortcut-push-to-talk; recording: root.recording-shortcut == 3; record => { app-shortcuts.focus(); root.begin-shortcut-capture(3); } clear => { root.clear-shortcut(3); } }
                            ShortcutRow { icon-name: "users"; label: "显示 / 隐藏用户列表"; description: "打开当前频道的成员列表"; shortcut: root.shortcut-toggle-members; recording: root.recording-shortcut == 4; record => { app-shortcuts.focus(); root.begin-shortcut-capture(4); } clear => { root.clear-shortcut(4); } }
                            Rectangle { vertical-stretch: 1; }
                        }
                    }
                    Text { height: root.settings-error == "" ? 0px : 28px; text: root.settings-error; wrap: word-wrap; font-size: 10px; color: #d65359; }
                    HorizontalLayout { height: 32px; spacing: 8px; Rectangle { horizontal-stretch: 1; } cancel-settings-button := FormButton { width: 82px; text: "取消"; clicked => { root.close-settings(); } } save-settings-button := FormButton { width: 100px; text: "保存设置"; primary: true; clicked => { root.save-audio-settings(); } } }
                }
            }
        }

        if (root.show-master-provision && root.show-main) : Rectangle {
            width: parent.width; height: parent.height; background: #11131888;
            TouchArea { clicked => { root.show-master-provision = false; root.master-provision-token = ""; } }
            Rectangle {
                width: 410px; height: 215px; x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: #ffffff; border-radius: 11px; border-width: 1px; border-color: #d8dbe3;
                VerticalLayout { padding: 20px; spacing: 11px;
                    Text { text: "认领服务器 MASTER"; font-size: 16px; font-weight: 700; color: #242832; }
                    Text { height: 34px; text: "输入服务器首次启动时生成的 Master Provision Token。认领成功后，该 Token 立即失效。"; wrap: word-wrap; font-size: 10px; color: #646b78; }
                    FormLineEdit { text <=> root.master-provision-token; placeholder-text: "Master Provision Token"; accepted => { if (root.master-provision-token != "") { root.provision-master(root.master-provision-token); root.master-provision-token = ""; root.show-master-provision = false; } } }
                    HorizontalLayout { height: 34px; spacing: 8px;
                        Rectangle { horizontal-stretch: 1; }
                        FormButton { width: 82px; text: "取消"; clicked => { root.show-master-provision = false; root.master-provision-token = ""; } }
                        FormButton { width: 108px; text: "认领 MASTER"; primary: true; enabled: root.master-provision-token != ""; clicked => { root.provision-master(root.master-provision-token); root.master-provision-token = ""; root.show-master-provision = false; } }
                    }
                }
            }
        }
    }

}

pub fn run() -> Result<(), slint::PlatformError> {
    install_native_platform()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();
    let app = Rc::new(RefCell::new(Application::new()));
    let window = AppWindow::new()?;
    window.set_ui_font_family(ui_font_family().into());
    #[cfg(target_os = "macos")]
    {
        window.set_macos_titlebar_overlay(true);
    }
    let tree_model = Rc::new(VecModel::from(Vec::<TreeItem>::new()));
    let member_model = Rc::new(VecModel::from(Vec::<MemberItem>::new()));
    let compact_channel_model = Rc::new(VecModel::from(Vec::<CompactChannelItem>::new()));
    let compact_avatar_model = Rc::new(VecModel::from(Vec::<MemberItem>::new()));
    let saved_server_model = Rc::new(VecModel::from(Vec::<SavedServerItem>::new()));
    let recent_server_model = Rc::new(VecModel::from(Vec::<SavedServerItem>::new()));
    let ban_model = Rc::new(VecModel::from(Vec::<BanItem>::new()));
    let chat_model = Rc::new(VecModel::from(Vec::<ChatItem>::new()));
    let input_device_model = Rc::new(VecModel::from(Vec::<SharedString>::new()));
    let output_device_model = Rc::new(VecModel::from(Vec::<SharedString>::new()));
    window.set_tree_items(ModelRc::from(tree_model.clone()));
    window.set_members(ModelRc::from(member_model.clone()));
    window.set_compact_channels(ModelRc::from(compact_channel_model.clone()));
    window.set_compact_avatar_members(ModelRc::from(compact_avatar_model.clone()));
    window.set_saved_servers(ModelRc::from(saved_server_model.clone()));
    window.set_recent_servers(ModelRc::from(recent_server_model.clone()));
    window.set_bans(ModelRc::from(ban_model.clone()));
    window.set_chat_items(ModelRc::from(chat_model.clone()));
    window.set_input_devices(ModelRc::from(input_device_model.clone()));
    window.set_output_devices(ModelRc::from(output_device_model.clone()));
    window.set_app_version(env!("CARGO_PKG_VERSION").into());
    let shortcut_draft = Rc::new(RefCell::new(app.borrow().shortcut_bindings().clone()));
    let shortcut_capture = Rc::new(RefCell::new(ShortcutRecorder::default()));
    let avatar_crop_draft = Rc::new(RefCell::new(None::<CropDraft>));
    {
        let app_ref = app.borrow();
        window.set_server_address(app_ref.server_address().into());
        window.set_nickname(app_ref.nickname().into());
        window.set_self_initials(initials(app_ref.nickname()));
        let initial_avatar = if app_ref.avatar().is_empty() {
            initials(app_ref.nickname())
        } else {
            app_ref.avatar().into()
        };
        window.set_self_avatar(initial_avatar);
        window.set_self_custom_avatar(is_custom_avatar(app_ref.avatar()));
        window.set_self_avatar_image(image_for_avatar(app_ref.avatar()));
        update_model(
            &saved_server_model,
            saved_server_items(app_ref.saved_servers()),
        );
        update_model(
            &recent_server_model,
            recent_server_items(app_ref.saved_servers()),
        );
        sync_audio_settings(
            &window,
            &app_ref,
            &input_device_model,
            &output_device_model,
            true,
        );
        sync_shortcut_settings(&window, app_ref.shortcut_bindings());
    }
    let app_for_chat = app.clone();
    window.on_send_chat(move |text| {
        app_for_chat
            .borrow_mut()
            .dispatch(AppCommand::SendTextMessage(text.to_string()));
    });
    let app_for_avatar = app.clone();
    window.on_set_avatar(move |avatar| {
        app_for_avatar
            .borrow_mut()
            .dispatch(AppCommand::SetAvatar(avatar.to_string()));
    });
    let weak_for_choose_avatar = window.as_weak();
    let draft_for_choose_avatar = avatar_crop_draft.clone();
    window.on_choose_avatar_image(move || {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("图片", &["jpg", "jpeg", "png", "webp", "gif"])
            .pick_file()
        else {
            return;
        };
        let Some(window) = weak_for_choose_avatar.upgrade() else {
            return;
        };
        match CropDraft::load(&path) {
            Ok(draft) => {
                let (width, height) = draft.dimensions();
                window.set_avatar_crop_image(draft.preview());
                window.set_avatar_crop_width(width as f32);
                window.set_avatar_crop_height(height as f32);
                window.set_avatar_crop_zoom(1.0);
                window.set_avatar_crop_offset_x(0.0);
                window.set_avatar_crop_offset_y(0.0);
                window.set_show_avatar_picker(false);
                window.set_show_avatar_crop(true);
                *draft_for_choose_avatar.borrow_mut() = Some(draft);
            }
            Err(error) => window.set_connection_error(format!("无法使用这张图片：{error}").into()),
        }
    });
    let weak_for_save_avatar = window.as_weak();
    let app_for_save_avatar = app.clone();
    let draft_for_save_avatar = avatar_crop_draft.clone();
    window.on_save_avatar_crop(move |zoom, offset_x, offset_y| {
        let Some(window) = weak_for_save_avatar.upgrade() else {
            return;
        };
        let result = draft_for_save_avatar
            .borrow()
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有待裁剪的图片"))
            .and_then(|draft| draft.encode(zoom, offset_x, offset_y));
        match result {
            Ok(avatar) => {
                app_for_save_avatar
                    .borrow_mut()
                    .dispatch(AppCommand::SetAvatar(avatar));
                window.set_show_avatar_crop(false);
                *draft_for_save_avatar.borrow_mut() = None;
            }
            Err(error) => window.set_connection_error(format!("头像保存失败：{error}").into()),
        }
    });
    let weak_for_begin_shortcut = window.as_weak();
    let capture_for_begin_shortcut = shortcut_capture.clone();
    window.on_begin_shortcut_capture(move |action| {
        if !(0..=4).contains(&action) {
            return;
        }
        capture_for_begin_shortcut.borrow_mut().begin(action);
        if let Some(window) = weak_for_begin_shortcut.upgrade() {
            window.set_recording_shortcut(action);
        }
    });
    let weak_for_clear_shortcut = window.as_weak();
    let draft_for_clear_shortcut = shortcut_draft.clone();
    let capture_for_clear_shortcut = shortcut_capture.clone();
    window.on_clear_shortcut(move |action| {
        if !(0..=4).contains(&action) {
            return;
        }
        set_shortcut_binding(
            &mut draft_for_clear_shortcut.borrow_mut(),
            action,
            String::new(),
        );
        capture_for_clear_shortcut.borrow_mut().cancel();
        if let Some(window) = weak_for_clear_shortcut.upgrade() {
            window.set_recording_shortcut(-1);
            sync_shortcut_settings(&window, &draft_for_clear_shortcut.borrow());
        }
    });
    let app_for_shortcut_key = app.clone();
    let weak_for_shortcut_key = window.as_weak();
    let draft_for_shortcut_key = shortcut_draft.clone();
    let capture_for_shortcut_key = shortcut_capture.clone();
    window.on_shortcut_key_pressed(move |text, control, alt, shift, meta| {
        let text = text.to_string();
        match capture_for_shortcut_key.borrow_mut().record(
            &text,
            control,
            alt,
            shift,
            meta,
            Instant::now(),
        ) {
            CaptureProgress::ModifierStarted { action, generation } => {
                let capture_for_timeout = capture_for_shortcut_key.clone();
                let weak_for_timeout = weak_for_shortcut_key.clone();
                Timer::single_shot(Duration::from_millis(CAPTURE_WINDOW_MS as u64), move || {
                    if capture_for_timeout
                        .borrow_mut()
                        .expire(action, generation, Instant::now())
                        && let Some(window) = weak_for_timeout.upgrade()
                    {
                        window.set_recording_shortcut(-1);
                    }
                });
                99
            }
            CaptureProgress::Waiting => 99,
            CaptureProgress::Recorded { action, binding } => {
                set_shortcut_binding(&mut draft_for_shortcut_key.borrow_mut(), action, binding);
                if let Some(window) = weak_for_shortcut_key.upgrade() {
                    window.set_recording_shortcut(-1);
                    sync_shortcut_settings(&window, &draft_for_shortcut_key.borrow());
                }
                99
            }
            CaptureProgress::Expired => {
                if let Some(window) = weak_for_shortcut_key.upgrade() {
                    window.set_recording_shortcut(-1);
                }
                99
            }
            CaptureProgress::Inactive => {
                let Some(binding) = binding_from_key_event(&text, control, alt, shift, meta) else {
                    return -1;
                };
                matching_shortcut_action(
                    app_for_shortcut_key.borrow().shortcut_bindings(),
                    &binding,
                )
            }
        }
    });
    let weak_window = window.as_weak();
    let weak_app = Rc::downgrade(&app);
    let tree_model_for_refresh = tree_model.clone();
    let member_model_for_refresh = member_model.clone();
    let compact_channel_model_for_refresh = compact_channel_model.clone();
    let compact_avatar_model_for_refresh = compact_avatar_model.clone();
    let saved_server_model_for_refresh = saved_server_model.clone();
    let recent_server_model_for_refresh = recent_server_model.clone();
    let ban_model_for_refresh = ban_model.clone();
    let chat_model_for_refresh = chat_model.clone();
    let refresh = move || {
        let (Some(window), Some(app)) = (weak_window.upgrade(), weak_app.upgrade()) else {
            return;
        };
        let mut app = app.borrow_mut();
        app.update();
        sync_window(
            &window,
            &app,
            &tree_model_for_refresh,
            &member_model_for_refresh,
            &compact_channel_model_for_refresh,
            &compact_avatar_model_for_refresh,
            &saved_server_model_for_refresh,
            &recent_server_model_for_refresh,
            &ban_model_for_refresh,
            &chat_model_for_refresh,
        );
    };
    refresh();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), refresh);
    let app_for_connect = app.clone();
    let weak_for_connect = window.as_weak();
    window.on_connect(move |address, nickname, remember| {
        let address = address.trim().to_owned();
        let nickname = nickname.trim().to_owned();
        if address.is_empty() || nickname.is_empty() || nickname.chars().count() > 64 {
            return;
        }
        let mut a = app_for_connect.borrow_mut();
        a.dispatch(AppCommand::Connect { server_password: Default::default(),
            address,
            nickname,
            remember,
            auto_connect: None,
        });
        if let Some(w) = weak_for_connect.upgrade() {
            w.set_connecting(true);
            w.set_connection_status("正在连接服务器…".into());
            w.set_connection_error("".into());
        }
    });
    let app_for_recent = app.clone();
    let weak_for_recent = window.as_weak();
    window.on_connect_saved_server(move |index| {
        let profile = app_for_recent
            .borrow()
            .saved_servers()
            .get(index as usize)
            .cloned();
        let Some(profile) = profile else {
            return;
        };
        if let Some(window) = weak_for_recent.upgrade() {
            window.set_server_address(profile.address.clone().into());
            window.set_nickname(profile.nickname.clone().into());
            if profile.nickname.is_empty() {
                window.set_connection_error("请先编辑该服务器的加入昵称。".into());
                return;
            }
            window.set_connecting(true);
            window.set_connection_status("正在连接服务器…".into());
            window.set_connection_error("".into());
        }
        app_for_recent.borrow_mut().dispatch(AppCommand::Connect { server_password: Default::default(),
            address: profile.address,
            nickname: profile.nickname,
            remember: true,
            auto_connect: None,
        });
    });
    let app_for_switch = app.clone();
    let weak_for_switch = window.as_weak();
    window.on_switch_saved_server(move |index| {
        let profile = app_for_switch
            .borrow()
            .saved_servers()
            .get(index as usize)
            .cloned();
        let Some(profile) = profile else {
            return;
        };
        if profile.nickname.is_empty() {
            return;
        }
        let mut app = app_for_switch.borrow_mut();
        app.dispatch(AppCommand::Disconnect);
        app.dispatch(AppCommand::Connect { server_password: Default::default(),
            address: profile.address.clone(),
            nickname: profile.nickname.clone(),
            remember: true,
            auto_connect: None,
        });
        if let Some(window) = weak_for_switch.upgrade() {
            window.set_server_address(profile.address.into());
            window.set_nickname(profile.nickname.into());
            window.set_connecting(true);
            window.set_connection_status("正在切换服务器…".into());
        }
    });
    let app_for_saved_edit = app.clone();
    let weak_for_saved_edit = window.as_weak();
    let saved_model_for_edit = saved_server_model.clone();
    let recent_model_for_edit = recent_server_model.clone();
    window.on_save_saved_server(move |index, name, address, nickname| {
        let profile = SavedServer {
            display_name: name.trim().to_owned(),
            server_name: String::new(),
            address: address.trim().to_owned(),
            nickname: nickname.trim().to_owned(),
        };
        let mut app = app_for_saved_edit.borrow_mut();
        let update_online_name = app.state().connection == ConnectionState::Connected
            && app.server_address() == profile.address;
        if app.update_saved_server(index as usize, profile.clone()) {
            update_model(
                &saved_model_for_edit,
                saved_server_items(app.saved_servers()),
            );
            update_model(
                &recent_model_for_edit,
                recent_server_items(app.saved_servers()),
            );
            if let Some(window) = weak_for_saved_edit.upgrade()
                && app.server_address() == profile.address
            {
                window.set_server_address(profile.address.into());
                window.set_nickname(profile.nickname.clone().into());
            }
            if update_online_name {
                app.dispatch(AppCommand::SetDisplayName(profile.nickname));
            }
        }
    });
    let app_for_disconnect = app.clone();
    let weak_for_disconnect = window.as_weak();
    window.on_disconnect(move || {
        let mut a = app_for_disconnect.borrow_mut();
        a.dispatch(AppCommand::Disconnect);
        if let Some(w) = weak_for_disconnect.upgrade() {
            w.set_connection_status("正在断开连接…".into());
        }
    });
    let app_for_mic = app.clone();
    let weak_for_mic = window.as_weak();
    window.on_toggle_mic(move || {
        let mut a = app_for_mic.borrow_mut();
        if let Some(next) = requested_mute_toggle(a.state()) {
            a.dispatch(AppCommand::SetMuted(next));
        }
        if let Some(w) = weak_for_mic.upgrade() {
            w.set_mic_muted(a.state().muted);
            w.set_mic_admin_muted(a.state().admin_muted);
        }
    });
    let app_for_speaker = app.clone();
    let weak_for_speaker = window.as_weak();
    window.on_toggle_speaker(move || {
        let mut a = app_for_speaker.borrow_mut();
        let next = !a.state().deafened;
        a.dispatch(AppCommand::SetDeafened(next));
        if let Some(w) = weak_for_speaker.upgrade() {
            w.set_speaker_muted(next);
        }
    });
    let app_for_output_device = app.clone();
    let weak_for_output_device = window.as_weak();
    window.on_select_output_device(move |index| {
        let mut app = app_for_output_device.borrow_mut();
        let mut preferences = app.audio_preferences();
        preferences.output_device = device_at(&app.devices().outputs, index);
        app.dispatch(AppCommand::UpdateAudioPreferences(preferences));
        if let Some(window) = weak_for_output_device.upgrade() {
            window.set_output_device_index(index);
            window.set_settings_error(app.error().unwrap_or_default().into());
        }
    });
    let app_for_settings = app.clone();
    let weak_for_settings = window.as_weak();
    let input_model_for_settings = input_device_model.clone();
    let output_model_for_settings = output_device_model.clone();
    let draft_for_settings = shortcut_draft.clone();
    let capture_for_settings = shortcut_capture.clone();
    window.on_open_settings(move || {
        let mut app = app_for_settings.borrow_mut();
        app.dispatch(AppCommand::RefreshDevices);
        app.dispatch(AppCommand::PrepareLocalAudio);
        if let Some(window) = weak_for_settings.upgrade() {
            if window.get_settings_page() == 1 {
                if !window.get_can_set_target_network_policy() {
                    window.set_settings_page(0);
                    window.set_room_network_channel_index(-1);
                    window.set_room_network_channel_name("".into());
                }
            } else {
                window.set_room_network_channel_index(-1);
                window.set_room_network_channel_name("".into());
            }
            sync_audio_settings(
                &window,
                &app,
                &input_model_for_settings,
                &output_model_for_settings,
                true,
            );
            *draft_for_settings.borrow_mut() = app.shortcut_bindings().clone();
            capture_for_settings.borrow_mut().cancel();
            window.set_recording_shortcut(-1);
            sync_shortcut_settings(&window, &draft_for_settings.borrow());
            window.set_show_settings(true);
        }
    });
    let app_for_refresh_devices = app.clone();
    let weak_for_refresh_devices = window.as_weak();
    let input_model_for_refresh_devices = input_device_model.clone();
    let output_model_for_refresh_devices = output_device_model.clone();
    window.on_refresh_devices(move || {
        let mut app = app_for_refresh_devices.borrow_mut();
        app.dispatch(AppCommand::RefreshDevices);
        if let Some(window) = weak_for_refresh_devices.upgrade() {
            sync_audio_settings(
                &window,
                &app,
                &input_model_for_refresh_devices,
                &output_model_for_refresh_devices,
                true,
            );
        }
    });
    let app_for_save_settings = app.clone();
    let weak_for_save_settings = window.as_weak();
    let draft_for_save_settings = shortcut_draft.clone();
    let capture_for_save_settings = shortcut_capture.clone();
    window.on_save_audio_settings(move || {
        let Some(window) = weak_for_save_settings.upgrade() else {
            return;
        };
        let mut app = app_for_save_settings.borrow_mut();
        let current = app.audio_preferences();
        let preferences = audio_preferences_from_window(&window, &app, current);
        app.dispatch(AppCommand::UpdateAudioPreferences(preferences));
        app.dispatch(AppCommand::UpdateShortcuts(
            draft_for_save_settings.borrow().clone(),
        ));
        capture_for_save_settings.borrow_mut().cancel();
        window.set_recording_shortcut(-1);
        window.set_ptt_key(display_binding(&app.shortcut_bindings().push_to_talk).into());
        let room_policy = room_network_policy_at(window.get_room_network_policy_index());
        let target_channel = usize::try_from(window.get_room_network_channel_index())
            .ok()
            .and_then(|index| app.state().channels.get(index))
            .map(|channel| channel.id);
        if let Some(channel_id) = target_channel
            && should_submit_network_policy(window.get_settings_page(), app.state(), channel_id)
        {
            app.dispatch(AppCommand::SetChannelNetworkPolicy {
                channel_id,
                policy: room_policy,
            });
        }
        window.set_settings_error(app.error().unwrap_or_default().into());
        window.set_show_settings(false);
        if matches!(
            app.state().connection,
            ConnectionState::Disconnected | ConnectionState::Error(_)
        ) {
            app.dispatch(AppCommand::StopMicTest);
            app.dispatch(AppCommand::StopLocalAudio);
        }
    });
    let app_for_close_settings = app.clone();
    let weak_for_close_settings = window.as_weak();
    let capture_for_close_settings = shortcut_capture.clone();
    window.on_close_settings(move || {
        let mut app = app_for_close_settings.borrow_mut();
        app.dispatch(AppCommand::StopMicTest);
        if matches!(
            app.state().connection,
            ConnectionState::Disconnected | ConnectionState::Error(_)
        ) {
            app.dispatch(AppCommand::StopLocalAudio);
        }
        if let Some(window) = weak_for_close_settings.upgrade() {
            capture_for_close_settings.borrow_mut().cancel();
            window.set_recording_shortcut(-1);
            window.set_show_settings(false);
        }
    });
    let app_for_output_test = app.clone();
    window.on_play_output_test(move || {
        app_for_output_test
            .borrow_mut()
            .dispatch(AppCommand::PlayOutputTest);
    });
    let app_for_start_mic_test = app.clone();
    window.on_start_mic_test(move || {
        app_for_start_mic_test
            .borrow_mut()
            .dispatch(AppCommand::StartMicTest);
    });
    let app_for_original_test = app.clone();
    window.on_play_mic_original(move || {
        app_for_original_test
            .borrow_mut()
            .dispatch(AppCommand::PlayMicTestOriginal);
    });
    let app_for_processed_test = app.clone();
    window.on_play_mic_processed(move || {
        app_for_processed_test
            .borrow_mut()
            .dispatch(AppCommand::PlayMicTestProcessed);
    });
    let app_for_stop_mic_test = app.clone();
    window.on_stop_mic_test(move || {
        app_for_stop_mic_test
            .borrow_mut()
            .dispatch(AppCommand::StopMicTest);
    });
    let app_for_join = app.clone();
    let weak_for_join = window.as_weak();
    let tree_model_for_join = tree_model.clone();
    let member_model_for_join = member_model.clone();
    let compact_channel_model_for_join = compact_channel_model.clone();
    let compact_avatar_model_for_join = compact_avatar_model.clone();
    let saved_server_model_for_join = saved_server_model.clone();
    let recent_server_model_for_join = recent_server_model.clone();
    let ban_model_for_join = ban_model.clone();
    let chat_model_for_join = chat_model.clone();
    window.on_join_channel(move |index| {
        let mut app = app_for_join.borrow_mut();
        let channel = app
            .state()
            .channels
            .get(index as usize)
            .map(|channel| (channel.id, channel.name.clone()));
        if let Some((channel_id, channel_name)) = channel {
            tracing::info!(channel_id, channel_name, "joining channel from Slint UI");
            app.dispatch(AppCommand::JoinChannel(channel_id));
        } else {
            tracing::warn!(index, "ignored invalid channel index from Slint UI");
        }
        if let Some(window) = weak_for_join.upgrade() {
            sync_window(
                &window,
                &app,
                &tree_model_for_join,
                &member_model_for_join,
                &compact_channel_model_for_join,
                &compact_avatar_model_for_join,
                &saved_server_model_for_join,
                &recent_server_model_for_join,
                &ban_model_for_join,
                &chat_model_for_join,
            );
        }
    });
    let app_for_user_mute = app.clone();
    let app_for_user_volume = app.clone();
    window.on_set_user_volume(move |session_id, volume| {
        let Ok(session_id) = session_id.parse::<u64>() else {
            tracing::warn!(session_id = %session_id, "ignored invalid user session from Slint UI");
            return;
        };
        app_for_user_volume
            .borrow_mut()
            .dispatch(AppCommand::SetUserVolume { session_id, volume });
    });
    window.on_set_user_local_mute(move |session_id, muted| {
        let Ok(session_id) = session_id.parse::<u64>() else {
            tracing::warn!(session_id = %session_id, "ignored invalid user session from Slint UI");
            return;
        };
        app_for_user_mute
            .borrow_mut()
            .dispatch(AppCommand::SetUserLocalMute { session_id, muted });
    });
    let app_for_admin_mute = app.clone();
    window.on_admin_mute_user(move |session_id, enabled| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_admin_mute
                .borrow_mut()
                .dispatch(AppCommand::AdminMute {
                    session_id,
                    enabled,
                });
        }
    });
    let app_for_kick_channel = app.clone();
    window.on_kick_user_channel(move |session_id| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_kick_channel
                .borrow_mut()
                .dispatch(AppCommand::KickChannel(session_id));
        }
    });
    let app_for_kick_server = app.clone();
    window.on_kick_user_server(move |session_id| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_kick_server
                .borrow_mut()
                .dispatch(AppCommand::KickServer(session_id));
        }
    });
    let app_for_ban = app.clone();
    window.on_ban_user(move |session_id| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_ban.borrow_mut().dispatch(AppCommand::BanUser {
                session_id,
                reason: "Banned by administrator".into(),
            });
        }
    });
    let app_for_request_bans = app.clone();
    window.on_request_bans(move |page| {
        app_for_request_bans
            .borrow_mut()
            .dispatch(AppCommand::ListBans {
                offset: page.max(0) as usize * 8,
                limit: 8,
            });
    });
    let app_for_unban = app.clone();
    window.on_unban_client(move |client_id, page| {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            return;
        }
        let mut app = app_for_unban.borrow_mut();
        app.dispatch(AppCommand::Unban(client_id.to_owned()));
        app.dispatch(AppCommand::ListBans {
            offset: page.max(0) as usize * 8,
            limit: 8,
        });
    });
    let app_for_move_user = app.clone();
    window.on_move_user_to_current_channel(move |session_id| {
        let Ok(session_id) = session_id.parse::<u64>() else {
            return;
        };
        let mut app = app_for_move_user.borrow_mut();
        let channel_id = app
            .state()
            .self_session
            .and_then(|id| app.state().users.get(&id))
            .map(|user| user.channel_id)
            .unwrap_or(0);
        if channel_id != 0 {
            app.dispatch(AppCommand::MoveUser {
                session_id,
                channel_id,
            });
        }
    });
    let app_for_channel_admin = app.clone();
    window.on_set_user_channel_admin(move |session_id, enabled| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_channel_admin
                .borrow_mut()
                .dispatch(AppCommand::SetChannelAdmin {
                    session_id,
                    enabled,
                });
        }
    });
    let app_for_server_admin = app.clone();
    window.on_set_user_server_admin(move |session_id, enabled| {
        if let Ok(session_id) = session_id.parse::<u64>() {
            app_for_server_admin
                .borrow_mut()
                .dispatch(AppCommand::SetServerAdmin {
                    session_id,
                    enabled,
                });
        }
    });
    let app_for_delete_channel = app.clone();
    window.on_delete_channel(move |index| {
        let mut app = app_for_delete_channel.borrow_mut();
        if let Some(channel_id) = app
            .state()
            .channels
            .get(index as usize)
            .map(|channel| channel.id)
        {
            app.dispatch(AppCommand::DeleteChannel(channel_id));
        }
    });
    let app_for_trust = app.clone();
    window.on_trust_server(move |endpoint, fingerprint| {
        app_for_trust
            .borrow_mut()
            .dispatch(AppCommand::TrustServer {
                endpoint: endpoint.to_string(),
                fingerprint: fingerprint.to_string(),
            });
    });
    let app_for_master_provision = app.clone();
    window.on_provision_master(move |token| {
        let token = token.trim();
        if !token.is_empty() {
            app_for_master_provision
                .borrow_mut()
                .dispatch(AppCommand::ProvisionMaster(token.to_owned()));
        }
    });
    let app_for_channel_editor = app.clone();
    window.on_save_channel_editor(move |mode, index, name, user_limit| {
        let mut app = app_for_channel_editor.borrow_mut();
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let user_limit = user_limit.trim();
        let user_limit = if user_limit.is_empty() {
            0
        } else if let Ok(value @ 0..=10_000) = user_limit.parse::<u32>() {
            value
        } else {
            tracing::warn!(user_limit = %user_limit, "ignored invalid channel user limit");
            return;
        };
        let existing = app.state().channels.get(index as usize).cloned();
        match (mode, existing) {
            (3, _) => {
                let sort_order = app
                    .state()
                    .channels
                    .iter()
                    .filter(|channel| channel.parent_id == 0)
                    .map(|channel| channel.sort_order)
                    .max()
                    .unwrap_or(0)
                    + 1;
                app.dispatch(AppCommand::CreateChannel(
                    crate::protocol::control::Channel {
                        id: 0,
                        parent_id: 0,
                        name,
                        sort_order,
                        user_limit,
                        network_policy: "auto".into(),
                        audio_quality_policy: "auto".into(),
                        temporary: false,
                    },
                ));
            }
            (1, Some(parent)) => app.dispatch(AppCommand::CreateChannel(
                crate::protocol::control::Channel {
                    id: 0,
                    parent_id: parent.id,
                    name,
                    sort_order: parent.sort_order + 1,
                    user_limit,
                    network_policy: "auto".into(),
                    audio_quality_policy: "auto".into(),
                    temporary: false,
                },
            )),
            (2, Some(mut channel)) => {
                channel.name = name;
                channel.user_limit = user_limit;
                app.dispatch(AppCommand::EditChannel(channel));
            }
            _ => tracing::warn!(mode, index, "ignored invalid channel editor request"),
        }
    });
    let app_for_channel_mute = app.clone();
    window.on_mute_channel(move |index| {
        let mut app = app_for_channel_mute.borrow_mut();
        let Some(channel_id) = app
            .state()
            .channels
            .get(index as usize)
            .map(|channel| channel.id)
        else {
            tracing::warn!(index, "ignored invalid channel mute index from Slint UI");
            return;
        };
        let own_session = app.state().self_session;
        let sessions = app
            .state()
            .users
            .values()
            .filter(|user| user.channel_id == channel_id && Some(user.session_id) != own_session)
            .map(|user| user.session_id)
            .collect::<Vec<_>>();
        for session_id in sessions {
            app.dispatch(AppCommand::SetUserLocalMute {
                session_id,
                muted: true,
            });
        }
    });
    window.run()
}

#[cfg(target_os = "macos")]
fn ui_font_family() -> &'static str {
    "SF Pro Text"
}

#[cfg(target_os = "windows")]
fn ui_font_family() -> &'static str {
    "Segoe UI"
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn ui_font_family() -> &'static str {
    "Noto Sans"
}

#[cfg(target_os = "macos")]
pub fn install_native_platform() -> Result<(), slint::PlatformError> {
    use winit::platform::macos::WindowAttributesExtMacOS;

    let backend = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attributes| {
            attributes
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
        })
        .build()?;
    slint::platform::set_platform(Box::new(backend))
        .map_err(|error| slint::PlatformError::Other(error.to_string()))
}

#[cfg(not(target_os = "macos"))]
pub fn install_native_platform() -> Result<(), slint::PlatformError> {
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AdministrationUiCapabilities {
    create_channel: bool,
    edit_channel: bool,
    delete_channel: bool,
    mute_member: bool,
    kick_member_channel: bool,
    kick_member_server: bool,
}

fn administration_ui_capabilities(actions: &[String]) -> AdministrationUiCapabilities {
    let has = |action: &str| actions.iter().any(|value| value == action);
    AdministrationUiCapabilities {
        create_channel: has("channel.create"),
        edit_channel: has("channel.edit"),
        delete_channel: has("channel.delete"),
        mute_member: has("member.mute"),
        kick_member_channel: has("member.kick_channel"),
        kick_member_server: has("member.kick_server"),
    }
}

fn channel_tree_order(channels: &[protocol::control::Channel]) -> Vec<(usize, i32)> {
    fn append_subtree(
        root: usize,
        channels: &[protocol::control::Channel],
        visited: &mut [bool],
        ordered: &mut Vec<(usize, i32)>,
    ) {
        let mut stack = vec![(root, 0_i32)];
        while let Some((index, depth)) = stack.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            ordered.push((index, depth.min(6)));
            let parent_id = channels[index].id;
            let mut children = (0..channels.len())
                .filter(|&child_index| {
                    channels[child_index].parent_id == parent_id && !visited[child_index]
                })
                .collect::<Vec<_>>();
            children.sort_by_key(|&child_index| {
                (channels[child_index].sort_order, channels[child_index].id)
            });
            for child_index in children.into_iter().rev() {
                stack.push((child_index, depth.saturating_add(1)));
            }
        }
    }

    let mut visited = vec![false; channels.len()];
    let mut ordered = Vec::with_capacity(channels.len());
    let mut roots = channels
        .iter()
        .enumerate()
        .filter(|(_, channel)| {
            channel.parent_id == 0
                || !channels
                    .iter()
                    .any(|candidate| candidate.id == channel.parent_id)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    roots.sort_by_key(|&index| (channels[index].sort_order, channels[index].id));
    for root in roots {
        append_subtree(root, channels, &mut visited, &mut ordered);
    }
    // A malformed snapshot may contain a cycle. Keep every channel visible without
    // recursing forever; the server still rejects cycles at the authorization boundary.
    let mut remaining = (0..channels.len()).collect::<Vec<_>>();
    remaining.sort_by_key(|&index| (channels[index].sort_order, channels[index].id));
    for index in remaining {
        if !visited[index] {
            append_subtree(index, channels, &mut visited, &mut ordered);
        }
    }
    ordered
}

#[allow(clippy::too_many_arguments)]
fn sync_window(
    window: &AppWindow,
    app: &Application,
    tree_model: &VecModel<TreeItem>,
    member_model: &VecModel<MemberItem>,
    compact_channel_model: &VecModel<CompactChannelItem>,
    compact_avatar_model: &VecModel<MemberItem>,
    saved_server_model: &VecModel<SavedServerItem>,
    recent_server_model: &VecModel<SavedServerItem>,
    ban_model: &VecModel<BanItem>,
    chat_model: &VecModel<ChatItem>,
) {
    let state = app.state();
    let administration_capabilities = administration_ui_capabilities(&state.capabilities);
    window.set_can_create_channels(administration_capabilities.create_channel);
    window.set_can_edit_channels(administration_capabilities.edit_channel);
    window.set_can_delete_channels(administration_capabilities.delete_channel);
    window.set_can_admin_mute_users(administration_capabilities.mute_member);
    window.set_can_kick_channel_users(administration_capabilities.kick_member_channel);
    window.set_can_kick_server_users(administration_capabilities.kick_member_server);
    window.set_can_ban_users(state.capabilities.iter().any(|value| value == "member.ban"));
    window.set_can_unban_users(
        state
            .capabilities
            .iter()
            .any(|value| value == "member.unban")
            && state
                .capabilities
                .iter()
                .any(|value| value == "member.ban_list"),
    );
    update_model(ban_model, ban_items(&state.bans));
    window.set_ban_total(state.ban_list_total as i32);
    window.set_ban_page((state.ban_list_offset / 8) as i32);
    window.set_can_move_users(
        state
            .capabilities
            .iter()
            .any(|value| value == "member.move"),
    );
    window.set_can_set_server_roles(
        state
            .capabilities
            .iter()
            .any(|value| value == "server.role.set"),
    );
    window.set_can_set_channel_roles(
        state
            .capabilities
            .iter()
            .any(|value| value == "channel.role.set"),
    );
    let channel_administration_access = state
        .channels
        .iter()
        .map(|channel| can_set_network_policy(state, channel.id))
        .collect::<Vec<_>>();
    window.set_network_policy_channel_access(ModelRc::new(VecModel::from(
        channel_administration_access.clone(),
    )));
    window.set_channel_administration_access(ModelRc::new(VecModel::from(
        channel_administration_access,
    )));
    window.set_server_role(state.server_role.clone().into());
    window.set_self_session_id(
        state
            .self_session
            .map(|session_id| session_id.to_string())
            .unwrap_or_default()
            .into(),
    );
    window.set_trust_endpoint(state.trust_endpoint.clone().into());
    window.set_trust_fingerprint(state.trust_fingerprint.clone().into());
    match &state.connection {
        ConnectionState::Disconnected => {
            window.set_show_main(false);
            window.set_connecting(false);
            window.set_connection_status("".into());
            window.set_connection_error(app.error().unwrap_or_default().into());
        }
        ConnectionState::Connecting => {
            window.set_show_main(false);
            window.set_connecting(true);
            window.set_connection_status("正在连接服务器…".into());
            window.set_connection_error("".into());
        }
        ConnectionState::Connected => {
            window.set_show_main(true);
            window.set_connecting(false);
            window.set_connection_status("".into());
            window.set_connection_error(app.error().unwrap_or_default().into());
        }
        ConnectionState::Reconnecting => {
            window.set_show_main(true);
            window.set_connecting(true);
            window.set_connection_status("连接中断，正在重连…".into());
            window.set_connection_error(app.error().unwrap_or_default().into());
        }
        ConnectionState::Error(error) => {
            window.set_show_main(false);
            window.set_connecting(false);
            window.set_connection_status("".into());
            window.set_connection_error(error.clone().into());
        }
    }
    let own_channel = state
        .self_session
        .and_then(|id| state.users.get(&id))
        .map(|u| u.channel_id)
        .unwrap_or(0);
    window.set_own_channel_index(
        state
            .channels
            .iter()
            .position(|channel| channel.id == own_channel)
            .map(|index| index as i32)
            .unwrap_or(-1),
    );
    let channel_order = channel_tree_order(&state.channels);
    let mut tree = Vec::new();
    for &(channel_index, depth) in &channel_order {
        let channel = &state.channels[channel_index];
        let users: Vec<_> = state
            .users
            .values()
            .filter(|user| user.channel_id == channel.id)
            .collect();
        let remote_sessions = users
            .iter()
            .filter(|user| Some(user.session_id) != state.self_session)
            .map(|user| user.session_id)
            .collect::<Vec<_>>();
        let channel_locally_muted = !remote_sessions.is_empty()
            && remote_sessions.iter().all(|session_id| {
                state
                    .locally_muted_users
                    .get(session_id)
                    .copied()
                    .unwrap_or(false)
            });
        tree.push(TreeItem {
            label: channel.name.clone().into(),
            detail: "".into(),
            count: users.len().to_string().into(),
            avatar: "".into(),
            avatar_image: Default::default(),
            custom_avatar: false,
            channel_index: channel_index as i32,
            session_id: "".into(),
            user_limit: channel.user_limit.to_string().into(),
            depth,
            is_channel: true,
            selected: channel.id == own_channel,
            speaking: false,
            muted: channel_locally_muted,
            admin_muted: false,
            server_muted: false,
            channel_muted: false,
            deafened: false,
            locally_muted: false,
            volume: 1.0,
            server_role: "".into(),
            channel_role: "".into(),
        });
        for user in users {
            tree.push(TreeItem {
                label: user.nickname.clone().into(),
                detail: channel.name.clone().into(),
                count: "".into(),
                avatar: user_avatar(user),
                avatar_image: image_for_avatar(&user.avatar),
                custom_avatar: is_custom_avatar(&user.avatar),
                channel_index: channel_index as i32,
                session_id: user.session_id.to_string().into(),
                user_limit: "".into(),
                depth,
                is_channel: false,
                selected: false,
                speaking: state.is_talking(user.session_id),
                muted: user.muted,
                admin_muted: user.admin_muted,
                server_muted: user.server_muted,
                channel_muted: user.channel_muted,
                deafened: user.deafened,
                locally_muted: state
                    .locally_muted_users
                    .get(&user.session_id)
                    .copied()
                    .unwrap_or(false),
                volume: state.user_volume(user.session_id),
                server_role: user.server_role.clone().into(),
                channel_role: user.channel_role.clone().into(),
            });
        }
    }
    let members: Vec<_> = state
        .users
        .values()
        .filter(|u| u.channel_id == own_channel && own_channel != 0)
        .map(|u| MemberItem {
            name: u.nickname.clone().into(),
            avatar: user_avatar(u),
            avatar_image: image_for_avatar(&u.avatar),
            custom_avatar: is_custom_avatar(&u.avatar),
            session_id: u.session_id.to_string().into(),
            speaking: state.is_talking(u.session_id),
            muted: u.muted,
            admin_muted: u.admin_muted,
            server_muted: u.server_muted,
            channel_muted: u.channel_muted,
            deafened: u.deafened,
            locally_muted: state
                .locally_muted_users
                .get(&u.session_id)
                .copied()
                .unwrap_or(false),
            volume: state.user_volume(u.session_id),
            server_role: u.server_role.clone().into(),
            channel_role: u.channel_role.clone().into(),
        })
        .collect();
    let compact_channels = channel_order
        .iter()
        .map(|&(channel_index, depth)| {
            let channel = &state.channels[channel_index];
            CompactChannelItem {
                label: channel.name.as_str().into(),
                count: state
                    .users
                    .values()
                    .filter(|user| user.channel_id == channel.id)
                    .count()
                    .to_string()
                    .into(),
                channel_index: channel_index as i32,
                depth,
                selected: channel.id == own_channel,
            }
        })
        .collect();
    let (compact_avatars, compact_member_overflow) = compact_member_preview(&members);
    // Keep one stable model for the lifetime of the window. Replacing it every refresh destroys
    // repeated row instances (including their TouchAreas) between mouse press and release.
    update_model(tree_model, tree);
    update_model(member_model, members);
    update_model(compact_channel_model, compact_channels);
    update_model(compact_avatar_model, compact_avatars);
    let chat_items = state
        .chat_messages
        .iter()
        .filter(|message| message.channel_id == own_channel && own_channel != 0)
        .map(|message| ChatItem {
            name: message.sender_name.as_str().into(),
            avatar: if message.sender_avatar.is_empty() {
                initials(&message.sender_name)
            } else {
                message.sender_avatar.as_str().into()
            },
            avatar_image: image_for_avatar(&message.sender_avatar),
            custom_avatar: is_custom_avatar(&message.sender_avatar),
            time: chat_time_label(message.sent_at_unix_ms).into(),
            text: message.text.as_str().into(),
            own: state.self_session == Some(message.sender_session_id),
        })
        .collect();
    update_model(chat_model, chat_items);
    let own_user = state
        .self_session
        .and_then(|session_id| state.users.get(&session_id));
    let own_name = own_user.map_or(app.nickname(), |user| user.nickname.as_str());
    window.set_self_initials(initials(own_name));
    let own_avatar = own_user.map_or(app.avatar(), |user| user.avatar.as_str());
    window.set_self_avatar(own_user.map(user_avatar).unwrap_or_else(|| {
        if app.avatar().is_empty() {
            initials(own_name)
        } else {
            app.avatar().into()
        }
    }));
    window.set_self_custom_avatar(is_custom_avatar(own_avatar));
    window.set_self_avatar_image(image_for_avatar(own_avatar));
    window.set_compact_member_overflow(compact_member_overflow);
    update_model(saved_server_model, saved_server_items(app.saved_servers()));
    update_model(
        recent_server_model,
        recent_server_items(app.saved_servers()),
    );
    window.set_topology(match &state.connection {
        ConnectionState::Reconnecting => "CONNECTING".into(),
        ConnectionState::Error(_) => "NETWORK ERROR".into(),
        _ => topology_label(&state.actual_transport),
    });
    window.set_network_quality(state.network_quality.clone().into());
    window.set_latency(
        state
            .rtt_ms
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "--ms".into())
            .into(),
    );
    window.set_control_latency(
        state
            .control_rtt_ms
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "--ms".into())
            .into(),
    );
    window.set_server_name(if state.server_name.is_empty() {
        app.server_address().into()
    } else {
        state.server_name.clone().into()
    });
    window.set_channel_name(
        state
            .channels
            .iter()
            .find(|channel| channel.id == own_channel)
            .map(|channel| channel.name.clone().into())
            .unwrap_or_else(|| "未加入频道".into()),
    );
    window.set_output_device(
        state
            .active_output_device
            .clone()
            .unwrap_or_else(|| "系统默认输出".into())
            .into(),
    );
    window.set_codec_label(
        state
            .active_output_sample_rate
            .map(|rate| format!("OPUS  {}kHz", rate / 1_000))
            .unwrap_or_else(|| "OPUS  48kHz".into())
            .into(),
    );
    window.set_mic_muted(state.muted);
    window.set_mic_admin_muted(state.admin_muted);
    window.set_speaker_muted(state.deafened);
    window.set_input_level(state.input_level.clamp(0.0, 1.0));
    window.set_mic_test_status(mic_test_label(state.mic_test_status).into());
    window.set_audio_status(audio_state_label(&state.audio_state).into());
    let settings_error = state
        .device_error
        .as_deref()
        .or(state.dsp_error.as_deref())
        .or(app.global_ptt_error())
        .unwrap_or_default();
    window.set_settings_error(settings_error.into());
    let packet_total = state.packets_received.saturating_add(state.packets_lost);
    window.set_packet_loss_label(if packet_total == 0 {
        "--".into()
    } else {
        format!(
            "{:.1}%",
            state.packets_lost as f64 * 100.0 / packet_total as f64
        )
        .into()
    });
    window.set_jitter_label(if state.packets_received == 0 {
        "--".into()
    } else {
        format!("{}ms", state.jitter_depth.saturating_mul(20)).into()
    });
    window.set_uplink_label(if state.opus_bitrate_bps > 0 {
        format!("{} kbps", state.opus_bitrate_bps / 1_000).into()
    } else {
        "--".into()
    });
    window.set_downlink_label("--".into());
    window.set_connection_duration_label("--".into());
    window.set_input_device(
        state
            .active_input_device
            .clone()
            .unwrap_or_else(|| "系统默认麦克风".into())
            .into(),
    );
    window.set_input_sample_rate_label(sample_rate_label(state.active_input_sample_rate).into());
    window.set_output_sample_rate_label(sample_rate_label(state.active_output_sample_rate).into());
    window.set_dsp_label(
        state
            .dsp_loading
            .map(|mode| format!("正在加载 {}", noise_mode_label(mode)))
            .unwrap_or_else(|| noise_mode_label(state.active_noise_suppression).into())
            .into(),
    );
    window.set_aec_label(if state.active_echo_cancellation {
        "已开启".into()
    } else {
        "已关闭".into()
    });
    window.set_agc_label(if state.active_automatic_gain {
        "已开启".into()
    } else {
        "已关闭".into()
    });
}

fn sync_audio_settings(
    window: &AppWindow,
    app: &Application,
    input_model: &VecModel<SharedString>,
    output_model: &VecModel<SharedString>,
    reset_controls: bool,
) {
    let inputs = device_options(&app.devices().inputs, "系统默认麦克风");
    let outputs = device_options(&app.devices().outputs, "系统默认扬声器");
    update_model(input_model, inputs);
    update_model(output_model, outputs);
    if !reset_controls {
        return;
    }
    let preferences = app.audio_preferences();
    window.set_input_device_index(device_index(
        &app.devices().inputs,
        preferences.input_device.as_deref(),
    ));
    window.set_output_device_index(device_index(
        &app.devices().outputs,
        preferences.output_device.as_deref(),
    ));
    window.set_input_mode_index(match preferences.input_mode {
        InputMode::VoiceActivity => 0,
        InputMode::PushToTalk => 1,
    });
    window.set_noise_mode_index(match preferences.noise_suppression_mode {
        NoiseSuppressionMode::Off => 0,
        NoiseSuppressionMode::Standard => 1,
        NoiseSuppressionMode::RnNoise => 2,
        NoiseSuppressionMode::DeepFilterNet => 3,
        NoiseSuppressionMode::DeepFilterNetStrong => 4,
    });
    window.set_noise_strength_index(match preferences.noise_suppression_strength {
        NoiseSuppressionStrength::Low => 0,
        NoiseSuppressionStrength::Medium => 1,
        NoiseSuppressionStrength::High => 2,
    });
    window.set_input_gain(preferences.input_gain);
    window.set_output_volume(preferences.output_volume);
    window.set_echo_cancellation(preferences.echo_cancellation);
    window.set_automatic_gain(preferences.automatic_gain);
    window.set_ptt_key(display_binding(&app.shortcut_bindings().push_to_talk).into());
    let room_network_policy = usize::try_from(window.get_room_network_channel_index())
        .ok()
        .and_then(|index| app.state().channels.get(index))
        .map(|channel| room_network_policy_from_wire(&channel.network_policy))
        .unwrap_or_else(|| app.room_network_policy());
    window.set_room_network_policy_index(room_network_policy_index(room_network_policy));
}

fn audio_preferences_from_window(
    window: &AppWindow,
    app: &Application,
    _current: AudioPreferences,
) -> AudioPreferences {
    AudioPreferences {
        input_device: device_at(&app.devices().inputs, window.get_input_device_index()),
        output_device: device_at(&app.devices().outputs, window.get_output_device_index()),
        input_gain: window.get_input_gain().clamp(0.0, 3.0),
        output_volume: window.get_output_volume().clamp(0.01, 3.0),
        input_mode: if window.get_input_mode_index() == 1 {
            InputMode::PushToTalk
        } else {
            InputMode::VoiceActivity
        },
        echo_cancellation: window.get_echo_cancellation(),
        automatic_gain: window.get_automatic_gain(),
        noise_suppression_mode: match window.get_noise_mode_index() {
            0 => NoiseSuppressionMode::Off,
            2 => NoiseSuppressionMode::RnNoise,
            3 => NoiseSuppressionMode::DeepFilterNet,
            4 => NoiseSuppressionMode::DeepFilterNetStrong,
            _ => NoiseSuppressionMode::Standard,
        },
        noise_suppression_strength: match window.get_noise_strength_index() {
            0 => NoiseSuppressionStrength::Low,
            2 => NoiseSuppressionStrength::High,
            _ => NoiseSuppressionStrength::Medium,
        },
    }
}

fn set_shortcut_binding(bindings: &mut ShortcutBindings, action: i32, value: String) {
    match action {
        0 => bindings.toggle_window = value,
        1 => bindings.toggle_mic = value,
        2 => bindings.toggle_speaker = value,
        3 => bindings.push_to_talk = value,
        4 => bindings.toggle_members = value,
        _ => {}
    }
}

fn matching_shortcut_action(bindings: &ShortcutBindings, binding: &str) -> i32 {
    [
        &bindings.toggle_window,
        &bindings.toggle_mic,
        &bindings.toggle_speaker,
        &bindings.push_to_talk,
        &bindings.toggle_members,
    ]
    .into_iter()
    .position(|candidate| !candidate.is_empty() && candidate == binding)
    .map_or(0, |index| index as i32 + 1)
}

fn sync_shortcut_settings(window: &AppWindow, bindings: &ShortcutBindings) {
    window.set_shortcut_toggle_window(display_binding(&bindings.toggle_window).into());
    window.set_shortcut_toggle_mic(display_binding(&bindings.toggle_mic).into());
    window.set_shortcut_toggle_speaker(display_binding(&bindings.toggle_speaker).into());
    window.set_shortcut_push_to_talk(display_binding(&bindings.push_to_talk).into());
    window.set_shortcut_toggle_members(display_binding(&bindings.toggle_members).into());
    window.set_ptt_key(display_binding(&bindings.push_to_talk).into());
}

fn device_options(devices: &[String], default_label: &str) -> Vec<SharedString> {
    std::iter::once(SharedString::from(default_label))
        .chain(devices.iter().map(SharedString::from))
        .collect()
}

fn device_index(devices: &[String], selected: Option<&str>) -> i32 {
    selected
        .and_then(|selected| devices.iter().position(|device| device == selected))
        .map(|index| index as i32 + 1)
        .unwrap_or(0)
}

fn device_at(devices: &[String], index: i32) -> Option<String> {
    usize::try_from(index - 1)
        .ok()
        .and_then(|index| devices.get(index))
        .cloned()
}

fn room_network_policy_index(policy: RoomNetworkPolicy) -> i32 {
    match policy {
        RoomNetworkPolicy::Auto => 0,
        RoomNetworkPolicy::ForceSfu => 1,
        RoomNetworkPolicy::ForceP2p => 2,
    }
}

fn room_network_policy_at(index: i32) -> RoomNetworkPolicy {
    match index {
        1 => RoomNetworkPolicy::ForceSfu,
        2 => RoomNetworkPolicy::ForceP2p,
        _ => RoomNetworkPolicy::Auto,
    }
}

fn room_network_policy_from_wire(policy: &str) -> RoomNetworkPolicy {
    match policy {
        "force_sfu" => RoomNetworkPolicy::ForceSfu,
        "force_p2p" => RoomNetworkPolicy::ForceP2p,
        _ => RoomNetworkPolicy::Auto,
    }
}

fn mic_test_label(status: MicTestStatus) -> &'static str {
    match status {
        MicTestStatus::Idle => "空闲",
        MicTestStatus::Recording => "录音中",
        MicTestStatus::PreviewReady => "可试听",
        MicTestStatus::PlayingOriginal => "播放原声",
        MicTestStatus::PlayingProcessed => "播放处理后",
    }
}

fn audio_state_label(state: &AudioState) -> &'static str {
    match state {
        AudioState::Stopped => "音频未启动",
        AudioState::Starting => "正在启动…",
        AudioState::Ready => "音频就绪",
        AudioState::Error(_) => "音频错误",
    }
}

fn noise_mode_label(mode: NoiseSuppressionMode) -> &'static str {
    match mode {
        NoiseSuppressionMode::Off => "关闭",
        NoiseSuppressionMode::Standard => "标准降噪（WebRTC APM）",
        NoiseSuppressionMode::RnNoise => "RNNoise（自动）",
        NoiseSuppressionMode::DeepFilterNet => "DeepFilterNet · 自然",
        NoiseSuppressionMode::DeepFilterNetStrong => "DeepFilterNet · 强力",
    }
}

fn sample_rate_label(sample_rate: Option<u32>) -> String {
    sample_rate
        .map(|rate| {
            if rate % 1_000 == 0 {
                format!("{} kHz", rate / 1_000)
            } else {
                format!("{:.1} kHz", rate as f64 / 1_000.0)
            }
        })
        .unwrap_or_else(|| "自动".into())
}

fn saved_server_items(servers: &[SavedServer]) -> Vec<SavedServerItem> {
    servers
        .iter()
        .map(|server| {
            let name = if !server.display_name.trim().is_empty() {
                server.display_name.as_str()
            } else if server.server_name.trim().is_empty() {
                server.address.as_str()
            } else {
                server.server_name.as_str()
            };
            SavedServerItem {
                name: name.into(),
                address: server.address.as_str().into(),
                nickname: server.nickname.as_str().into(),
                avatar: initials(name),
                avatar_image: Default::default(),
                custom_avatar: false,
            }
        })
        .collect()
}

fn recent_server_items(servers: &[SavedServer]) -> Vec<SavedServerItem> {
    // The reference login window reserves exactly two compact history rows.
    // Keep the complete saved-server list elsewhere, while preventing a third
    // recent row from being clipped by the 450 px connection window.
    saved_server_items(servers).into_iter().take(2).collect()
}

fn ban_items(bans: &[crate::protocol::control::BanRecord]) -> Vec<BanItem> {
    bans.iter()
        .map(|ban| BanItem {
            client_id: ban.client_id.as_str().into(),
            display_name: ban.display_name.as_str().into(),
            reason: ban.reason.as_str().into(),
            created_at: ban.created_at.as_str().into(),
        })
        .collect()
}

fn compact_member_preview(members: &[MemberItem]) -> (Vec<MemberItem>, i32) {
    (
        members.iter().take(4).cloned().collect(),
        members.len().saturating_sub(4) as i32,
    )
}

fn initials(name: &str) -> SharedString {
    name.chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".into())
        .into()
}

fn user_avatar(user: &crate::protocol::control::User) -> SharedString {
    if user.avatar.is_empty() {
        initials(&user.nickname)
    } else {
        user.avatar.as_str().into()
    }
}

fn chat_time_label(sent_at_unix_ms: i64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let age_minutes = now_ms.saturating_sub(sent_at_unix_ms) / 60_000;
    match age_minutes {
        0 => "刚刚".into(),
        1..=59 => format!("{age_minutes} 分钟前"),
        60..=1439 => format!("{} 小时前", age_minutes / 60),
        _ => format!("{} 天前", age_minutes / 1440),
    }
}

fn topology_label(actual_transport: &str) -> SharedString {
    match actual_transport {
        "Hybrid" | "HYBRID" => "HYBRID",
        "Relay" => "SFU",
        "P2P" => "P2P",
        "SFU" => "SFU",
        other => other,
    }
    .into()
}

fn update_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, expected: Vec<T>) {
    let common_rows = model.row_count().min(expected.len());
    for (row, item) in expected.iter().take(common_rows).enumerate() {
        if model.row_data(row).as_ref() != Some(item) {
            model.set_row_data(row, item.clone());
        }
    }
    while model.row_count() > expected.len() {
        model.remove(model.row_count() - 1);
    }
    for item in expected.into_iter().skip(common_rows) {
        model.push(item);
    }
}

fn can_set_network_policy(state: &crate::model::AppState, channel_id: u64) -> bool {
    state.network_policy_channel_ids.contains(&channel_id)
        || (matches!(state.server_role.as_str(), "MASTER" | "ADMIN")
            && state
                .capabilities
                .iter()
                .any(|value| value == NETWORK_POLICY_CAPABILITY))
}

fn should_submit_network_policy(
    settings_page: i32,
    state: &crate::model::AppState,
    channel_id: u64,
) -> bool {
    settings_page == 1 && can_set_network_policy(state, channel_id)
}

fn requested_mute_toggle(state: &crate::model::AppState) -> Option<bool> {
    (!state.admin_muted).then_some(!state.muted)
}

#[cfg(test)]
mod ui_adapter_tests {
    use super::*;

    fn test_channel(id: u64, parent_id: u64, sort_order: i32) -> protocol::control::Channel {
        protocol::control::Channel {
            id,
            parent_id,
            name: format!("Channel {id}"),
            sort_order,
            user_limit: 0,
            network_policy: "auto".into(),
            audio_quality_policy: "auto".into(),
            temporary: false,
        }
    }

    #[test]
    fn channel_tree_places_children_after_their_parent_with_visible_depth() {
        let channels = vec![
            test_channel(12, 10, 20),
            test_channel(20, 0, 20),
            test_channel(13, 11, 0),
            test_channel(10, 0, 10),
            test_channel(11, 10, 10),
        ];
        let ordered = channel_tree_order(&channels)
            .into_iter()
            .map(|(index, depth)| (channels[index].id, depth))
            .collect::<Vec<_>>();
        assert_eq!(ordered, [(10, 0), (11, 1), (13, 2), (12, 1), (20, 0)]);
    }

    #[test]
    fn malformed_channel_cycles_remain_visible_and_bounded() {
        let channels = vec![test_channel(1, 2, 0), test_channel(2, 1, 0)];
        let ordered = channel_tree_order(&channels);
        assert_eq!(ordered.len(), 2);
        assert!(ordered.iter().all(|(_, depth)| *depth <= 6));
    }

    #[test]
    fn network_policy_capability_matches_server_wire_name() {
        assert_eq!(NETWORK_POLICY_CAPABILITY, "network.policy.set");
    }

    #[test]
    fn administration_ui_capabilities_do_not_leak_between_actions() {
        let channel_admin = administration_ui_capabilities(&[
            "channel.edit".into(),
            "member.mute".into(),
            "member.kick_channel".into(),
        ]);
        assert_eq!(
            channel_admin,
            AdministrationUiCapabilities {
                edit_channel: true,
                mute_member: true,
                kick_member_channel: true,
                ..Default::default()
            }
        );

        let creator = administration_ui_capabilities(&["channel.create".into()]);
        assert_eq!(
            creator,
            AdministrationUiCapabilities {
                create_channel: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn network_policy_access_is_scoped_to_the_target_channel() {
        let mut state = crate::model::AppState {
            network_policy_channel_ids: vec![7],
            ..Default::default()
        };
        assert!(can_set_network_policy(&state, 7));
        assert!(!can_set_network_policy(&state, 8));

        state.server_role = "ADMIN".into();
        state.capabilities = vec![NETWORK_POLICY_CAPABILITY.into()];
        assert!(can_set_network_policy(&state, 8));
    }

    #[test]
    fn audio_settings_never_submit_a_room_network_policy() {
        let state = crate::model::AppState {
            network_policy_channel_ids: vec![7],
            ..Default::default()
        };
        assert!(!should_submit_network_policy(0, &state, 7));
        assert!(!should_submit_network_policy(2, &state, 7));
        assert!(should_submit_network_policy(1, &state, 7));
    }

    #[test]
    fn shortcut_dispatch_uses_only_explicit_user_bindings() {
        let mut bindings = ShortcutBindings::default();
        assert_eq!(matching_shortcut_action(&bindings, "Control+KeyM"), 0);
        bindings.toggle_mic = "Control+KeyM".into();
        bindings.push_to_talk = "Space".into();
        assert_eq!(matching_shortcut_action(&bindings, "Control+KeyM"), 2);
        assert_eq!(matching_shortcut_action(&bindings, "Space"), 4);
        assert_eq!(matching_shortcut_action(&bindings, "Control+KeyU"), 0);
    }

    #[test]
    fn administrative_mute_disables_the_self_unmute_action() {
        let mut state = crate::model::AppState::default();
        assert_eq!(requested_mute_toggle(&state), Some(true));
        state.muted = true;
        assert_eq!(requested_mute_toggle(&state), Some(false));
        state.admin_muted = true;
        assert_eq!(requested_mute_toggle(&state), None);
    }

    #[test]
    fn topology_labels_match_the_visual_state_vocabulary() {
        assert_eq!(topology_label("Hybrid"), "HYBRID");
        assert_eq!(topology_label("P2P"), "P2P");
        assert_eq!(topology_label("Relay"), "SFU");
    }

    #[test]
    fn compact_member_preview_keeps_four_avatars_and_aggregates_the_rest() {
        let members = (0..7)
            .map(|index| MemberItem {
                name: format!("Member {index}").into(),
                avatar: index.to_string().into(),
                avatar_image: Default::default(),
                custom_avatar: false,
                session_id: index.to_string().into(),
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
            })
            .collect::<Vec<_>>();
        let (preview, overflow) = compact_member_preview(&members);
        assert_eq!(preview.len(), 4);
        assert_eq!(overflow, 3);
    }

    #[test]
    fn recent_server_preview_fits_the_two_reference_rows() {
        let servers = (0..3)
            .map(|index| SavedServer {
                display_name: format!("Server {index}"),
                server_name: String::new(),
                address: format!("127.0.0.1:900{index}"),
                nickname: "Lumi".into(),
            })
            .collect::<Vec<_>>();
        let recent = recent_server_items(&servers);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].name.as_str(), "Server 0");
        assert_eq!(recent[1].name.as_str(), "Server 1");
    }

    #[test]
    fn audio_device_combo_keeps_system_default_at_index_zero() {
        let devices = vec!["Studio Mic".to_owned(), "USB Mic".to_owned()];
        let labels = device_options(&devices, "系统默认麦克风");
        assert_eq!(labels, ["系统默认麦克风", "Studio Mic", "USB Mic"]);
        assert_eq!(device_index(&devices, None), 0);
        assert_eq!(device_index(&devices, Some("USB Mic")), 2);
        assert_eq!(device_at(&devices, 0), None);
        assert_eq!(device_at(&devices, 2).as_deref(), Some("USB Mic"));
    }

    #[test]
    fn room_network_policy_selector_maps_all_real_policies() {
        for policy in [
            RoomNetworkPolicy::Auto,
            RoomNetworkPolicy::ForceSfu,
            RoomNetworkPolicy::ForceP2p,
        ] {
            assert_eq!(
                room_network_policy_at(room_network_policy_index(policy)),
                policy
            );
        }
    }

    #[test]
    fn room_network_policy_wire_values_load_the_authoritative_selector_state() {
        assert_eq!(
            room_network_policy_from_wire("force_sfu"),
            RoomNetworkPolicy::ForceSfu
        );
        assert_eq!(
            room_network_policy_from_wire("force_p2p"),
            RoomNetworkPolicy::ForceP2p
        );
        assert_eq!(
            room_network_policy_from_wire("auto"),
            RoomNetworkPolicy::Auto
        );
        assert_eq!(
            room_network_policy_from_wire("unknown"),
            RoomNetworkPolicy::Auto
        );
    }

    #[test]
    fn audio_and_microphone_test_states_have_user_facing_labels() {
        assert_eq!(audio_state_label(&AudioState::Starting), "正在启动…");
        assert_eq!(audio_state_label(&AudioState::Ready), "音频就绪");
        assert_eq!(mic_test_label(MicTestStatus::Recording), "录音中");
        assert_eq!(mic_test_label(MicTestStatus::PreviewReady), "可试听");
    }

    #[test]
    fn audio_detail_labels_preserve_real_backend_semantics() {
        assert_eq!(
            noise_mode_label(NoiseSuppressionMode::Standard),
            "标准降噪（WebRTC APM）"
        );
        assert_eq!(
            noise_mode_label(NoiseSuppressionMode::RnNoise),
            "RNNoise（自动）"
        );
        assert_eq!(
            noise_mode_label(NoiseSuppressionMode::DeepFilterNet),
            "DeepFilterNet · 自然"
        );
        assert_eq!(
            noise_mode_label(NoiseSuppressionMode::DeepFilterNetStrong),
            "DeepFilterNet · 强力"
        );
        assert_eq!(sample_rate_label(None), "自动");
        assert_eq!(sample_rate_label(Some(44_100)), "44.1 kHz");
        assert_eq!(sample_rate_label(Some(48_000)), "48 kHz");
    }
}
