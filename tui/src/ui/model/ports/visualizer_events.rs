use std::{fmt::Debug, pin::Pin};

use anyhow::Result;
use futures_util::Stream;
use termusiclib::player::VisualizerFrame;
use tokio_stream::StreamExt;
use tuirealm::{
    event::Event,
    listener::{PollAsync, PortResult},
};

use crate::ui::{model::UserEvent, msg::Msg};

pub type WrappedVisualizerEvents = Pin<Box<dyn Stream<Item = Result<VisualizerFrame>> + Send>>;

/// tuirealm async port for the live audio-visualizer stream.
///
/// Kept as its own port (not folded into [`PortStreamEvents`](super::stream_events::PortStreamEvents))
/// since it is a much higher-frequency, disposable stream: an error or gap here should never be
/// treated with the same severity as a missed "common" event.
pub struct PortVisualizerEvents(WrappedVisualizerEvents);

impl PortVisualizerEvents {
    pub fn new(events: WrappedVisualizerEvents) -> Self {
        Self(events)
    }
}

impl Debug for PortVisualizerEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PortVisualizerEvents")
            .field(&"<stream>")
            .finish()
    }
}

#[tuirealm::async_trait]
impl PollAsync<UserEvent> for PortVisualizerEvents {
    async fn poll(&mut self) -> PortResult<Option<Event<UserEvent>>> {
        match self.0.next().await {
            Some(Ok(frame)) => Ok(Some(Event::User(UserEvent::Forward(
                Msg::VisualizerFrame(frame.into()),
            )))),
            Some(Err(err)) => {
                trace!("Visualizer stream error (ignored): {err:#?}");
                Ok(None)
            }
            None => Ok(None),
        }
    }
}
