use termusiclib::config::TuiOverlay;
use tuirealm::{
    component::{AppComponent, Component},
    event::Event,
};

use crate::ui::{components::LabelSpan, model::UserEvent, msg::Msg};

#[derive(Component)]
pub struct Footer {
    component: LabelSpan,
}

impl Footer {
    pub fn new(config: &TuiOverlay) -> Self {
        Self {
            component: LabelSpan::new(config, &[]),
        }
    }
}

impl AppComponent<Msg, UserEvent> for Footer {
    fn on(&mut self, _ev: &Event<UserEvent>) -> Option<Msg> {
        None
    }
}
