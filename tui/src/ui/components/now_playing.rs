use termusiclib::common::const_unknown::{UNKNOWN_ALBUM, UNKNOWN_ARTIST, UNKNOWN_TITLE};
use termusiclib::config::SharedTuiSettings;
use termusiclib::track::MediaTypes;
use tui_realm_stdlib::components::Paragraph;
use tuirealm::{
    component::{AppComponent, Component},
    event::Event,
    props::{Attribute, AttrValue, BorderType, Borders, HorizontalAlignment, TextModifiers, Title},
};

use crate::ui::components::database::Matchable;
use crate::ui::ids::Id;
use crate::ui::model::{Model, UserEvent};
use crate::ui::msg::Msg;

/// Persistent "Now Playing" info (title / artist / album), independent from the Lyric panel.
#[derive(Component)]
pub struct NowPlaying {
    component: Paragraph,
}

impl NowPlaying {
    pub fn new(config: &SharedTuiSettings) -> Self {
        let config_tui = config.read_recursive();
        Self {
            component: Paragraph::default()
                .borders(
                    Borders::default()
                        .color(config_tui.settings.theme.library_border())
                        .modifiers(BorderType::Rounded),
                )
                .foreground(config_tui.settings.theme.library_foreground())
                .background(config_tui.settings.theme.library_background())
                .modifiers(TextModifiers::BOLD)
                .alignment_horizontal(HorizontalAlignment::Center)
                .title(Title::from(" Now Playing ").alignment(HorizontalAlignment::Center))
                .text(Self::IDLE_TEXT),
        }
    }

    const IDLE_TEXT: &str = "No track is playing";
}

impl AppComponent<Msg, UserEvent> for NowPlaying {
    fn on(&mut self, _ev: &Event<UserEvent>) -> Option<Msg> {
        None
    }
}

impl Model {
    /// Update the [`NowPlaying`] widget with the currently playing track's info.
    ///
    /// Needs to be run on:
    /// - running status change
    /// - track change
    pub fn now_playing_update(&mut self) {
        let track = self.playback.current_track();

        let text = if self.playback.is_stopped() || track.is_none() {
            NowPlaying::IDLE_TEXT.to_string()
        } else {
            let track = track.unwrap();
            match track.inner() {
                MediaTypes::Track(_track_data) => {
                    let artist = track.artist().unwrap_or(UNKNOWN_ARTIST);
                    let title = track.title().unwrap_or(UNKNOWN_TITLE);
                    let album = track.meta_album().unwrap_or(UNKNOWN_ALBUM);
                    format!("{title} — {artist} — {album}")
                }
                MediaTypes::Radio(_radio_track_data) => "Live Radio".to_string(),
                MediaTypes::Podcast(_podcast_track_data) => {
                    track.title().unwrap_or(UNKNOWN_TITLE).to_string()
                }
            }
        };

        self.app
            .attr(&Id::NowPlaying, Attribute::Text, AttrValue::String(text))
            .ok();
    }
}
