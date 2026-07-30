use tui_realm_stdlib::components::Table;
use tui_realm_stdlib::prop_ext::CommonHighlight;
use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers};
use tuirealm::props::{BorderType, Borders, HorizontalAlignment, LineStatic, TableBuilder, Title};

use termusiclib::config::SharedTuiSettings;

use crate::ui::model::UserEvent;
use crate::ui::msg::{Msg, TEMsg, TFMsg};
use crate::ui::utils::STYLE_REMOVE_REVERSE;

#[derive(Component)]
pub struct TETableLyricOptions {
    component: Table,
    config: SharedTuiSettings,
}

impl TETableLyricOptions {
    pub fn new(config: SharedTuiSettings) -> Self {
        let component = {
            let config = config.read();
            Table::default()
                .borders(
                    Borders::default()
                        .modifiers(BorderType::Rounded)
                        .color(config.settings.theme.library_border()),
                )
                .foreground(config.settings.theme.library_foreground())
                .background(config.settings.theme.library_background())
                .title(Title::from(" Search Results ").alignment(HorizontalAlignment::Left))
                .scroll(true)
                .highlight_style(
                    CommonHighlight::default()
                        .style
                        .fg(config.settings.theme.library_highlight()),
                )
                .highlight_style_inactive(STYLE_REMOVE_REVERSE)
                .highlight_str("\u{1f680}")
                .rewind(false)
                .step(4)
                .row_height(1)
                .headers(["Artist", "Title", "Album", "api", "Copyright Info"])
                .column_spacing(1)
                .widths(&[20, 20, 20, 10, 30])
                .table(
                    TableBuilder::default()
                        .add_col(LineStatic::from("0"))
                        .add_col(LineStatic::from(" "))
                        .add_col(LineStatic::from("No Results."))
                        .build(),
                )
        };

        Self { component, config }
    }
}

impl AppComponent<Msg, UserEvent> for TETableLyricOptions {
    fn on(&mut self, ev: &Event<UserEvent>) -> Option<Msg> {
        let config = self.config.clone();
        let keys = &config.read().settings.keys;
        let cmd_result = match ev {
            Event::Keyboard(KeyEvent { code: Key::Tab, .. }) => {
                return Some(Msg::TagEditor(TEMsg::Focus(
                    TFMsg::TableLyricOptionsBlurDown,
                )));
            }
            Event::Keyboard(KeyEvent {
                code: Key::BackTab,
                modifiers: KeyModifiers::SHIFT,
            }) => {
                return Some(Msg::TagEditor(TEMsg::Focus(TFMsg::TableLyricOptionsBlurUp)));
            }

            Event::Keyboard(keyevent) if keyevent == keys.config_keys.save.get() => {
                return Some(Msg::TagEditor(TEMsg::Save));
            }

            Event::Keyboard(k) if k == keys.quit.get() => {
                return Some(Msg::TagEditor(TEMsg::Close));
            }
            Event::Keyboard(k) if k == keys.escape.get() => {
                return Some(Msg::TagEditor(TEMsg::Close));
            }
            Event::Keyboard(KeyEvent {
                code: Key::Down, ..
            }) => self.perform(Cmd::Move(Direction::Down)),
            Event::Keyboard(KeyEvent { code: Key::Up, .. }) => {
                self.perform(Cmd::Move(Direction::Up))
            }
            Event::Keyboard(k) if k == keys.navigation_keys.down.get() => {
                self.perform(Cmd::Move(Direction::Down))
            }
            Event::Keyboard(k) if k == keys.navigation_keys.up.get() => {
                self.perform(Cmd::Move(Direction::Up))
            }
            Event::Keyboard(KeyEvent {
                code: Key::PageDown,
                ..
            }) => self.perform(Cmd::Scroll(Direction::Down)),
            Event::Keyboard(KeyEvent {
                code: Key::PageUp, ..
            }) => self.perform(Cmd::Scroll(Direction::Up)),
            Event::Keyboard(KeyEvent {
                code: Key::Home, ..
            }) => self.perform(Cmd::GoTo(Position::Begin)),
            Event::Keyboard(KeyEvent { code: Key::End, .. }) => {
                self.perform(Cmd::GoTo(Position::End))
            }

            Event::Keyboard(k) if k == keys.navigation_keys.goto_top.get() => {
                self.perform(Cmd::GoTo(Position::Begin))
            }

            Event::Keyboard(k) if k == keys.navigation_keys.goto_bottom.get() => {
                self.perform(Cmd::GoTo(Position::End))
            }

            _ => CmdResult::NoChange,
        };
        match cmd_result {
            CmdResult::NoChange => None,
            _ => Some(Msg::ForceRedraw),
        }
    }
}
