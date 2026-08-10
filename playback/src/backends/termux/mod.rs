//! Playback backend for Android/Termux, driven through the `termux-media-player` CLI (part of
//! the `termux-api` Termux package, which itself talks to the separate Termux:API companion
//! Android app). This exists because `cpal`'s own Android backend needs a JNI/Activity context
//! that a plain Termux-launched process does not have — see
//! [`crate::backends::rusty::open_output_stream`].
//!
//! `termux-media-player` only exposes `play [file]` / `pause` / `stop` / `info`, so this backend
//! is deliberately limited compared to `rusty`/`mpv`:
//! - No seek support at all.
//! - Volume goes through the system-wide "music" audio stream (`termux-volume music <level>`,
//!   0..=max_volume, `max_volume` device-dependent), not a per-app one.
//! - No playback-speed control.
//! - No real gapless preloading (only one track can be loaded at a time); track transitions
//!   always go through the normal Eos -> play-next path.
//! - No push events: position and end-of-track are both inferred by polling `info` on an
//!   interval, rather than being reported immediately.

use std::io;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use termusiclib::config::ServerOverlay;
use termusiclib::track::{MediaTypes, Track};

use crate::{MediaInfo, PlayerCmd, PlayerProgress, PlayerTrait, Speed, Volume};

/// How often to poll `termux-media-player info` for position updates / end-of-track detection.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Upper bound on how long a single `termux-media-player` invocation may take before it is
/// killed and treated as a failure. Needed because if the Termux:API companion app is missing
/// or unpaired, these commands can hang indefinitely instead of erroring out quickly.
const CMD_TIMEOUT: Duration = Duration::from_secs(5);
/// `termux-media-player` has no notion of playback speed; report back a fixed "normal" (1.0x,
/// i.e. `10` in this crate's `Speed` unit) regardless of what is requested.
const UNSUPPORTED_SPEED: Speed = 10;

/// Whether the `termux-media-player` CLI binary is present on `PATH`.
///
/// This only confirms the CLI itself is installed (`pkg install termux-api`); actually using it
/// also requires the separate Termux:API companion Android app to be installed and paired. That
/// second dependency can't be cheaply checked up front, so it instead surfaces as command
/// failures/timeouts once actually used (logged once, not fatal — see [`player_thread`]).
pub fn is_available() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| dir.join("termux-media-player").is_file())
}

pub struct TermuxBackend {
    volume: Arc<AtomicU16>,
    speed: Speed,
    gapless: bool,
    command_tx: Sender<PlayerInternalCmd>,
    position: Arc<Mutex<Duration>>,
    total_duration: Arc<Mutex<Option<Duration>>>,
    media_title: Arc<Mutex<String>>,
    is_paused: Arc<AtomicBool>,
    /// Whether the "seek is not supported" warning has already been logged once, to avoid
    /// spamming the log on every seek key-press.
    seek_warning_logged: bool,
}

#[derive(Debug)]
enum PlayerInternalCmd {
    Play(String),
    Resume,
    Pause,
    Stop,
    /// Stop playback *and* immediately report [`PlayerCmd::Eos`] to let the playlist advance,
    /// as opposed to [`Self::Stop`] which is a deliberate "stop and stay stopped".
    Skip,
    Volume(Volume),
}

impl TermuxBackend {
    pub fn new(config: &ServerOverlay, cmd_tx: crate::PlayerCmdSender) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let volume = Arc::new(AtomicU16::new(config.settings.player.volume));
        let speed = config.settings.player.speed;
        let gapless = config.settings.player.gapless;
        let position = Arc::new(Mutex::new(Duration::default()));
        let total_duration = Arc::new(Mutex::new(None));
        let media_title = Arc::new(Mutex::new(String::new()));
        let is_paused = Arc::new(AtomicBool::new(false));

        let position_inside = position.clone();
        let media_title_inside = media_title.clone();
        let is_paused_inside = is_paused.clone();

        std::thread::Builder::new()
            .name("termux-media-player loop".into())
            .spawn(move || {
                player_thread(
                    &command_rx,
                    &cmd_tx,
                    &position_inside,
                    &is_paused_inside,
                    &media_title_inside,
                );
            })
            .expect("failed to spawn thread");

        Self {
            volume,
            speed,
            gapless,
            command_tx,
            position,
            total_duration,
            media_title,
            is_paused,
            seek_warning_logged: false,
        }
    }

    fn warn_seek_unsupported_once(&mut self) {
        if !self.seek_warning_logged {
            warn!(
                "Seek is not supported by the Termux audio backend (termux-media-player has no \
                 seek command) — ignoring."
            );
            self.seek_warning_logged = true;
        }
    }
}

fn track_to_string(track: &Track) -> String {
    match track.inner() {
        MediaTypes::Track(track_data) => track_data.path().to_string_lossy().to_string(),
        MediaTypes::Radio(radio_track_data) => radio_track_data.url().to_string(),
        MediaTypes::Podcast(podcast_track_data) => podcast_track_data.url().to_string(),
    }
}

