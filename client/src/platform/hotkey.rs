use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, hotkey::HotKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

pub struct GlobalPttHotkey {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

impl GlobalPttHotkey {
    pub fn register(binding: &str) -> Result<Self, String> {
        let hotkey = parse_binding(binding)?;
        let manager = GlobalHotKeyManager::new()
            .map_err(|error| format!("global PTT hotkey service unavailable: {error}"))?;
        manager.register(hotkey).map_err(|error| {
            format!("{binding} global PTT hotkey could not be registered: {error}")
        })?;
        Ok(Self { manager, hotkey })
    }

    pub fn try_next(&self) -> Option<HotkeyEvent> {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.hotkey.id() {
                return Some(match event.state {
                    global_hotkey::HotKeyState::Pressed => HotkeyEvent::Pressed,
                    global_hotkey::HotKeyState::Released => HotkeyEvent::Released,
                });
            }
        }
        None
    }
}

fn parse_binding(binding: &str) -> Result<HotKey, String> {
    if binding.trim().is_empty() {
        return Err("PTT 快捷键尚未绑定".into());
    }
    binding
        .parse::<HotKey>()
        .map_err(|_| format!("unsupported PTT shortcut: {binding}"))
}

impl Drop for GlobalPttHotkey {
    fn drop(&mut self) {
        let _ = self.manager.unregister(self.hotkey);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_ptt_accepts_single_keys_and_modifier_chords() {
        assert!(parse_binding("Space").is_ok());
        assert!(parse_binding("Control+Shift+KeyS").is_ok());
        assert_eq!(parse_binding("").unwrap_err(), "PTT 快捷键尚未绑定");
    }
}
