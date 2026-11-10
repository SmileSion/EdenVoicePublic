use crate::mpv::{self, Player};
use edenvoice_client::{
    application::{AppCommand, Application},
    model::ConnectionState,
    protocol::watch::{WatchFile, WatchRequest, WatchState},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub managing: bool,
    pub can_control: bool,
    pub can_close: bool,
    pub can_transfer_host: bool,
    pub role: String,
    pub active: bool,
    pub loading: bool,
    pub ready: bool,
    pub can_reopen: bool,
    pub room_open: bool,
    pub channel_id: String,
    pub host_id: String,
    pub mpv_path: String,
    pub selected_file: String,
    pub error: String,
    pub status: String,
    pub position: f64,
    pub paused: bool,
    pub room_paused: bool,
    pub duration: f64,
    pub members: Vec<MemberView>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub session_id: String,
    pub ready: bool,
    pub file: Option<WatchFile>,
}
struct Active {
    channel: u64,
    session: u64,
    epoch: u64,
    revision: u64,
    player: Option<Player>,
    file: Option<WatchFile>,
    ready: bool,
    opened: Instant,
    last_send: Instant,
    last_sync: Instant,
    queue: VecDeque<WatchRequest>,
    pending: BTreeMap<u64, (Instant, String)>,
    latency: f64,
    sample: Option<(f64, bool, Instant)>,
    suppress_until: Instant,
    speed: f64,
    force_sync: bool,
}
pub struct Coordinator {
    active: Option<Active>,
    selected_path: Option<PathBuf>,
    next_request: u64,
    context: Option<(String, u64, u64)>,
    status_request: Option<(u64, Instant)>,
    room_epoch: u64,
    management_request: Option<(u64, Instant)>,
    pub view: View,
}
impl Default for Coordinator {
    fn default() -> Self {
        Self {
            active: None,
            selected_path: None,
            next_request: 0,
            context: None,
            status_request: None,
            room_epoch: 0,
            management_request: None,
            view: View {
                paused: true,
                room_paused: true,
                ..View::default()
            },
        }
    }
}
impl Coordinator {
    fn current(core: &Application) -> Option<(String, u64, u64)> {
        let s = core.state();
        if s.connection != ConnectionState::Connected || !s.watch_supported {
            return None;
        }
        let session = s.self_session?;
        let channel = s.users.get(&session)?.channel_id;
        (channel != 0).then(|| (core.server_address().to_string(), channel, session))
    }
    pub fn room(
        &mut self,
        core: &mut Application,
        action: &str,
        channel: u64,
        session: u64,
        target: Option<u64>,
    ) {
        self.tick_context(core);
        if Self::current(core).is_none_or(|(_, c, s)| c != channel || s != session) {
            self.view.error = "语音房间已改变，请重新打开观影面板".into();
            return;
        }
        match action {
            "status" => {
                if self.management_request.is_some()
                    || self.active.is_some()
                    || self
                        .status_request
                        .is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(3))
                {
                    return;
                }
                self.next_request += 1;
                self.status_request = Some((self.next_request, Instant::now()));
                core.dispatch(AppCommand::Watch(WatchRequest {
                    channel_id: channel,
                    request_id: self.next_request,
                    action: "status".into(),
                    ..Default::default()
                }));
            }
            "open" | "join" => {
                if self.management_request.is_some() || self.active.is_some() {
                    return;
                }
                let now = Instant::now();
                self.status_request = None;
                self.active = Some(Active {
                    channel,
                    session,
                    epoch: 0,
                    revision: 0,
                    player: None,
                    file: None,
                    ready: false,
                    opened: now,
                    last_send: now - Duration::from_secs(1),
                    last_sync: now,
                    queue: VecDeque::from([WatchRequest {
                        action: action.into(),
                        ..Default::default()
                    }]),
                    pending: BTreeMap::new(),
                    latency: 0.0,
                    sample: None,
                    suppress_until: now,
                    speed: 1.0,
                    force_sync: false,
                });
                self.view.active = true;
                self.view.loading = true;
                self.view.error.clear();
                self.view.status = "正在加入观影房间…".into();
            }
            "close" | "transfer_host" => {
                if !(if action == "close" {
                    self.view.can_close
                } else {
                    self.view.can_transfer_host
                }) {
                    self.view.error = "只有主持人和本频道管理员可以管理观影房间".into();
                    return;
                }
                if self.view.managing || self.management_request.is_some() {
                    return;
                }
                self.view.managing = true;
                let request = WatchRequest {
                    action: action.into(),
                    target_session_id: target.unwrap_or(0),
                    ..Default::default()
                };
                if let Some(a) = self.active.as_mut() {
                    if !a.queue.iter().any(|r| r.action == action) {
                        a.queue.push_back(request);
                    }
                } else {
                    self.next_request += 1;
                    self.status_request = None;
                    self.management_request = Some((self.next_request, Instant::now()));
                    core.dispatch(AppCommand::Watch(WatchRequest {
                        channel_id: channel,
                        epoch: self.room_epoch,
                        request_id: self.next_request,
                        ..request
                    }));
                }
            }
            _ => {}
        }
    }
    pub fn open(&mut self, core: &mut Application, file: String, channel: u64, session: u64) {
        self.tick_context(core);
        let Some(a) = self
            .active
            .as_mut()
            .filter(|a| a.channel == channel && a.session == session && a.epoch != 0)
        else {
            self.view.error = "请先开启或加入观影房间".into();
            return;
        };
        self.selected_path = Some(PathBuf::from(&file));
        self.view.selected_file = PathBuf::from(&file)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.view.error.clear();
        if let Err(e) = mpv::executable_path(core.mpv_path()) {
            self.view.error = e;
            return;
        }
        match Player::launch(core.mpv_path().into(), PathBuf::from(file)) {
            Err(e) => self.view.error = e,
            Ok(player) => {
                a.player = Some(player);
                a.file = None;
                a.ready = false;
                a.sample = None;
                a.speed = 1.0;
                a.opened = Instant::now();
                a.queue.retain(|r| {
                    !matches!(
                        r.action.as_str(),
                        "file" | "play" | "pause" | "seek" | "sync" | "unready"
                    )
                });
                a.queue.push_back(WatchRequest {
                    action: "unready".into(),
                    ..Default::default()
                });
                self.view.ready = false;
                self.view.loading = true;
                self.view.duration = 0.0;
                self.view.position = 0.0;
                self.view.status = "正在启动 mpv 并加载影片…".into();
            }
        }
    }
    pub fn reopen(&mut self, core: &mut Application, channel: u64, session: u64) {
        self.tick_context(core);
        if !self.active.as_ref().is_some_and(|a| {
            a.channel == channel && a.session == session && a.epoch != 0 && a.player.is_none()
        }) {
            return;
        }
        if let Some(path) = &self.selected_path {
            self.open(core, path.to_string_lossy().into_owned(), channel, session);
        } else {
            self.view.error = "请先选择本地影片".into();
        }
    }
    pub fn leave(&mut self, core: &mut Application) {
        if let Some(a) = self.active.take()
            && Self::current(core).is_some_and(|(_, c, s)| c == a.channel && s == a.session)
        {
            self.next_request += 1;
            core.dispatch(AppCommand::Watch(WatchRequest {
                channel_id: a.channel,
                epoch: a.epoch,
                request_id: self.next_request,
                action: "leave".into(),
                ..Default::default()
            }));
        }
        self.view.can_control = false;
        self.view.can_close = false;
        self.view.can_transfer_host = false;
        self.view.active = false;
        self.view.ready = false;
        self.view.loading = false;
        self.view.status = "已退出观影，语音保持连接".into();
        self.view.members.clear();
    }
    pub fn control(&mut self, action: &str, position: Option<f64>) {
        let Some(a) = self.active.as_mut().filter(|a| a.ready) else {
            return;
        };
        if action != "sync" && !self.view.can_control {
            self.view.error = "播放进度由主持人或管理员控制，请使用同步自己".into();
            return;
        }
        let p = position.unwrap_or(self.view.position);
        if !matches!(action, "play" | "pause" | "seek" | "sync")
            || !p.is_finite()
            || p < 0.0
            || p > self.view.duration
        {
            return;
        }
        if action == "sync" {
            a.force_sync = true;
        }
        queue_action(a, action, p);
        a.suppress_until = Instant::now() + Duration::from_millis(700);
    }
    fn tick_context(&mut self, core: &Application) {
        let context = Self::current(core);
        if context != self.context {
            self.active = None;
            self.selected_path = None;
            self.status_request = None;
            self.management_request = None;
            self.room_epoch = 0;
            self.view = View {
                paused: true,
                room_paused: true,
                channel_id: context
                    .as_ref()
                    .map(|(_, c, _)| c.to_string())
                    .unwrap_or_default(),
                mpv_path: core.mpv_path().into(),
                ..View::default()
            };
            self.context = context;
        }
        self.view.mpv_path = core.mpv_path().into();
    }
    pub fn tick(&mut self, core: &mut Application) {
        self.tick_context(core);
        for message in core.take_watch_messages() {
            self.receive(core, message);
        }
        if self
            .management_request
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(10))
        {
            self.management_request = None;
            self.view.error = "管理操作响应超时，请刷新房间状态后重试".into();
        }
        self.view.managing = self.management_request.is_some();
        self.view.can_reopen = false;
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let mut stopped = false;
        for _ in 0..32 {
            let Some(event) = a.player.as_ref().and_then(|p| p.events.try_recv().ok()) else {
                break;
            };
            match event {
                mpv::Event::Ready(file) => {
                    self.view.duration = file.duration;
                    a.file = Some(file.clone());
                    a.queue.push_back(WatchRequest {
                        action: "file".into(),
                        file: Some(file),
                        ready: true,
                        ..Default::default()
                    });
                }
                mpv::Event::Error(e) => {
                    self.view.error = e;
                    stopped = true;
                }
                mpv::Event::Closed => {
                    stopped = true;
                }
                mpv::Event::Sample {
                    position,
                    paused,
                    seek,
                } => {
                    let now = Instant::now();
                    if self.view.can_control
                        && a.ready
                        && now >= a.suppress_until
                        && a.pending.is_empty()
                        && a.queue.is_empty()
                        && let Some((old, was_paused, at)) = a.sample
                    {
                        let expected = old
                            + if was_paused {
                                0.0
                            } else {
                                now.duration_since(at).as_secs_f64() * a.speed
                            };
                        if paused != was_paused {
                            queue_action(a, if paused { "pause" } else { "play" }, position);
                        } else if seek || (position - expected).abs() > 0.8 {
                            queue_action(a, "seek", position);
                        }
                    }
                    a.sample = Some((position, paused, now));
                    self.view.position = position;
                    self.view.paused = paused;
                }
            }
        }
        if stopped {
            a.player = None;
            a.ready = false;
            a.file = None;
            a.sample = None;
            a.queue.retain(|r| {
                !matches!(
                    r.action.as_str(),
                    "file" | "play" | "pause" | "seek" | "sync" | "unready"
                )
            });
            a.queue.push_back(WatchRequest {
                action: "unready".into(),
                ..Default::default()
            });
            self.view.loading = false;
            self.view.ready = false;
            self.view.status = "播放器已关闭，点击重新打开影片继续观影".into();
        }
        self.view.can_reopen = self.selected_path.is_some() && a.epoch != 0 && a.player.is_none();
        let now = Instant::now();
        if a.epoch != 0
            && a.last_sync.elapsed() >= Duration::from_secs(2)
            && a.pending.is_empty()
            && a.queue.is_empty()
        {
            a.queue.push_back(WatchRequest {
                action: "sync".into(),
                ..Default::default()
            });
            a.last_sync = now;
        }
        if a.pending.is_empty()
            && a.last_send.elapsed() >= Duration::from_millis(80)
            && let Some(mut request) = a.queue.pop_front()
        {
            self.next_request += 1;
            request.channel_id = a.channel;
            request.epoch = a.epoch;
            request.request_id = self.next_request;
            a.pending
                .insert(request.request_id, (now, request.action.clone()));
            core.dispatch(AppCommand::Watch(request));
            a.last_send = now;
        }
        self.view.managing = a
            .queue
            .iter()
            .any(|r| matches!(r.action.as_str(), "close" | "transfer_host"))
            || a.pending
                .values()
                .any(|(_, action)| matches!(action.as_str(), "close" | "transfer_host"));
        if a.pending
            .values()
            .any(|(at, _)| at.elapsed() > Duration::from_secs(10))
        {
            self.view.error = "观影同步超时，请重新加入；语音不受影响".into();
            self.leave(core);
        }
    }
    fn receive(&mut self, core: &mut Application, message: WatchState) {
        if self
            .context
            .as_ref()
            .is_none_or(|(_, c, _)| *c != message.channel_id)
        {
            return;
        }
        if self.active.is_none()
            && self
                .management_request
                .is_some_and(|(id, _)| id == message.request_id)
        {
            self.management_request = None;
            self.view.error = message.error.clone();
            self.room_epoch = if message.action == "closed" {
                0
            } else {
                message.epoch
            };
            self.view.room_open = self.room_epoch != 0;
            self.view.room_paused = message.paused;
            self.view.host_id = message.host_id.to_string();
            apply_permissions(
                &mut self.view,
                &message,
                self.context.as_ref().map(|(_, _, id)| *id).unwrap_or(0),
            );
            self.set_members(if message.action == "closed" {
                vec![]
            } else {
                message.members.unwrap_or_default()
            });
            return;
        }
        if message.action == "status" {
            if self.active.is_none()
                && self
                    .status_request
                    .is_some_and(|(id, _)| id == message.request_id)
            {
                self.status_request = None;
                if !message.error.is_empty() {
                    self.view.error = message.error;
                    return;
                }
                self.room_epoch = message.epoch;
                apply_permissions(
                    &mut self.view,
                    &message,
                    self.context.as_ref().map(|(_, _, id)| *id).unwrap_or(0),
                );
                self.view.room_open = message.epoch != 0;
                self.view.room_paused = message.paused;
                self.view.host_id = message.host_id.to_string();
                self.set_members(message.members.unwrap_or_default());
            }
            return;
        }
        let Some(a) = self.active.as_mut() else {
            return;
        };
        if matches!(message.action.as_str(), "closed" | "ended")
            && a.epoch != 0
            && message.epoch == a.epoch
            && (message.action == "closed" || message.actor_id == a.session)
        {
            self.active = None;
            self.view.active = false;
            self.view.ready = false;
            self.view.loading = false;
            self.view.room_open = false;
            self.room_epoch = 0;
            self.view.can_control = false;
            self.view.can_close = false;
            self.view.can_transfer_host = false;
            self.view.members.clear();
            self.view.status = "观影房间已关闭，语音保持连接".into();
            return;
        }
        let own = message.actor_id == a.session
            || (!message.error.is_empty() && a.pending.contains_key(&message.request_id));
        let pending = if own {
            a.pending.remove(&message.request_id)
        } else {
            None
        };
        if !message.error.is_empty() {
            if let Some((_, action)) = pending {
                if action == "file" {
                    a.player = None;
                    a.file = None;
                    a.ready = false;
                    a.sample = None;
                    self.view.ready = false;
                    self.view.loading = false;
                }
                apply_permissions(
                    &mut self.view,
                    &message,
                    self.context.as_ref().map(|(_, _, id)| *id).unwrap_or(0),
                );
                self.view.error = message.error;
                if a.epoch == 0
                    || message.epoch != a.epoch
                    || !message
                        .members
                        .as_ref()
                        .is_some_and(|members| members.iter().any(|m| m.session_id == a.session))
                {
                    self.leave(core);
                }
            }
            return;
        }
        if a.epoch == 0 {
            if !own || !matches!(message.action.as_str(), "open" | "join") || pending.is_none() {
                return;
            }
            a.epoch = message.epoch;
        }
        if message.epoch != a.epoch
            || message.revision < a.revision
            || !message.position.is_finite()
            || message.position < 0.0
            || message.position > 604800.0
        {
            return;
        }
        a.revision = message.revision;
        self.view.room_paused = message.paused;
        self.room_epoch = message.epoch;
        apply_permissions(
            &mut self.view,
            &message,
            self.context.as_ref().map(|(_, _, id)| *id).unwrap_or(0),
        );
        if !self.view.can_control {
            a.queue
                .retain(|r| !matches!(r.action.as_str(), "play" | "pause" | "seek"));
        }
        if let Some((at, action)) = pending {
            if action != "sync" || a.force_sync {
                self.view.error.clear();
            }
            a.latency = (at.elapsed().as_secs_f64() / 2.0).min(1.0);
        }
        let members = message.members.unwrap_or_default();
        // A response for a previous file must not mark a replacement player ready.
        a.ready = a.player.is_some()
            && a.file.is_some()
            && members
                .iter()
                .any(|m| m.session_id == a.session && m.ready && m.file == a.file);
        self.view.room_open = true;
        self.view.host_id = message.host_id.to_string();
        self.view.ready = a.ready;
        self.view.loading = a.player.is_some() && !a.ready;
        if a.player.is_none() && self.view.error.is_empty() {
            self.view.status = if self.selected_path.is_some() {
                "点击重新打开影片，继续同步观看"
            } else {
                "已加入观影房间，请选择本地影片"
            }
            .into();
        }
        if a.ready {
            self.view.status = if message.paused {
                "已暂停"
            } else {
                "正在播放"
            }
            .into();
        }
        self.view.members = members
            .into_iter()
            .map(|m| MemberView {
                session_id: m.session_id.to_string(),
                ready: m.ready,
                file: m.file,
            })
            .collect();
        if !a.ready
            || !(matches!(message.action.as_str(), "play" | "pause" | "seek" | "sync")
                || (own && message.action == "file"))
        {
            return;
        }
        let target = (message.position + if message.paused { 0.0 } else { a.latency })
            .min(self.view.duration);
        let (local, paused) = a
            .sample
            .map(|(p, paused, at)| {
                (
                    p + if paused {
                        0.0
                    } else {
                        at.elapsed().as_secs_f64() * a.speed
                    },
                    paused,
                )
            })
            .unwrap_or((0.0, true));
        let correction = correction(
            target - local,
            message.paused,
            message.action == "seek"
                || (own
                    && (message.action == "file" || (message.action == "sync" && a.force_sync))),
        );
        if own && message.action == "sync" {
            a.force_sync = false;
        }
        if correction.0 || paused != message.paused || (correction.1 - a.speed).abs() > 0.001 {
            if a.player.as_ref().is_some_and(|p| {
                p.commands
                    .try_send(mpv::Control::Apply {
                        position: correction.0.then_some(target),
                        paused: message.paused,
                        speed: correction.1,
                    })
                    .is_err()
            }) {
                self.view.error = "播放器控制队列已满，请重新打开影片".into();
                self.leave(core);
                return;
            }
            a.speed = correction.1;
            a.suppress_until = Instant::now() + Duration::from_millis(700);
            a.sample = None;
        }
    }
    fn set_members(&mut self, members: Vec<edenvoice_client::protocol::watch::WatchMember>) {
        self.view.members = members
            .into_iter()
            .map(|m| MemberView {
                session_id: m.session_id.to_string(),
                ready: m.ready,
                file: m.file,
            })
            .collect();
    }
}
fn apply_permissions(view: &mut View, state: &WatchState, session: u64) {
    if let Some(p) = &state.permissions {
        view.can_control = p.can_control;
        view.can_close = p.can_close;
        view.can_transfer_host = p.can_transfer_host;
        view.role = p.role.clone();
    } else {
        // Preserve host controls on older servers without inventing admin rights.
        let host = session != 0
            && state.host_id == session
            && state.epoch != 0
            && !matches!(state.action.as_str(), "closed" | "ended");
        view.can_control = host;
        view.can_close = host;
        view.can_transfer_host = false;
        view.role = if host { "host" } else { "viewer" }.into();
    }
}
fn queue_action(a: &mut Active, action: &str, position: f64) {
    a.queue
        .retain(|r| !matches!(r.action.as_str(), "play" | "pause" | "seek" | "sync"));
    a.queue.push_back(WatchRequest {
        action: action.into(),
        position,
        ..Default::default()
    });
}
fn correction(drift: f64, paused: bool, force: bool) -> (bool, f64) {
    if force || drift.abs() > 1.5 || (paused && drift.abs() > 0.15) {
        return (true, 1.0);
    }
    if !paused && drift.abs() > 0.3 {
        (false, if drift > 0.0 { 1.02 } else { 0.98 })
    } else {
        (false, 1.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drift_has_deadband_and_bounded_correction() {
        assert_eq!(correction(0.1, false, false), (false, 1.0));
        assert_eq!(correction(0.5, false, false), (false, 1.02));
        assert_eq!(correction(-0.5, false, false), (false, 0.98));
        assert_eq!(correction(2.0, false, false), (true, 1.0));
        assert_eq!(correction(0.2, true, false), (true, 1.0));
    }
}

#[cfg(test)]
mod moderation_tests {
    use super::*;
    use edenvoice_client::protocol::watch::{WatchMember, WatchPermissions};

    fn state(action: &str, request_id: u64, can_manage: bool) -> WatchState {
        WatchState {
            channel_id: 7,
            epoch: 3,
            revision: 4,
            host_id: 2,
            actor_id: 1,
            action: action.into(),
            request_id,
            paused: true,
            members: Some(vec![
                WatchMember {
                    session_id: 1,
                    ..Default::default()
                },
                WatchMember {
                    session_id: 2,
                    ..Default::default()
                },
            ]),
            permissions: Some(WatchPermissions {
                role: "channel_admin".into(),
                can_control: can_manage,
                can_close: can_manage,
                can_transfer_host: can_manage,
            }),
            ..Default::default()
        }
    }
    fn coordinator() -> Coordinator {
        Coordinator {
            context: Some(("test".into(), 7, 1)),
            ..Default::default()
        }
    }
    fn active() -> Active {
        let now = Instant::now();
        Active {
            channel: 7,
            session: 1,
            epoch: 3,
            revision: 1,
            player: None,
            file: None,
            ready: true,
            opened: now,
            last_send: now,
            last_sync: now,
            queue: VecDeque::new(),
            pending: BTreeMap::new(),
            latency: 0.0,
            sample: None,
            suppress_until: now,
            speed: 1.0,
            force_sync: false,
        }
    }
    #[test]
    fn administrator_can_discover_and_receive_close_without_player_or_membership() {
        let mut c = coordinator();
        let mut core = Application::new();
        c.status_request = Some((10, Instant::now()));
        c.receive(&mut core, state("status", 9, true));
        assert!(!c.view.room_open);
        let mut playing = state("status", 10, true);
        playing.paused = false;
        c.receive(&mut core, playing);
        assert!(!c.view.room_paused);
        assert!(c.view.paused);
        assert!(c.view.room_open && c.view.can_close);
        assert_eq!(c.room_epoch, 3);
        assert!(c.active.is_none());
        c.management_request = Some((11, Instant::now()));
        c.receive(&mut core, state("closed", 11, false));
        assert!(!c.view.room_open && !c.view.can_close && c.view.members.is_empty());
        assert!(c.management_request.is_none() && c.active.is_none());
    }
    #[test]
    fn transfer_revokes_queued_global_controls_but_keeps_local_sync() {
        let mut c = coordinator();
        let mut core = Application::new();
        let mut a = active();
        for action in ["play", "seek", "sync"] {
            a.queue.push_back(WatchRequest {
                action: action.into(),
                ..Default::default()
            });
        }
        c.active = Some(a);
        c.receive(&mut core, state("transfer_host", 12, false));
        let a = c.active.as_ref().unwrap();
        assert!(!c.view.can_control && !c.view.can_close);
        assert_eq!(a.queue.len(), 1);
        assert_eq!(a.queue[0].action, "sync");
    }
    #[test]
    fn denied_control_preserves_membership_and_updates_permissions() {
        let mut c = coordinator();
        let mut core = Application::new();
        let mut a = active();
        a.pending.insert(12, (Instant::now(), "play".into()));
        c.active = Some(a);
        let mut response = state("play", 12, false);
        response.error = "permission denied".into();
        c.receive(&mut core, response);
        assert!(c.active.is_some());
        assert!(!c.view.can_control);
        assert_eq!(c.view.error, "permission denied");
    }
    #[test]
    fn viewer_cannot_enqueue_global_controls_but_can_request_force_sync() {
        let mut c = coordinator();
        c.active = Some(active());
        c.view.duration = 100.0;
        c.control("play", Some(10.0));
        c.control("seek", Some(10.0));
        assert!(c.active.as_ref().unwrap().queue.is_empty());
        c.control("sync", None);
        let a = c.active.as_ref().unwrap();
        assert!(a.force_sync);
        assert_eq!(a.queue[0].action, "sync");
    }
    #[test]
    fn old_server_keeps_host_controls_without_granting_moderation() {
        let mut view = View::default();
        let mut response = state("sync", 1, false);
        response.permissions = None;
        apply_permissions(&mut view, &response, 2);
        assert!(view.can_control && view.can_close && !view.can_transfer_host);
        apply_permissions(&mut view, &response, 1);
        assert!(!view.can_control && !view.can_close);
    }
}
