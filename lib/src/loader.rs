use async_trait::async_trait;
use tokio::sync::mpsc;
#[allow(unused_imports)]
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use crate::error::*;
use crate::event::*;
use crate::redo::LogLookup;

#[derive(Debug, Clone)]
pub struct LoadData {
    pub(crate) lookup: LogLookup,
    pub header: EventHeaderRaw,
    pub data: EventWeakData,
}

#[async_trait]
pub trait Loader: Send + Sync {
    /// Function invoked when the start of the history is being loaded
    async fn start_of_history(&mut self, _size: usize) {}

    /// Human message sent from the server to this process
    fn human_message(&mut self, _message: String) {}

    /// Events are being processed
    fn feed_events(&mut self, _evts: &Vec<EventWeakData>) {}

    /// Load data is being processed
    async fn feed_load_data(&mut self, _data: LoadData) {}

    /// The last event is now received
    async fn end_of_history(&mut self) {}

    /// Callback when the load has failed
    async fn failed(&mut self, err: ChainCreationError) -> Option<ChainCreationError> {
        Some(err)
    }

    fn relevance_check(&mut self, _header: &EventWeakData) -> bool {
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct DummyLoader {}

impl Loader for DummyLoader {}

#[derive(Default)]
pub struct CompositionLoader {
    pub loaders: Vec<Box<dyn Loader>>,
}

#[async_trait]
impl Loader for CompositionLoader {
    async fn start_of_history(&mut self, size: usize) {
        for loader in self.loaders.iter_mut() {
            loader.start_of_history(size).await;
        }
    }

    fn human_message(&mut self, message: String) {
        for loader in self.loaders.iter_mut() {
            loader.human_message(message.clone());
        }
    }

    fn feed_events(&mut self, evts: &Vec<EventWeakData>) {
        for loader in self.loaders.iter_mut() {
            loader.feed_events(evts);
        }
    }

    async fn feed_load_data(&mut self, data: LoadData) {
        for loader in self.loaders.iter_mut() {
            loader.feed_load_data(data.clone()).await;
        }
    }

    async fn end_of_history(&mut self) {
        for loader in self.loaders.iter_mut() {
            loader.end_of_history().await;
        }
    }

    async fn failed(&mut self, mut err: ChainCreationError) -> Option<ChainCreationError> {
        let err_msg = err.to_string();
        for loader in self.loaders.iter_mut() {
            err = match loader.failed(err).await {
                Some(a) => a,
                None => ChainCreationErrorKind::InternalError(err_msg.clone()).into(),
            };
        }
        Some(err)
    }

    fn relevance_check(&mut self, header: &EventWeakData) -> bool {
        for loader in self.loaders.iter_mut() {
            if loader.relevance_check(header) {
                return true;
            }
        }
        false
    }
}

pub struct NotificationLoader {
    notify: mpsc::Sender<Result<(), ChainCreationError>>,
}

impl NotificationLoader {
    pub fn new(notify: mpsc::Sender<Result<(), ChainCreationError>>) -> NotificationLoader {
        NotificationLoader { notify }
    }
}

#[async_trait]
impl Loader for NotificationLoader {
    async fn start_of_history(&mut self, _size: usize) {
        trace!("sending notify");
        let _ = self.notify.send(Ok(())).await;
    }

    async fn end_of_history(&mut self) {
        trace!("sending notify");
        let _ = self.notify.send(Ok(())).await;
    }

    async fn failed(&mut self, err: ChainCreationError) -> Option<ChainCreationError> {
        let _ = self.notify.send(Err(err)).await;
        None
    }
}