/// Run `termux-media-player <args>`, killing it if it takes longer than [`CMD_TIMEOUT`]
/// (guarding against a missing/unpaired Termux:API companion app hanging the call instead of
/// erroring).
fn run_termux_media_player(args: &[&str]) -> io::Result<std::process::Output> {
    let mut child = Command::new("termux-media-player")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let start = Instant::now();
    loop {
        if let Some(_status) = child.try_wait()? {
            return child.wait_with_output();
        }
        if start.elapsed() >= CMD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "termux-media-player timed out (the Termux:API companion app might not be \
                 installed/paired)",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Fire-and-forget variant of [`run_termux_media_player`] for commands whose result we don't
/// need to inspect (play/pause/stop), logging failures instead of propagating them — mirrors
/// how the other backends' internal command handlers are infallible.
fn run_cmd_logged(args: &[&str]) {
    if let Err(err) = run_termux_media_player(args) {
        error!("termux-media-player {args:?} failed: {err}");
    }
}

/// Get the system's current max volume for the "music" stream, via `termux-volume`'s JSON
/// output. Falls back to `15`, the value observed on real devices and the de-facto standard
/// range for Android's `STREAM_MUSIC`, if the query fails for any reason.
fn music_stream_max_volume() -> u16 {
    const FALLBACK: u16 = 15;

    let output = match run_termux_media_player_volume(&[]) {
        Ok(output) => output,
        Err(err) => {
            warn!("termux-volume query failed ({err}), assuming max_volume={FALLBACK}");
            return FALLBACK;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(err) => {
            warn!("termux-volume returned unparseable JSON ({err}), assuming max_volume={FALLBACK}");
            return FALLBACK;
        }
    };

    parsed
        .as_array()
        .and_then(|streams| streams.iter().find(|s| s["stream"] == "music"))
        .and_then(|music| music["max_volume"].as_u64())
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(FALLBACK)
}

fn run_termux_media_player_volume(args: &[&str]) -> io::Result<std::process::Output> {
    let mut child = Command::new("termux-volume")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let start = Instant::now();
    loop {
        if let Some(_status) = child.try_wait()? {
            return child.wait_with_output();
        }
        if start.elapsed() >= CMD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "termux-volume timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn set_system_music_volume(volume: Volume) {
    let max = music_stream_max_volume();
    // scale our 0..=100 range onto the device's 0..=max_volume range
    let level = (u32::from(volume) * u32::from(max)).div_ceil(100);
    let level = level.to_string();
    if let Err(err) = run_termux_media_player_volume(&["music", level.as_str()]) {
        error!("termux-volume music {level} failed: {err}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteStatus {
    Playing,
    Paused,
}

struct InfoSnapshot {
    status: RemoteStatus,
    position: Duration,
}

/// Parse `termux-media-player info`'s plain-text output. Returns `None` for "No track
/// currently!", which is reported both after a deliberate `stop` and when a track finishes
/// playing on its own (there is no way to tell those two apart from `info` alone — callers
/// must track whether *they* just asked for a stop).
fn parse_info(stdout: &str) -> Option<InfoSnapshot> {
    if stdout.contains("No track currently") {
        return None;
    }

    let mut status = None;
    let mut position = None;

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Status:") {
            status = match rest.trim() {
                "Playing" => Some(RemoteStatus::Playing),
                "Paused" => Some(RemoteStatus::Paused),
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("Current Position:") {
            position = rest.trim().split('/').next().and_then(parse_mmss);
        }
    }

    Some(InfoSnapshot {
        status: status?,
        position: position?,
    })
}

/// Parse a `[[HH:]MM:]SS` timestamp, as used by `termux-media-player info`'s
/// "Current Position: MM:SS / MM:SS" line.
fn parse_mmss(s: &str) -> Option<Duration> {
    let mut secs: u64 = 0;
    for part in s.trim().split(':') {
        secs = secs.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    Some(Duration::from_secs(secs))
}

fn player_thread(
    picmd_rx: &Receiver<PlayerInternalCmd>,
    pcmd_tx: &crate::PlayerCmdSender,
    position: &Arc<Mutex<Duration>>,
    is_paused: &Arc<AtomicBool>,
    media_title: &Arc<Mutex<String>>,
) {
    // Whether we expect a track to currently be loaded in termux-media-player. Used to tell an
    // unexpected "No track currently!" (the track finished on its own, i.e. Eos) apart from one
    // we caused ourselves via a deliberate Stop.
    let mut expecting_track = false;
    // Avoid spamming the log with the same "info failed" message every single poll tick.
    let mut poll_error_logged = false;

    loop {
        match picmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(PlayerInternalCmd::Play(file)) => {
                expecting_track = true;
                *position.lock() = Duration::ZERO;
                is_paused.store(false, Ordering::SeqCst);
                run_cmd_logged(&["play", file.as_str()]);
                continue;
            }
            Ok(PlayerInternalCmd::Resume) => {
                is_paused.store(false, Ordering::SeqCst);
                run_cmd_logged(&["play"]);
                continue;
            }
            Ok(PlayerInternalCmd::Pause) => {
                is_paused.store(true, Ordering::SeqCst);
                run_cmd_logged(&["pause"]);
                continue;
            }
            Ok(PlayerInternalCmd::Stop) => {
                expecting_track = false;
                media_title.lock().clear();
                run_cmd_logged(&["stop"]);
                continue;
            }
            Ok(PlayerInternalCmd::Skip) => {
                expecting_track = false;
                media_title.lock().clear();
                run_cmd_logged(&["stop"]);
                let _ = pcmd_tx.send(PlayerCmd::Eos);
                continue;
            }
            Ok(PlayerInternalCmd::Volume(volume)) => {
                set_system_music_volume(volume);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if !expecting_track {
            continue;
        }

        match run_termux_media_player(&["info"]) {
            Ok(output) => {
                poll_error_logged = false;
                let stdout = String::from_utf8_lossy(&output.stdout);
                match parse_info(&stdout) {
                    Some(snapshot) => {
                        *position.lock() = snapshot.position;
                        is_paused.store(snapshot.status == RemoteStatus::Paused, Ordering::SeqCst);
                    }
                    None => {
                        expecting_track = false;
                        media_title.lock().clear();
                        let _ = pcmd_tx.send(PlayerCmd::Eos);
                    }
                }
            }
            Err(err) => {
                if !poll_error_logged {
                    error!(
                        "termux-media-player info failed: {err}. This can happen if the \
                         Termux:API companion app is not installed/paired. Position updates and \
                         end-of-track detection will not work until this succeeds again \
                         (retrying quietly)."
                    );
                    poll_error_logged = true;
                }
            }
        }
    }
}

#[async_trait]
impl PlayerTrait for TermuxBackend {
    async fn add_and_play(&mut self, track: &Track) {
        let file = track_to_string(track);
        *self.total_duration.lock() = track.duration();
        *self.media_title.lock() = track.title().unwrap_or_default().to_string();
        let _ = self.command_tx.send(PlayerInternalCmd::Play(file));
    }

    fn volume(&self) -> Volume {
        self.volume.load(Ordering::SeqCst)
    }

    fn set_volume(&mut self, volume: Volume) -> Volume {
        let volume = volume.min(100);
        self.volume.store(volume, Ordering::SeqCst);
        let _ = self.command_tx.send(PlayerInternalCmd::Volume(volume));

        volume
    }

    fn pause(&mut self) {
        let _ = self.command_tx.send(PlayerInternalCmd::Pause);
    }

    fn resume(&mut self) {
        let _ = self.command_tx.send(PlayerInternalCmd::Resume);
    }

    fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    fn seek(&mut self, _secs: i64) -> anyhow::Result<()> {
        // Must stay `Ok`: some callers (e.g. `Player::seek_relative`) `.expect()` this.
        self.warn_seek_unsupported_once();
        Ok(())
    }

    fn seek_to(&mut self, _position: Duration) {
        self.warn_seek_unsupported_once();
    }

    fn get_progress(&self) -> Option<PlayerProgress> {
        Some(PlayerProgress {
            position: Some(*self.position.lock()),
            total_duration: *self.total_duration.lock(),
        })
    }

    fn set_speed(&mut self, _speed: Speed) -> Speed {
        // playback-speed control is not supported by `termux-media-player`; always report
        // back "normal" speed rather than pretending a change took effect.
        UNSUPPORTED_SPEED
    }

    fn speed(&self) -> Speed {
        UNSUPPORTED_SPEED
    }

    fn stop(&mut self) {
        let _ = self.command_tx.send(PlayerInternalCmd::Stop);
    }

    fn gapless(&self) -> bool {
        self.gapless
    }

    fn set_gapless(&mut self, to: bool) {
        // stored for API compatibility, but `enqueue_next` below cannot act on it: only one
        // track can ever be loaded in termux-media-player at a time.
        self.gapless = to;
    }

    fn skip_one(&mut self) {
        let _ = self.command_tx.send(PlayerInternalCmd::Skip);
    }

    fn enqueue_next(&mut self, _track: &Track) {
        // No real preloading capability; the normal Eos -> add_and_play flow (driven by the
        // polling loop in `player_thread`) handles the transition once the current track
        // actually finishes.
    }

    fn media_info(&self) -> MediaInfo {
        let title = self.media_title.lock();
        if title.is_empty() {
            MediaInfo::default()
        } else {
            MediaInfo {
                media_title: Some(title.clone()),
            }
        }
    }
}
