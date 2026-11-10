use std::time::Instant;

use slint::platform::Key;

pub const CAPTURE_WINDOW_MS: u128 = 500;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureProgress {
    Inactive,
    ModifierStarted { action: i32, generation: u64 },
    Waiting,
    Recorded { action: i32, binding: String },
    Expired,
}

#[derive(Debug, Default)]
pub struct ShortcutRecorder {
    action: Option<i32>,
    first_modifier_at: Option<Instant>,
    generation: u64,
}

impl ShortcutRecorder {
    pub fn begin(&mut self, action: i32) {
        self.action = Some(action);
        self.first_modifier_at = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn cancel(&mut self) {
        self.action = None;
        self.first_modifier_at = None;
    }

    pub fn record(
        &mut self,
        text: &str,
        control: bool,
        alt: bool,
        shift: bool,
        meta: bool,
        now: Instant,
    ) -> CaptureProgress {
        let Some(action) = self.action else {
            return CaptureProgress::Inactive;
        };
        if is_modifier_key(text) {
            if self.first_modifier_at.is_none() {
                self.first_modifier_at = Some(now);
                return CaptureProgress::ModifierStarted {
                    action,
                    generation: self.generation,
                };
            }
            return CaptureProgress::Waiting;
        }
        if self
            .first_modifier_at
            .is_some_and(|started| now.duration_since(started).as_millis() > CAPTURE_WINDOW_MS)
        {
            self.cancel();
            return CaptureProgress::Expired;
        }
        let Some(binding) = binding_from_key_event(text, control, alt, shift, meta) else {
            return CaptureProgress::Waiting;
        };
        self.cancel();
        CaptureProgress::Recorded { action, binding }
    }

    pub fn expire(&mut self, action: i32, generation: u64, now: Instant) -> bool {
        let should_expire = self.action == Some(action)
            && self.generation == generation
            && self.first_modifier_at.is_some_and(|started| {
                now.duration_since(started).as_millis() >= CAPTURE_WINDOW_MS
            });
        if should_expire {
            self.cancel();
        }
        should_expire
    }
}

pub fn is_modifier_key(text: &str) -> bool {
    [
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Shift,
        Key::ShiftR,
        Key::Meta,
        Key::MetaR,
    ]
    .into_iter()
    .any(|key| text == char::from(key).to_string())
}

pub fn binding_from_key_event(
    text: &str,
    control: bool,
    alt: bool,
    shift: bool,
    meta: bool,
) -> Option<String> {
    if text.is_empty() || is_modifier_key(text) {
        return None;
    }
    let key = key_code(text)?;
    let mut parts = Vec::with_capacity(5);
    if control {
        parts.push("Control".to_owned());
    }
    if alt {
        parts.push("Alt".to_owned());
    }
    if shift {
        parts.push("Shift".to_owned());
    }
    if meta {
        parts.push("Super".to_owned());
    }
    parts.push(key);
    Some(parts.join("+"))
}

pub fn display_binding(binding: &str) -> String {
    if binding.trim().is_empty() {
        return "未绑定".into();
    }
    binding
        .split('+')
        .map(|part| match part {
            "Control" => "Ctrl".into(),
            "Super" => {
                if cfg!(target_os = "macos") {
                    "Cmd".into()
                } else {
                    "Super".into()
                }
            }
            value if value.len() == 4 && value.starts_with("Key") => {
                value.trim_start_matches("Key").into()
            }
            value if value.len() == 6 && value.starts_with("Digit") => {
                value.trim_start_matches("Digit").into()
            }
            "Backquote" => "`".into(),
            "Minus" => "-".into(),
            "Equal" => "=".into(),
            "BracketLeft" => "[".into(),
            "BracketRight" => "]".into(),
            "Backslash" => "\\".into(),
            "Semicolon" => ";".into(),
            "Quote" => "'".into(),
            "Comma" => ",".into(),
            "Period" => ".".into(),
            "Slash" => "/".into(),
            other => other.into(),
        })
        .collect::<Vec<String>>()
        .join(" + ")
}

fn key_code(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(match first {
            'a'..='z' | 'A'..='Z' => format!("Key{}", first.to_ascii_uppercase()),
            '0'..='9' => format!("Digit{first}"),
            ' ' => "Space".into(),
            '`' | '~' => "Backquote".into(),
            '-' | '_' => "Minus".into(),
            '=' | '+' => "Equal".into(),
            '[' | '{' => "BracketLeft".into(),
            ']' | '}' => "BracketRight".into(),
            '\\' | '|' => "Backslash".into(),
            ';' | ':' => "Semicolon".into(),
            '\'' | '"' => "Quote".into(),
            ',' | '<' => "Comma".into(),
            '.' | '>' => "Period".into(),
            '/' | '?' => "Slash".into(),
            _ => named_key(first)?,
        });
    }
    None
}

