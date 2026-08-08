use std::sync::mpsc::{self, Receiver, RecvError, Sender};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use termusiclib::common::const_unknown::{UNKNOWN_ARTIST, UNKNOWN_TITLE};
use termusiclib::track::Track;

use crate::PlayerTimeUnit;

const APP_ID: &str = "1535632224899170417";

/// Handle for communicating with the discord ipc client
#[derive(Debug)]
pub struct Rpc {
    tx: Sender<RpcCommand>,
}

enum RpcCommand {
    Update(String, String, Option<Duration>),
    Resume(i64),
    Pause,
    Stop,
}

impl Default for Rpc {
    fn default() -> Self {
        let client = DiscordIpcClient::new(APP_ID);
        let (tx, rx): (Sender<RpcCommand>, Receiver<RpcCommand>) = mpsc::channel();

        std::thread::Builder::new()
            .name("discord rpc loop".into())
            .spawn(|| Self::thread_fn(client, rx))
            .expect("failed to start discord rpc loop thread");

        Self { tx }
    }
}

impl Rpc {
    /// Update the discord status track information.
    pub fn set_track(&self, track: &Track) {
        let artist = track.artist().unwrap_or(UNKNOWN_ARTIST).to_string();
        let title = track.title().unwrap_or(UNKNOWN_TITLE).to_string();
        self.tx
            .send(RpcCommand::Update(artist, title, track.duration()))
            .ok();
    }

    /// Update the discord status to show that it is paused.
    pub fn pause(&self) {
        self.tx.send(RpcCommand::Pause).ok();
    }

    /// Update the discord status to show that it is playing.
    pub fn resume(&self, time_pos: Option<PlayerTimeUnit>) {
        // ignore clippy here, this should not be a problem, maybe rich presence will support duration in the future
        #[allow(clippy::cast_possible_wrap)]
        if let Some(time_pos) = time_pos {
            self.tx
                .send(RpcCommand::Resume(time_pos.as_secs().cast_signed()))
                .ok();
        }
    }

    /// Update the discord status to show that it is stopped.
    pub fn stop(&self) {
        self.tx.send(RpcCommand::Stop).ok();
    }

    /// This function actually communicates with the discord client and is meant to run in its own thread.
    #[allow(clippy::needless_pass_by_value)]
    fn thread_fn(mut client: DiscordIpcClient, rx: Receiver<RpcCommand>) {
        let mut artist = String::new();
        let mut title = String::new();
        // Remembered from the last `Update`, so `Resume` (which doesn't get told the duration
        // again) can still draw the same progress bar.
        let mut duration: Option<Duration> = None;

        loop {
            let msg = match rx.recv() {
                Err(RecvError) => {
                    info!("No senders for discord updates anymore, closing discord connection");
                    break;
                }
                Ok(v) => v,
            };

            if !reconnect(&mut client) {
                // if connecting to the discord rpc fails, ignore the current command

                // likely for better status we should keep a state and try to reconnect, but also still handle all the commands send here
                continue;
            }

            match msg {
                RpcCommand::Update(artist_cmd, title_cmd, duration_cmd) => {
                    artist = artist_cmd;
                    title = title_cmd;
                    duration = duration_cmd;

                    client
                        .set_activity(
                            activity::Activity::new()
                                .activity_type(activity::ActivityType::Listening)
                                .assets(ishell_assets())
                                .timestamps(listening_timestamps(now_epoch_secs(), duration))
                                .state(&artist)
                                .details(&title),
                        )
                        .ok();
                }
                RpcCommand::Pause => {
                    client
                        .set_activity(
                            activity::Activity::new()
                                .activity_type(activity::ActivityType::Listening)
                                .assets(ishell_assets())
                                .state(&artist)
                                .details(format!("{}: Paused", title.as_str()).as_str()),
                        )
                        .ok();
                }
                RpcCommand::Resume(time_pos) => {
                    let start = now_epoch_secs() - time_pos;

                    client
                        .set_activity(
                            activity::Activity::new()
                                .activity_type(activity::ActivityType::Listening)
                                .assets(ishell_assets())
                                .timestamps(listening_timestamps(start, duration))
                                .state(&artist)
                                .details(&title),
                        )
                        .ok();
                }
                RpcCommand::Stop => {
                    title.clear();
                    artist.clear();
                    duration = None;

                    client
                        .set_activity(
                            activity::Activity::new()
                                .activity_type(activity::ActivityType::Listening)
                                .assets(ishell_assets())
                                .state(&artist)
                                .details("Stopped"),
                        )
                        .ok();
                }
            }
        }
    }
}

/// The bundled `ishell` icon, shown as the activity's large image for every state.
fn ishell_assets() -> activity::Assets<'static> {
    activity::Assets::new().large_image("ishell")
}

/// `start`, plus `end` if `duration` is known — Discord draws a Spotify-style progress bar for
/// `ActivityType::Listening` activities when both timestamps are present, and falls back to a
/// plain elapsed-time counter when only `start` is set.
#[allow(clippy::cast_possible_wrap)]
fn listening_timestamps(start: i64, duration: Option<Duration>) -> activity::Timestamps {
    let timestamps = activity::Timestamps::new().start(start);
    match duration {
        Some(duration) => timestamps.end(start + duration.as_secs() as i64),
        None => timestamps,
    }
}

/// Current unix time in seconds, as the `i64` the Discord IPC protocol expects.
fn now_epoch_secs() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    i64::try_from(secs).unwrap_or_else(|_| {
        warn!("SystemTime to i64 failed, discord interface can't handle this number");
        0
    })
}

const RETRIES: u8 = 3;

/// Try to connect the given client, with [`RETRIES`] amount of retries.
///
/// Returns `true` if connected, `false` otherwise
fn reconnect(client: &mut DiscordIpcClient) -> bool {
    let mut tries = 0;

    while tries < RETRIES {
        tries += 1;
        if client.connect().is_ok() {
            return true;
        }
        sleep(Duration::from_secs(2));
    }

    false
}
