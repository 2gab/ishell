use termusiclib::common::const_unknown::{UNKNOWN_ALBUM, UNKNOWN_ARTIST, UNKNOWN_TITLE};
use termusiclib::config::SharedTuiSettings;
use termusiclib::track::MediaTypes;
use tui_realm_stdlib::components::Paragraph;
use tuirealm::{
    component::{AppComponent, Component},
    event::Event,
    props::{BorderType, Borders, HorizontalAlignment, TextModifiers, TextStatic, Title},
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
    pub(crate) const IDLE_TEXT: &str = "No track is playing";

    pub fn new<T: Into<TextStatic>>(config: &SharedTuiSettings, text: T) -> Self {
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
                .text(text.into()),
        }
    }
}

impl AppComponent<Msg, UserEvent> for NowPlaying {
    fn on(&mut self, _ev: &Event<UserEvent>) -> Option<Msg> {
        None
    }
}

impl Model {
    /// (Re)mount the [`NowPlaying`] widget with the currently playing track's info.
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
                    // fall back to the filename/id, same as the old "Currently Playing" toast did,
                    // instead of a blunt "Unknown Title" for untagged files.
                    let title = track
                        .title()
                        .map_or_else(|| track.id_str().into_owned(), Into::into);
                    let artist = track.artist().unwrap_or(UNKNOWN_ARTIST);
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
            .remount(
                Id::NowPlaying,
                Box::new(NowPlaying::new(&self.config_tui, text)),
                Vec::new(),
            )
            .ok();
    }
}
