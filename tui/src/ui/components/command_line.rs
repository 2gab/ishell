use termusiclib::config::SharedTuiSettings;
use tui_realm_stdlib::components::Input;
use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers};
use tuirealm::props::{BorderSides, Borders, InputType, Style};
use tuirealm::state::{State, StateValue};

use crate::ui::ids::Id;
use crate::ui::model::{Panel, UserEvent};
use crate::ui::msg::{CommandLineMsg, Msg};

/// The vim-like `:` command-line input, used to configure which panels are shown.
#[derive(Component)]
pub struct CommandLine {
    component: Input,
}

impl CommandLine {
    pub fn new(config: &SharedTuiSettings) -> Self {
        let config = config.read_recursive();
        Self {
            component: Input::default()
                .foreground(config.settings.theme.fallback_foreground())
                .background(config.settings.theme.fallback_background())
                .inactive(Style::new().bg(config.settings.theme.fallback_background()))
                .borders(Borders::default().sides(BorderSides::NONE))
                .input_type(InputType::Text),
        }
    }
}

impl AppComponent<Msg, UserEvent> for CommandLine {
    fn on(&mut self, ev: &Event<UserEvent>) -> Option<Msg> {
        let cmd_result = match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Left, ..
            }) => self.perform(Cmd::Move(Direction::Left)),
            Event::Keyboard(KeyEvent {
                code: Key::Right, ..
            }) => self.perform(Cmd::Move(Direction::Right)),
            Event::Keyboard(KeyEvent {
                code: Key::Home, ..
            }) => self.perform(Cmd::GoTo(Position::Begin)),
            Event::Keyboard(KeyEvent { code: Key::End, .. }) => {
                self.perform(Cmd::GoTo(Position::End))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Delete, ..
            }) => self.perform(Cmd::Cancel),
            Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                ..
            }) => self.perform(Cmd::Delete),
            Event::Keyboard(KeyEvent {
                code: Key::Char(ch),
                modifiers: KeyModifiers::SHIFT | KeyModifiers::NONE,
            }) => self.perform(Cmd::Type(*ch)),
            Event::Keyboard(KeyEvent { code: Key::Esc, .. }) => {
                return Some(Msg::CommandLine(CommandLineMsg::Close));
            }
            Event::Keyboard(KeyEvent {
                code: Key::Enter, ..
            }) => self.perform(Cmd::Submit),
            _ => CmdResult::NoChange,
        };

        match cmd_result {
            CmdResult::Submit(State::Single(StateValue::String(input))) => {
                Some(Msg::CommandLine(CommandLineMsg::Submit(input)))
            }
            CmdResult::Submit(_) => Some(Msg::CommandLine(CommandLineMsg::Submit(String::new()))),
            CmdResult::NoChange => None,
            _ => Some(Msg::ForceRedraw),
        }
    }
}

impl crate::ui::model::Model {
    /// Mount the `:` command-line input and give it focus.
    pub fn mount_command_line(&mut self) {
        self.app
            .remount(
                Id::CommandLine,
                Box::new(CommandLine::new(&self.config_tui)),
                Vec::new(),
            )
            .expect("Expected to mount CommandLine without error");
        self.app.active(&Id::CommandLine).ok();
    }

    /// Close the `:` command-line input.
    pub fn umount_command_line(&mut self) {
        self.app.umount(&Id::CommandLine).ok();
    }

    /// Handle a [`CommandLineMsg`].
    pub fn update_command_line_msg(&mut self, msg: CommandLineMsg) {
        match msg {
            CommandLineMsg::Show => self.mount_command_line(),
            CommandLineMsg::Close => self.umount_command_line(),
            CommandLineMsg::Submit(input) => {
                self.umount_command_line();
                let trimmed = input.trim();
                if trimmed == "?" || trimmed.eq_ignore_ascii_case("help") {
                    self.mount_help_popup();
                } else if let Err(err) = self.apply_layout_command(&input) {
                    self.mount_error_popup(anyhow::anyhow!(err));
                }
            }
        }
    }

    /// Parse and apply a `:` layout command (e.g. `player`, `player+library`, `all`).
    ///
    /// Recognized tokens (combine with `+`): `player` (now-playing + progress), `playlist`,
    /// `lyric`, `library` (the left sidebar), and `all` (reset to everything). Each command
    /// replaces the current set of visible regions entirely. An empty command is a no-op.
    pub fn apply_layout_command(&mut self, input: &str) -> Result<(), String> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        if input.eq_ignore_ascii_case("all") {
            self.visible_panels = Panel::ALL.to_vec();
            self.show_sidebar = true;
            self.ensure_focus_visible();
            return Ok(());
        }

        let mut requested: Vec<Panel> = Vec::new();
        let mut show_sidebar = false;
        for token in input.split('+') {
            match token.trim().to_ascii_lowercase().as_str() {
                "player" => requested.extend([Panel::NowPlaying, Panel::Progress]),
                "playlist" => requested.push(Panel::Playlist),
                "lyric" | "lyrics" => requested.push(Panel::Lyric),
                "library" => show_sidebar = true,
                other => {
                    return Err(format!(
                        "unknown panel \"{other}\" (try: player, playlist, lyric, library, all)"
                    ));
                }
            }
        }

        self.visible_panels = Panel::ALL
            .iter()
            .copied()
            .filter(|panel| requested.contains(panel))
            .collect();
        self.show_sidebar = show_sidebar;
        self.ensure_focus_visible();

        Ok(())
    }
}