fn named_key(value: char) -> Option<String> {
    let keys = [
        (Key::Escape, "Escape"),
        (Key::Return, "Enter"),
        (Key::Tab, "Tab"),
        (Key::Backspace, "Backspace"),
        (Key::Delete, "Delete"),
        (Key::UpArrow, "ArrowUp"),
        (Key::DownArrow, "ArrowDown"),
        (Key::LeftArrow, "ArrowLeft"),
        (Key::RightArrow, "ArrowRight"),
        (Key::Home, "Home"),
        (Key::End, "End"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
        (Key::F1, "F1"),
        (Key::F2, "F2"),
        (Key::F3, "F3"),
        (Key::F4, "F4"),
        (Key::F5, "F5"),
        (Key::F6, "F6"),
        (Key::F7, "F7"),
        (Key::F8, "F8"),
        (Key::F9, "F9"),
        (Key::F10, "F10"),
        (Key::F11, "F11"),
        (Key::F12, "F12"),
    ];
    keys.into_iter()
        .find_map(|(key, name)| (value == char::from(key)).then(|| name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_single_keys_and_modifier_chords() {
        assert_eq!(
            binding_from_key_event("b", true, false, false, false).as_deref(),
            Some("Control+KeyB")
        );
        assert_eq!(
            binding_from_key_event(" ", false, false, false, false).as_deref(),
            Some("Space")
        );
        assert_eq!(
            binding_from_key_event("S", true, false, true, false).as_deref(),
            Some("Control+Shift+KeyS")
        );
    }

    #[test]
    fn modifier_press_waits_for_the_main_key() {
        let control = char::from(Key::Control).to_string();
        assert!(is_modifier_key(&control));
        assert_eq!(
            binding_from_key_event(&control, true, false, false, false),
            None
        );
    }

    #[test]
    fn empty_binding_has_an_explicit_label() {
        assert_eq!(display_binding(""), "未绑定");
        assert_eq!(display_binding("Control+Shift+KeyS"), "Ctrl + Shift + S");
    }

    #[test]
    fn recorder_accepts_a_modifier_chord_inside_the_half_second_window() {
        let start = Instant::now();
        let mut recorder = ShortcutRecorder::default();
        recorder.begin(2);
        let control = char::from(Key::Control).to_string();
        assert_eq!(
            recorder.record(&control, true, false, false, false, start),
            CaptureProgress::ModifierStarted {
                action: 2,
                generation: 1,
            }
        );
        assert_eq!(
            recorder.record(
                "s",
                true,
                false,
                true,
                false,
                start + std::time::Duration::from_millis(499),
            ),
            CaptureProgress::Recorded {
                action: 2,
                binding: "Control+Shift+KeyS".into(),
            }
        );
    }

    #[test]
    fn recorder_rejects_a_modifier_chord_after_the_half_second_window() {
        let start = Instant::now();
        let mut recorder = ShortcutRecorder::default();
        recorder.begin(1);
        let control = char::from(Key::Control).to_string();
        let CaptureProgress::ModifierStarted { generation, .. } =
            recorder.record(&control, true, false, false, false, start)
        else {
            panic!("modifier did not start the capture window");
        };
        assert!(recorder.expire(1, generation, start + std::time::Duration::from_millis(500)));
        assert_eq!(
            recorder.record(
                "m",
                true,
                false,
                false,
                false,
                start + std::time::Duration::from_millis(501),
            ),
            CaptureProgress::Inactive
        );
    }

    #[test]
    fn recorder_completes_a_single_key_without_starting_a_timer() {
        let mut recorder = ShortcutRecorder::default();
        recorder.begin(3);
        assert_eq!(
            recorder.record(" ", false, false, false, false, Instant::now()),
            CaptureProgress::Recorded {
                action: 3,
                binding: "Space".into(),
            }
        );
    }
}
