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
use crate::ui::tui_cmd::TuiCmd;

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
                    return;
                }

                if ["q", "kill", "exit"]
                    .iter()
                    .any(|cmd| trimmed.eq_ignore_ascii_case(cmd))
                {
                    // same behavior as the old direct `q` keybinding: still respects
                    // "confirm_quit" — quitting via `:` is deliberate enough already, no need
                    // for a second confirmation on top of that when the setting is off.
                    if self.config_tui.read().settings.behavior.confirm_quit {
                        self.mount_quit_popup();
                    } else {
                        self.quit = true;
                    }
                    return;
                }

                if let Some((cmd, arg)) = trimmed.split_once(' ')
                    && cmd.eq_ignore_ascii_case("presence")
                {
                    let arg = arg.trim();
                    if arg.eq_ignore_ascii_case("on") {
                        self.command(TuiCmd::SetDiscordPresence(true));
                    } else if arg.eq_ignore_ascii_case("off") {
                        self.command(TuiCmd::SetDiscordPresence(false));
                    } else {
                        trace!("ignoring malformed `:presence` command {trimmed:?}");
                    }
                    return;
                }

                // Any other non-empty command exits help mode, same as switching to any other mode.
                if !trimmed.is_empty() && self.app.mounted(&Id::HelpPopup) {
                    self.app.umount(&Id::HelpPopup).ok();
                    self.update_photo().ok();
                }

                self.apply_layout_command(&input);
            }
        }
    }

    /// Parse and apply a `:` layout command (e.g. `player`, `player+library`, `ishell`).
    ///
    /// Recognized tokens (combine with `+`): `player` (now-playing + visualizer + progress),
    /// `playlist`, `lyric`, `library` (the left sidebar), and `ishell` (reset to everything).
    /// Each command replaces the current set of visible regions entirely.
    ///
    /// An empty command is a no-op. An unrecognized token rejects the *whole* command silently
    /// (no popup, nothing changes) — this is a quiet, low-stakes input, not worth interrupting
    /// the user over.
    pub fn apply_layout_command(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }

        if input.eq_ignore_ascii_case("ishell") {
            self.visible_panels = Panel::ALL.to_vec();
            self.show_sidebar = true;
            self.show_visualizer = true;
            self.ensure_focus_visible();
            self.update_photo().ok();
            return;
        }

        let mut requested: Vec<Panel> = Vec::new();
        let mut show_sidebar = false;
        let mut show_visualizer = false;
        for token in input.split('+') {
            match token.trim().to_ascii_lowercase().as_str() {
                "player" => {
                    requested.extend([Panel::NowPlaying, Panel::Progress]);
                    show_visualizer = true;
                }
                "playlist" => requested.push(Panel::Playlist),
                "lyric" | "lyrics" => requested.push(Panel::Lyric),
                "library" => show_sidebar = true,
                other => {
                    trace!("ignoring unknown `:` command token {other:?} in {input:?}");
                    return;
                }
            }
        }

        // A bare "player" (no playlist/lyric to fill the middle) gets a spacer inserted between
        // NowPlaying and Progress, pushing Progress down and leaving a big empty gap up top for
        // the (centered, enlarged) cover art to occupy.
        if requested.contains(&Panel::NowPlaying)
            && requested.contains(&Panel::Progress)
            && !requested
                .iter()
                .any(|panel| matches!(panel, Panel::Playlist | Panel::Lyric))
        {
            requested.push(Panel::Spacer);
        }

        const ORDER: &[Panel] = &[
            Panel::NowPlaying,
            Panel::Spacer,
            Panel::Playlist,
            Panel::Progress,
            Panel::Lyric,
        ];
        self.visible_panels = ORDER
            .iter()
            .copied()
            .filter(|panel| requested.contains(panel))
            .collect();
        self.show_sidebar = show_sidebar;
        self.show_visualizer = show_visualizer;
        self.ensure_focus_visible();
        self.update_photo().ok();
    }
}
