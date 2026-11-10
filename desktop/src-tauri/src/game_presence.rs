use edenvoice_client::{application::Application, model::ConnectionState};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(windows)]
#[path = "game_presence_windows.rs"]
mod windows;
#[cfg(not(windows))]
mod windows {
    pub fn processes() -> Result<(Vec<(u32, String)>, u32), ()> {
        Err(())
    }
}

use edenvoice_client::game_catalog::{Catalog, Game};
fn bundled() -> Vec<Game> {
    let mut data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../server/internal/protocol/game_catalog.json"
    ))
    .expect("bundled catalog");
    for g in data.as_array_mut().unwrap() {
        g["enabled"] = true.into();
    }
    serde_json::from_value(data).expect("bundled games")
}
#[cfg(test)]
fn identify(processes: &[(u32, String)], foreground: u32, current: Option<&str>) -> Option<String> {
    identify_with(&bundled(), processes, foreground, current)
}
fn identify_with(
    games: &[Game],
    processes: &[(u32, String)],
    foreground: u32,
    current: Option<&str>,
) -> Option<String> {
    let matches: Vec<_> = games
        .iter()
        .filter(|game| game.enabled)
        .filter_map(|game| {
            let pids: Vec<_> = processes
                .iter()
                .filter(|(_, name)| {
                    game.executables
                        .iter()
                        .any(|exe| exe.eq_ignore_ascii_case(name))
                })
                .map(|(pid, _)| *pid)
                .collect();
            (!pids.is_empty()).then_some((game, pids))
        })
        .collect();
    matches
        .iter()
        .find(|(_, pids)| pids.contains(&foreground))
        .or_else(|| {
            matches
                .iter()
                .find(|(game, _)| Some(game.id.as_str()) == current)
        })
        .or_else(|| matches.first())
        .map(|(game, _)| game.id.clone())
}

// Process enumeration never runs on the core/audio thread. A single replaceable
// result bounds memory; generations reject scans completed after disabling.
type PresenceResult = (u64, Option<String>);

pub struct Tracker {
    catalog: Arc<Mutex<Arc<Vec<Game>>>>,
    catalog_version: Option<Catalog>,
    generation: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<PresenceResult>>>,
    enabled: bool,
    context: Option<(String, u64)>,
    sent: Option<Option<String>>,
    last_sent: Instant,
}
impl Tracker {
    pub fn new() -> Self {
        let catalog = Arc::new(Mutex::new(Arc::new(bundled())));
        let generation = Arc::new(AtomicU64::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(None));
        if cfg!(windows) {
            let catalog = catalog.clone();
            let (generation, stopped, latest) =
                (generation.clone(), stopped.clone(), latest.clone());
            let _ = std::thread::Builder::new()
                .name("eden-game-presence".into())
                .spawn(move || {
                    let mut last_generation = 0;
                    let mut next = Instant::now();
                    let mut current = None;
                    while !stopped.load(Ordering::Acquire) {
                        let epoch = generation.load(Ordering::Acquire);
                        if epoch != last_generation {
                            next = Instant::now();
                            current = None;
                            last_generation = epoch;
                        }
                        if epoch % 2 == 1 && Instant::now() >= next {
                            // A failed enumeration clears presence instead of retaining stale activity.
                            current =
                                windows::processes()
                                    .ok()
                                    .and_then(|(processes, foreground)| {
                                        catalog.lock().ok().map(|games| games.clone()).and_then(
                                            |games| {
                                                identify_with(
                                                    &games,
                                                    &processes,
                                                    foreground,
                                                    current.as_deref(),
                                                )
                                            },
                                        )
                                    });
                            if generation.load(Ordering::Acquire) == epoch
                                && let Ok(mut result) = latest.lock()
                            {
                                *result = Some((epoch, current.clone()));
                            }
                            next = Instant::now() + Duration::from_secs(5);
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
        }
        Self {
            catalog,
            catalog_version: None,
            generation,
            stopped,
            latest,
            enabled: false,
            context: None,
            sent: None,
            last_sent: Instant::now(),
        }
    }
    pub fn tick(&mut self, core: &Application) {
        let connected = core.state().connection == ConnectionState::Connected
            && core.state().game_activity_supported;
        let context = if connected {
            core.state()
                .self_session
                .map(|id| (core.server_address().to_owned(), id))
        } else {
            None
        };
        let enabled = cfg!(windows)
            && core.share_game_activity()
            && context.is_some()
            && (!core.state().game_catalog_required || core.state().game_catalog.is_some());
        let catalog_changed = self.catalog_version.as_ref().map(|c| c.version)
            != core.state().game_catalog.as_ref().map(|c| c.version);
        if self.context != context || catalog_changed {
            self.catalog_version = core.state().game_catalog.clone();
            if let Ok(mut games) = self.catalog.lock() {
                *games = Arc::new(
                    self.catalog_version
                        .as_ref()
                        .map(|c| c.games.clone())
                        .unwrap_or_else(bundled),
                );
            }
            self.context = context;
            self.sent = None;
            // Force a fresh result after every connection change, including reconnects.
            if self.enabled {
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.enabled = false;
            }
        }
        if enabled != self.enabled {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.enabled = enabled;
            self.sent = None;
        }
        if self.context.is_none() {
            return;
        }
        let desired = if !enabled {
            Some(None)
        } else {
            let epoch = self.generation.load(Ordering::Acquire);
            self.latest.try_lock().ok().and_then(|result| {
                result
                    .as_ref()
                    .filter(|(generation, _)| *generation == epoch)
                    .map(|(_, id)| id.clone())
            })
        };
        let Some(desired) = desired else {
            return;
        };
        if (self.sent.as_ref() != Some(&desired)
            || (desired.is_some() && self.last_sent.elapsed() >= Duration::from_secs(20)))
            && core
                .publish_game_activity(desired.clone().unwrap_or_default())
                .is_ok()
        {
            self.sent = Some(desired);
            self.last_sent = Instant::now();
        }
    }
}
impl Drop for Tracker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_rules_replace_embedded_rules_and_disable_detection() {
        let mut games = bundled();
        let processes = vec![(1, "newgame.exe".into())];
        assert_eq!(identify_with(&games, &processes, 1, None), None);
        games[0].executables = vec!["newgame.exe".into()];
        assert_eq!(
            identify_with(&games, &processes, 1, None),
            Some("cs2".into())
        );
        games[0].enabled = false;
        assert_eq!(identify_with(&games, &processes, 1, Some("cs2")), None);
    }
    #[test]
    fn exact_names_only_and_no_launchers() {
        assert_eq!(
            identify(
                &[
                    (1, "Steam.exe".into()),
                    (2, "javaw.exe".into()),
                    (3, "cs2.exe.bak".into())
                ],
                1,
                None
            ),
            None
        );
    }
    #[test]
    fn foreground_then_current_and_clear_on_exit() {
        let processes = vec![(1, "CS2.EXE".into()), (2, "dota2.exe".into())];
        assert_eq!(identify(&processes, 2, Some("cs2")), Some("dota2".into()));
        assert_eq!(identify(&processes, 99, Some("cs2")), Some("cs2".into()));
        assert_eq!(identify(&[], 99, Some("cs2")), None);
    }
}
