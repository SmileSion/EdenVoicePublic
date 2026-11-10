//! macOS output-only capture; no microphone fallback and no voice queue access.
#[derive(Clone, serde::Deserialize)]
pub struct AudioContext {
    server: String,
    session: String,
    channel: String,
}
#[cfg(target_os = "macos")]
fn connected(snapshot: &crate::CoreSnapshot, context: &AudioContext) -> bool {
    snapshot.connection == "connected"
        && snapshot.screen_share_supported
        && snapshot.server_address == context.server
        && snapshot.self_session_id == context.session
        && snapshot.users.iter().any(|u| {
            u.session_id == context.session
                && u.channel_id == context.channel
                && u.channel_id != "0"
        })
}
#[cfg(target_os = "macos")]
mod platform {
    use std::{
        ffi::c_void,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };
    unsafe extern "C" {
        fn eden_audio_start(handle: *mut *mut c_void, rate: *mut f64) -> i32;
        fn eden_audio_read(handle: *mut c_void, pcm: *mut f32, frames: u32) -> u32;
        fn eden_audio_stop(handle: *mut c_void);
        fn eden_audio_set_paused(handle: *mut c_void, paused: i32);
    }
    struct Capture {
        pointer: *mut c_void,
        token: String,
        last_read: Instant,
        context: super::AudioContext,
        snapshot: std::sync::Arc<std::sync::RwLock<crate::CoreSnapshot>>,
    }
    // Native non-callback operations are serialized under CAPTURE's mutex.
    unsafe impl Send for Capture {}
    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe { eden_audio_stop(self.pointer) }
        }
    }
    static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);
    static WATCHDOG: OnceLock<()> = OnceLock::new();
    pub fn start(
        token: String,
        context: super::AudioContext,
        snapshot: std::sync::Arc<std::sync::RwLock<crate::CoreSnapshot>>,
    ) -> Result<f64, String> {
        if token.is_empty() || token.len() > 64 {
            return Err("无效的音频采集标识".into());
        }
        WATCHDOG.get_or_init(|| {
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    if let Ok(mut state) = CAPTURE.lock() {
                        if state.as_ref().is_some_and(|s| {
                            s.last_read.elapsed() > Duration::from_secs(2)
                                || !s
                                    .snapshot
                                    .read()
                                    .map(|view| super::connected(&view, &s.context))
                                    .unwrap_or(false)
                        }) {
                            state.take();
                        }
                    }
                }
            });
        });
        let mut state = CAPTURE.lock().map_err(|_| "系统音频状态不可用")?;
        if state.is_some() {
            return Err("已有系统音频采集，请先停止共享".into());
        }
        if !snapshot
            .read()
            .map(|view| super::connected(&view, &context))
            .unwrap_or(false)
        {
            return Err("语音房间已改变，不能启动系统音频采集".into());
        }
        let mut pointer = std::ptr::null_mut();
        let mut rate = 0.;
        let status = unsafe { eden_audio_start(&mut pointer, &mut rate) };
        if status != 0 {
            return Err(format!(
                "系统声音采集未启动（{status}）。需要 macOS 14.2 或更新版本，并允许系统音频录制。"
            ));
        }
        *state = Some(Capture {
            pointer,
            token,
            last_read: Instant::now(),
            context,
            snapshot,
        });
        Ok(rate)
    }
    pub fn read(token: &str) -> Result<Vec<f32>, String> {
        let mut state = CAPTURE.lock().map_err(|_| "系统音频状态不可用")?;
        let c = state
            .as_mut()
            .filter(|s| s.token == token)
            .ok_or("系统音频采集已停止")?;
        let mut pcm = vec![0.; 16384];
        let frames = unsafe { eden_audio_read(c.pointer, pcm.as_mut_ptr(), 8192) };
        pcm.truncate(frames as usize * 2);
        c.last_read = Instant::now();
        Ok(pcm)
    }
    pub fn pause(token: &str, paused: bool) -> Result<(), String> {
        let mut state = CAPTURE.lock().map_err(|_| "系统音频状态不可用")?;
        let capture = state.as_ref().filter(|s| s.token == token).ok_or("系统音频采集已停止")?;
        let valid = capture.snapshot.read().map(|view| super::connected(&view, &capture.context)).unwrap_or(false);
        if !valid {
            state.take();
            return Err("语音房间已改变，系统音频采集已停止".into());
        }
        let c = state.as_mut().expect("capture was checked above");
        unsafe { eden_audio_set_paused(c.pointer, i32::from(paused)); }
        c.last_read = Instant::now();
        Ok(())
    }
    pub fn stop(token: &str) {
        if let Ok(mut state) = CAPTURE.lock() {
            if state.as_ref().is_some_and(|s| s.token == token) {
                state.take();
            }
        }
    }
}
#[tauri::command]
pub async fn start_screen_audio(
    token: String,
    context: AudioContext,
    state: tauri::State<'_, crate::BridgeState>,
) -> Result<f64, String> {
    #[cfg(target_os = "macos")]
    {
        let snapshot = state.snapshot.clone();
        tauri::async_runtime::spawn_blocking(move || platform::start(token, context, snapshot))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (token, context, state);
        Err("此平台通过系统画面选择器共享声音".into())
    }
}
#[tauri::command]
pub fn read_screen_audio(token: String) -> Result<Vec<f32>, String> {
    #[cfg(target_os = "macos")]
    {
        platform::read(&token)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = token;
        Err("原生系统音频不可用".into())
    }
}
#[tauri::command]
pub fn stop_screen_audio(token: String) {
    #[cfg(target_os = "macos")]
    platform::stop(&token);
    #[cfg(not(target_os = "macos"))]
    let _ = token;
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn capture_requires_the_same_connected_voice_room() {
        let context = AudioContext {
            server: "server".into(),
            session: "7".into(),
            channel: "2".into(),
        };
        let mut snapshot = crate::CoreSnapshot::default();
        snapshot.connection = "connected".into();
        snapshot.screen_share_supported = true;
        snapshot.server_address = "server".into();
        snapshot.self_session_id = "7".into();
        snapshot.users.push(crate::UserSnapshot {
            session_id: "7".into(),
            channel_id: "2".into(),
            ..Default::default()
        });
        assert!(connected(&snapshot, &context));
        snapshot.users[0].channel_id = "3".into();
        assert!(!connected(&snapshot, &context));
        snapshot.users[0].channel_id = "2".into();
        snapshot.self_session_id = "8".into();
        assert!(!connected(&snapshot, &context));
        snapshot.self_session_id = "7".into();
        snapshot.connection = "disconnected".into();
        assert!(!connected(&snapshot, &context));
    }
}

#[tauri::command]
pub fn set_screen_audio_paused(token: String, paused: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    { platform::pause(&token, paused) }
    #[cfg(not(target_os = "macos"))]
    { let _ = (token, paused); Err("原生系统音频不可用".into()) }
}
