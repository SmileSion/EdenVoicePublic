//! External mpv process and local IPC. All process/file/pipe work runs off-core.
use edenvoice_client::protocol::watch::WatchFile;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static WORKERS: AtomicUsize = AtomicUsize::new(0);
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn workers_running() -> bool {
    WORKERS.load(Ordering::Acquire) != 0
}
pub fn shutdown_all() {
    SHUTTING_DOWN.store(true, Ordering::Release);
}
struct WorkerGuard;
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub enum Control {
    Apply {
        position: Option<f64>,
        paused: bool,
        speed: f64,
    },
    Stop,
}
pub enum Event {
    Ready(WatchFile),
    Sample {
        position: f64,
        paused: bool,
        seek: bool,
    },
    Closed,
    Error(String),
}

pub fn executable_path(value: &str) -> Result<PathBuf, String> {
    let base = PathBuf::from(value.trim());
    #[cfg(target_os = "macos")]
    let path = if base
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        base.join("Contents/MacOS/mpv")
    } else {
        base
    };
    #[cfg(not(target_os = "macos"))]
    let path = base;
    if !path.is_absolute() || !path.is_file() {
        return Err("请选择有效的 mpv 可执行文件 / Select an mpv executable".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&path)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("mpv 文件没有执行权限 / mpv is not executable".into());
        }
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err("请选择 mpv.exe / Select mpv.exe".into());
    }
    Ok(path)
}

pub struct Player {
    pub commands: SyncSender<Control>,
    pub events: Receiver<Event>,
}
impl Player {
    pub fn launch(executable: String, file: PathBuf) -> Result<Self, String> {
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return Err("application is shutting down".into());
        }
        let (commands, rx) = mpsc::sync_channel(16);
        let (tx, events) = mpsc::sync_channel(32);
        WORKERS.fetch_add(1, Ordering::AcqRel);
        let guard = WorkerGuard;
        thread::Builder::new()
            .name("edenvoice-mpv".into())
            .spawn(move || {
                let _guard = guard;
                if let Err(error) = run(executable, file, rx, &tx) {
                    let _ = tx.try_send(Event::Error(error));
                }
                let _ = tx.try_send(Event::Closed);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { commands, events })
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Control::Stop);
    }
}

// Drop kills only this owned child and removes only our private IPC directory.
struct Owned {
    child: Child,
    directory: Option<PathBuf>,
}
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(directory) = &self.directory {
            let _ = fs::remove_file(directory.join("socket"));
            let _ = fs::remove_dir(directory);
        }
    }
}

#[cfg(unix)]
type Pipe = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Pipe = fs::File;

#[cfg(unix)]
fn connect(path: &str) -> io::Result<Pipe> {
    let pipe = Pipe::connect(path)?;
    pipe.set_nonblocking(true)?;
    Ok(pipe)
}
#[cfg(windows)]
fn connect(path: &str) -> io::Result<Pipe> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetNamedPipeHandleState(
            handle: *mut std::ffi::c_void,
            mode: *const u32,
            max_collection_count: *const u32,
            collect_data_timeout: *const u32,
        ) -> i32;
    }
    let pipe = fs::OpenOptions::new().read(true).write(true).open(path)?;
    // Byte mode, nonblocking. No blocking pipe operation can stall shutdown.
    let mode = 1u32;
    if unsafe {
        SetNamedPipeHandleState(
            pipe.as_raw_handle(),
            &mode,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(pipe)
}

fn retryable(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    ) || cfg!(windows) && matches!(e.raw_os_error(), Some(232) | Some(231))
}

fn enqueue(queue: &mut VecDeque<Vec<u8>>, command: Value) -> Result<(), String> {
    if queue.len() >= 32 {
        return Err("mpv IPC is not responding".into());
    }
    let mut bytes = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    queue.push_back(bytes);
    Ok(())
}

