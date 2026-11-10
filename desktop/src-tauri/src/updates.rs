use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct UpdateState {
    candidate: Mutex<Option<Update>>,
    busy: AtomicBool,
}
struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn lock(state: &UpdateState) -> Result<Busy<'_>, String> {
    state
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "更新正在处理中")?;
    Ok(Busy(&state.busy))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    version: String,
    url: String,
    download_url: Option<String>,
    automatic: bool,
    notes: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    version: String,
    signature: String,
    url: String,
    platform: String,
    #[serde(default)]
    notes: String,
}
fn cache(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|p| p.join("pending-update"))
        .map_err(|e| e.to_string())
}
fn pending(app: &tauri::AppHandle) -> Option<Pending> {
    let path = cache(app).ok()?;
    let value: Pending =
        serde_json::from_slice(&std::fs::read(path.join("pending.json")).ok()?).ok()?;
    let version = semver::Version::parse(&value.version).ok()?;
    if version <= app.package_info().version {
        let _ = std::fs::remove_dir_all(&path);
        return None;
    }
    path.join("package.bin").is_file().then_some(value)
}
fn verify(bytes: &[u8], signature: &str) -> Result<(), String> {
    verify_with_key(bytes, signature, include_str!("../updater.pub"))
}
fn verify_with_key(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let decode = |s: &str| -> Result<String, String> {
        String::from_utf8(STANDARD.decode(s.trim()).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    };
    let key =
        minisign_verify::PublicKey::decode(&decode(public_key)?).map_err(|e| e.to_string())?;
    let signature =
        minisign_verify::Signature::decode(&decode(signature)?).map_err(|e| e.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|_| "更新包签名校验失败".into())
}
fn builder(app: &tauri::AppHandle) -> tauri_plugin_updater::UpdaterBuilder {
    let handle = app.clone();
    app.updater_builder()
        .timeout(Duration::from_secs(15))
        .restart_after_install(true)
        .on_before_exit(move || {
            if let Some(state) = handle.try_state::<super::BridgeState>() {
                super::shutdown_and_wait(&state);
            }
            super::EXITING.store(true, Ordering::Release);
        })
}
fn platform() -> String {
    format!(
        "{}-{}",
        if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        },
        std::env::consts::ARCH
    )
}
fn valid_record(r: &Pending) -> bool {
    let url_ok = tauri::Url::parse(&r.url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    });
    url_ok
        && r.platform == platform()
        && r.url.len() <= 2048
        && r.notes.len() <= 2048
        && r.signature.len() <= 2048
        && r.url.starts_with("https://")
        && semver::Version::parse(&r.version).is_ok()
}
// The official updater only constructs installers via a manifest HTTP response.
// Adapt the authenticated control snapshot on a short-lived loopback socket; no
// public listener, GitHub request or external unsigned manifest is involved.
async fn from_record(app: &tauri::AppHandle, r: &Pending) -> Result<Update, String> {
    use std::io::{Read, Write};
    if !valid_record(r) {
        return Err("更新配置无效".into());
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let body=serde_json::to_string(&serde_json::json!({"version":r.version,"url":r.url,"signature":r.signature,"notes":r.notes})).map_err(|e|e.to_string())?;
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while std::time::Instant::now() < deadline {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut update = builder(app)
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .endpoints(vec![format!("http://{addr}/manifest").parse().unwrap()])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("没有可用更新")?;
    update.no_proxy = false;
    Ok(update)
}
#[tauri::command]
pub async fn check_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<Option<Release>, String> {
    let _busy = lock(&state)?;
    let record = {
        let core = app.state::<super::BridgeState>();
        let snapshot = core.snapshot.read().map_err(|e| e.to_string())?;
        if snapshot.connection != "connected" {
            return Err("未连接服务器".into());
        }
        snapshot
            .client_updates
            .iter()
            .filter_map(|r| serde_json::from_value::<Pending>(r.clone()).ok())
            .find(|r| r.platform == platform())
    };
    let next = if let Some(record) = record {
        if semver::Version::parse(&record.version).is_ok_and(|v| v > app.package_info().version) {
            Some(from_record(&app, &record).await?)
        } else {
            None
        }
    } else {
        None
    };
    let release = next.as_ref().map(|u| Release {
        version: u.version.clone(),
        url: u.download_url.to_string(),
        download_url: None,
        automatic: true,
        notes: u.body.clone().unwrap_or_default(),
    });
    *state.candidate.lock().map_err(|e| e.to_string())? = next;
    Ok(release)
}
#[tauri::command]
pub fn pending_update(app: tauri::AppHandle) -> Option<Pending> {
    pending(&app)
}
#[tauri::command]
pub fn cancel_pending_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<(), String> {
    let _busy = lock(&state)?;
    if let Ok(path) = cache(&app)
        && path.exists()
    {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
pub async fn download_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
    version: String,
) -> Result<(), String> {
    let _busy = lock(&state)?;
    let mut update = state
        .candidate
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .filter(|u| u.version == version)
        .ok_or("请重新检查更新")?;
    {
        let core = app.state::<super::BridgeState>();
        let snapshot = core.snapshot.read().map_err(|e| e.to_string())?;
        if snapshot.connection != "connected"
            || !snapshot.client_updates.iter().any(|r| {
                r["platform"] == platform()
                    && r["version"] == update.version
                    && r["url"] == update.download_url.as_str()
                    && r["signature"] == update.signature
            })
        {
            return Err("更新已暂停或发生变化，请重新检查".into());
        }
    }
    update.timeout = Some(Duration::from_secs(600));
    let mut downloaded = 0u64;
    let mut last = std::time::Instant::now();
    let bytes = update
        .download(
            |size, total| {
                downloaded += size as u64;
                if last.elapsed() >= Duration::from_millis(150) {
                    let _ = app.emit(
                        "eden-update-progress",
                        serde_json::json!({"downloaded":downloaded,"total":total}),
                    );
                    last = std::time::Instant::now();
                }
            },
            || {},
        )
        .await
        .map_err(|_| "下载未完成")?;
    let path = cache(&app)?;
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    // Publish metadata last: incomplete downloads can never become scheduled installs.
    let _ = std::fs::remove_file(path.join("pending.json"));
    std::fs::write(path.join("package.bin"), bytes).map_err(|e| e.to_string())?;
    let record = Pending {
        version,
        signature: update.signature.clone(),
        url: update.download_url.to_string(),
        platform: platform(),
        notes: update.body.clone().unwrap_or_default(),
    };
    std::fs::write(
        path.join("pending.tmp"),
        serde_json::to_vec(&record).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(path.join("pending.tmp"), path.join("pending.json"))
        .map_err(|e| e.to_string())?;
    Ok(())
}
async fn install(app: &tauri::AppHandle, allow_disconnect: bool) -> Result<(), String> {
    let record = pending(app).ok_or("没有已下载的更新")?;
    let cached = app
        .state::<UpdateState>()
        .candidate
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let update = if let Some(update) =
        cached.filter(|u| u.version == record.version && u.signature == record.signature)
    {
        update
    } else {
        from_record(app, &record).await?
    };
    let bytes = std::fs::read(cache(app)?.join("package.bin")).map_err(|e| e.to_string())?;
    verify(&bytes, &record.signature)?;
    if !allow_disconnect
        && let Some(state) = app.try_state::<super::BridgeState>()
        && state
            .snapshot
            .read()
            .map(|s| super::joined_channel_id(&s).is_some())
            .unwrap_or(true)
    {
        return Err("IN_CALL".into());
    }
    // No await between the final call-state check and handing off to the installer.
    if let Err(error) = update.install(bytes) {
        // Windows runs its shutdown hook before launching NSIS. Recover the old
        // client if the OS refuses to launch it, rather than leaving a dead core.
        if app
            .try_state::<super::BridgeState>()
            .is_some_and(|state| state.stopped.load(Ordering::Acquire))
        {
            app.restart();
        }
        super::EXITING.store(false, Ordering::Release);
        return Err(error.to_string());
    }
    let _ = std::fs::remove_dir_all(cache(app)?);
    if let Some(state) = app.try_state::<super::BridgeState>() {
        super::shutdown_and_wait(&state);
    }
    super::EXITING.store(true, Ordering::Release);
    app.restart();
}
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, UpdateState>,
    allow_disconnect: bool,
) -> Result<(), String> {
    let _busy = lock(&state)?;
    install(&app, allow_disconnect).await
}
/// Only explicit previous download consent schedules this path, before voice connects.
pub fn install_on_startup(app: &tauri::AppHandle) {
    if pending(app).is_some() {
        let _ = tauri::async_runtime::block_on(install(app, false));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_package_must_match_signature_and_pinned_key() {
        let bytes = include_bytes!("../tests/fixtures/update.txt");
        let signature = include_str!("../tests/fixtures/update.txt.sig");
        let key = include_str!("../tests/fixtures/update.pub");
        assert!(verify_with_key(bytes, signature, key).is_ok());
        assert!(verify_with_key(b"tampered", signature, key).is_err());
        assert!(verify(bytes, signature).is_err()); // A different signing key is never accepted.
        assert!(verify(bytes, "invalid signature").is_err());
    }
    #[test]
    fn config_and_cache_verifier_pin_same_public_key() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["updater"]["pubkey"]
                .as_str()
                .unwrap()
                .trim(),
            include_str!("../updater.pub").trim()
        );
    }
}
