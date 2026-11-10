//! Close-button policy is native so titlebar close and Alt+F4 agree, even at startup.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{Emitter, Manager, WebviewWindow};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    #[default]
    Ask,
    Tray,
    Quit,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Cancel,
    Tray,
    Quit,
}
static BEHAVIOR: Mutex<Behavior> = Mutex::new(Behavior::Ask);
static PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    behavior: Behavior,
    pending: bool,
    tray_available: bool,
}
fn read(path: &Path) -> Behavior {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
fn write(path: &Path, behavior: Behavior) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&behavior).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}
pub fn setup(app: &tauri::AppHandle) {
    if let Ok(dir) = app.path().app_config_dir() {
        *BEHAVIOR.lock().unwrap() = read(&dir.join("window-close.json"));
    }
}
fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("仅主窗口可以调整关闭行为".into())
    }
}
#[tauri::command]
pub fn get_close_preferences(window: WebviewWindow) -> Result<Preferences, String> {
    ensure_main(&window)?;
    Ok(Preferences {
        behavior: *BEHAVIOR.lock().unwrap(),
        pending: PENDING.load(Ordering::Acquire),
        tray_available: crate::tray::available(&window),
    })
}
#[tauri::command]
pub fn set_close_behavior(window: WebviewWindow, behavior: Behavior) -> Result<(), String> {
    ensure_main(&window)?;
    let mut current = BEHAVIOR.lock().unwrap();
    let path = window
        .app_handle()
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("window-close.json");
    write(&path, behavior)?;
    *current = behavior;
    drop(current);
    let _ = window.emit("close-behavior-changed", behavior);
    Ok(())
}
pub fn request(window: &WebviewWindow) {
    // A second close while the chooser is visible must not apply a changed default.
    if !PENDING.load(Ordering::Acquire) {
        let behavior = *BEHAVIOR.lock().unwrap();
        match behavior {
            Behavior::Quit => {
                window.app_handle().exit(0);
                return;
            }
            Behavior::Tray if crate::tray::hide_main(window).unwrap_or(false) => return,
            _ => {}
        }
    }
    PENDING.store(true, Ordering::Release);
    let _ = window.show();
    let _ = window.set_focus();
    if let Ok(preferences) = get_close_preferences(window.clone()) {
        let _ = window.emit("window-close-requested", preferences);
    }
}
#[tauri::command]
pub fn resolve_window_close(
    window: WebviewWindow,
    action: Action,
    remember: bool,
) -> Result<(), String> {
    ensure_main(&window)?;
    if !PENDING.load(Ordering::Acquire) {
        return Ok(());
    }
    match action {
        Action::Cancel => {}
        Action::Tray => {
            if !crate::tray::available(&window) {
                return Err("系统托盘暂不可用，请取消或选择退出程序".into());
            }
            if remember {
                set_close_behavior(window.clone(), Behavior::Tray)?;
            }
            if !crate::tray::hide_main(&window).map_err(|e| e.to_string())? {
                return Err("无法收起到系统托盘".into());
            }
        }
        Action::Quit => {
            if remember {
                set_close_behavior(window.clone(), Behavior::Quit)?;
            }
            PENDING.store(false, Ordering::Release);
            window.app_handle().exit(0);
            return Ok(());
        }
    }
    PENDING.store(false, Ordering::Release);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_ask_and_preferences_survive_replacement() {
        let dir = std::env::temp_dir().join(format!("edenvoice-close-{}", std::process::id()));
        let path = dir.join("window-close.json");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(read(&path), Behavior::Ask);
        write(&path, Behavior::Tray).unwrap();
        assert_eq!(read(&path), Behavior::Tray);
        write(&path, Behavior::Quit).unwrap();
        assert_eq!(read(&path), Behavior::Quit);
        write(&path, Behavior::Ask).unwrap();
        assert_eq!(read(&path), Behavior::Ask);
        fs::write(&path, b"invalid").unwrap();
        assert_eq!(read(&path), Behavior::Ask);
        fs::remove_dir_all(dir).unwrap();
    }
}