fn run(
    executable: String,
    file: PathBuf,
    commands: Receiver<Control>,
    events: &SyncSender<Event>,
) -> Result<(), String> {
    let executable = executable_path(&executable)?;
    let file =
        fs::canonicalize(file).map_err(|e| format!("无法打开所选影片 / Cannot open video: {e}"))?;
    #[cfg(windows)]
    let file = PathBuf::from(windows_media_path(&file.to_string_lossy()));
    let metadata = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Video must be a local file".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[cfg(unix)]
    let (address, directory) = {
        use std::os::unix::fs::DirBuilderExt;
        // /tmp is short enough for sockaddr_un on macOS; mode 0700 keeps IPC private.
        let directory = PathBuf::from(format!("/tmp/ev-mpv-{}-{nonce:x}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|e| e.to_string())?;
        (
            directory.join("socket").to_string_lossy().into_owned(),
            Some(directory),
        )
    };
    #[cfg(windows)]
    let (address, directory): (String, Option<PathBuf>) = (
        format!(r"\\.\pipe\edenvoice-mpv-{}-{nonce:x}", std::process::id()),
        None,
    );
    let mut command = Command::new(&executable);
    command
        .args([
            "--no-config",
            "--load-scripts=no",
            "--idle=yes",
            "--keep-open=yes",
            "--pause=yes",
            "--terminal=yes",
            "--msg-level=all=warn",
            "--force-window=yes",
        ])
        .arg(format!("--input-ipc-server={address}"))
        .arg("--")
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: suppress console, not video window.
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(dir) = directory {
                let _ = fs::remove_dir(dir);
            }
            return Err(format!("Cannot launch mpv: {error}"));
        }
    };
    let diagnostic = Arc::new(Mutex::new(VecDeque::<u8>::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let tail = Arc::clone(&diagnostic);
        let _ = thread::Builder::new()
            .name("mpv-diagnostics".into())
            .spawn(move || {
                let mut chunk = [0u8; 1024];
                while let Ok(n) = stderr.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut bytes) = tail.lock() {
                        bytes.extend(&chunk[..n]);
                        while bytes.len() > 4096 {
                            bytes.pop_front();
                        }
                    }
                }
            });
    }
    let details = || {
        diagnostic
            .lock()
            .map(|bytes| {
                String::from_utf8_lossy(&bytes.iter().copied().collect::<Vec<_>>())
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    };
    let mut owned = Owned { child, directory };
    let started = Instant::now();
    let mut pipe = loop {
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Ok(Some(status)) = owned.child.try_wait() {
            return Err(format!(
                "mpv 启动失败 ({status})。请确认选择 mpv.exe 而非安装程序，且播放器依赖完整。\n{}",
                details()
            ));
        }
        match commands.try_recv() {
            Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            _ => {}
        }
        if let Ok(pipe) = connect(&address) {
            break pipe;
        }
        if started.elapsed() > Duration::from_secs(8) {
            return Err(format!(
                "无法连接 mpv 控制管道，请检查播放器版本或安全软件。\n{}",
                details()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    let name = display_name(
        file.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    );
    let mut queue = VecDeque::new();
    let mut offset = 0;
    let mut input = Vec::new();
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    let mut last_response = Instant::now();
    let mut ready = false;
    let mut position = 0.0;
    let mut seeking = false;
    loop {
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return Ok(());
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Ok(Control::Apply {
                position,
                paused,
                speed,
            }) => {
                if let Some(p) = position.filter(|p| p.is_finite() && *p >= 0.0) {
                    enqueue(&mut queue, json!({"command":["seek",p,"absolute+exact"]}))?;
                }
                enqueue(
                    &mut queue,
                    json!({"command":["set_property","pause",paused]}),
                )?;
                enqueue(
                    &mut queue,
                    json!({"command":["set_property","speed",speed.clamp(0.98,1.02)]}),
                )?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(status) = owned.child.try_wait().map_err(|e| e.to_string())? {
            if !ready || !status.success() {
                return Err(format!(
                    "mpv 在影片就绪前退出或异常退出 ({status})。\n{}",
                    details()
                ));
            }
            return Ok(());
        }
        if last_poll.elapsed() >= Duration::from_millis(200) {
            for (id, property) in [(1, "time-pos"), (2, "pause"), (3, "duration")] {
                enqueue(
                    &mut queue,
                    json!({"command":["get_property",property],"request_id":id}),
                )?;
            }
            last_poll = Instant::now();
        }
        while let Some(bytes) = queue.front() {
            match pipe.write(&bytes[offset..]) {
                Ok(0) => return Err("mpv IPC closed".into()),
                Ok(n) => {
                    offset += n;
                    if offset == bytes.len() {
                        queue.pop_front();
                        offset = 0;
                    }
                }
                Err(e) if retryable(&e) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut buf = [0u8; 4096];
        // Bound work and buffered data even if a misconfigured executable floods IPC.
        for _ in 0..16 {
            match pipe.read(&mut buf) {
                // Rust maps Windows ERROR_NO_DATA to BrokenPipe and File::read
                // converts that to Ok(0). An idle PIPE_NOWAIT is therefore not EOF.
                // Child status and the bounded response timeout determine liveness.
                #[cfg(windows)]
                Ok(0) => break,
                #[cfg(not(windows))]
                Ok(0) => return Err("mpv 控制连接已关闭".into()),
                Ok(n) => input.extend_from_slice(&buf[..n]),
                Err(e) if retryable(&e) => break,
                Err(e) => return Err(e.to_string()),
            }
            if input.len() > 65536 {
                return Err("mpv IPC frame too large".into());
            }
        }
        while let Some(end) = input.iter().position(|b| *b == b'\n') {
            let message: Value =
                serde_json::from_slice(&input[..end]).map_err(|e| e.to_string())?;
            input.drain(..=end);
            last_response = Instant::now();
            if message["event"] == "end-file" && message["reason"] == "error" {
                return Err(format!(
                    "mpv 无法加载影片：{}\n{}",
                    message["file_error"]
                        .as_str()
                        .unwrap_or("不支持的文件或解码失败"),
                    details()
                ));
            }
            if message["event"] == "seek" {
                seeking = true;
            }
            if message["event"] == "start-file" && ready {
                return Err("请在 EdenVoice 中更换视频 / Reopen the video from EdenVoice".into());
            }
            if message["error"] == "success" {
                match message["request_id"].as_u64() {
                    Some(1) => {
                        if let Some(p) = message["data"]
                            .as_f64()
                            .filter(|p| p.is_finite() && *p >= 0.0)
                        {
                            position = p;
                        }
                    }
                    Some(2) => {
                        if ready && let Some(paused) = message["data"].as_bool() {
                            let _ = events.try_send(Event::Sample {
                                position,
                                paused,
                                seek: seeking,
                            });
                            seeking = false;
                        }
                    }
                    Some(3) if !ready => {
                        if let Some(duration) = message["data"]
                            .as_f64()
                            .filter(|d| d.is_finite() && *d > 0.0 && *d <= 604800.0)
                        {
                            events
                                .try_send(Event::Ready(WatchFile {
                                    name: name.clone(),
                                    size: metadata.len(),
                                    duration,
                                }))
                                .map_err(|_| "mpv event queue full")?;
                            ready = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if (!ready && started.elapsed() > Duration::from_secs(20))
            || last_response.elapsed() > Duration::from_secs(5)
        {
            return Err(format!(
                "mpv 加载影片超时或未响应，请检查影片和播放器。\n{}",
                details()
            ));
        }
    }
}

#[cfg(any(windows, test))]
fn windows_media_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
}

fn display_name(name: &str) -> String {
    let clean = name.replace(['/', '\\', '\0', '\r', '\n'], "_");
    let mut end = clean.len().min(128);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    let name = clean[..end].to_string();
    if name.is_empty() {
        "video".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_video_paths_are_compatible_with_media_libraries() {
        assert_eq!(
            windows_media_path(r"\\?\C:\影片\movie.mkv"),
            r"C:\影片\movie.mkv"
        );
        assert_eq!(
            windows_media_path(r"\\?\UNC\nas\films\a.mkv"),
            r"\\nas\films\a.mkv"
        );
    }
    #[test]
    fn file_names_are_bounded_utf8_and_not_paths() {
        let name = display_name(&"电影".repeat(100));
        assert!(name.len() <= 128);
        assert!(!name.is_empty());
        assert_eq!(display_name("a/b\\c\n"), "a_b_c_");
    }
    #[test]
    fn ipc_queue_is_bounded() {
        let mut q = VecDeque::new();
        for _ in 0..32 {
            enqueue(&mut q, json!({"command":["get_property","pause"]})).unwrap();
        }
        assert!(enqueue(&mut q, json!({})).is_err());
    }
    #[test]
    fn executable_must_be_an_absolute_file() {
        assert!(executable_path("mpv").is_err());
        assert!(executable_path("").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn early_process_exit_reports_failure_instead_of_silent_close() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let file = root.join("tests/fixtures/failing-mpv.py");
        let player = Player::launch(file.to_string_lossy().into_owned(), file).unwrap();
        match player.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Error(e) => assert!(e.contains("启动失败") && e.contains('7'), "{e}"),
            _ => panic!("startup failure must be visible before Closed"),
        }
    }
    #[cfg(unix)]
    #[test]
    fn external_process_ipc_ready_seek_pause_and_shutdown() {
        let file = std::env::temp_dir().join(format!(
            "edenvoice-watch-test-{}-{}.mkv",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&file, b"test fixture, not a real video").unwrap();
        let executable =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-mpv.py");
        let player =
            Player::launch(executable.to_string_lossy().into_owned(), file.clone()).unwrap();
        match player.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Ready(info) => {
                assert_eq!(info.duration, 120.0);
                assert_eq!(info.size, 30);
            }
            Event::Error(e) => panic!("{e}"),
            _ => panic!("expected ready"),
        }
        player
            .commands
            .send(Control::Apply {
                position: Some(15.0),
                paused: false,
                speed: 1.0,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "seek/play not observed");
            if let Event::Sample {
                position, paused, ..
            } = player.events.recv_timeout(Duration::from_secs(1)).unwrap()
                && position >= 15.0
                && !paused
            {
                break;
            }
        }
        player
            .commands
            .send(Control::Apply {
                position: Some(42.0),
                paused: true,
                speed: 1.0,
            })
            .unwrap();
        loop {
            assert!(Instant::now() < deadline, "pause not observed");
            if let Event::Sample {
                position, paused, ..
            } = player.events.recv_timeout(Duration::from_secs(1)).unwrap()
                && (position - 42.0).abs() < 0.1
                && paused
            {
                break;
            }
        }
        player.commands.send(Control::Stop).unwrap();
        loop {
            if matches!(
                player.events.recv_timeout(Duration::from_secs(2)).unwrap(),
                Event::Closed
            ) {
                break;
            }
        }
        fs::remove_file(file).unwrap();
    }
}
