use tokio::sync::broadcast;
#[allow(unused_imports)]
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use async_trait::async_trait;
use fxhash::FxHashSet;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use super::workers::ChainWorkProcessor;
use crate::error::*;
use crate::header::PrimaryKey;
use crate::pipe::*;
use crate::transaction::*;

use super::workers::*;

pub(super) struct InboxPipe {
    pub(super) inbox: ChainWorkProcessor,
    pub(super) decache: broadcast::Sender<Vec<PrimaryKey>>,
    pub(super) locks: StdMutex<FxHashSet<PrimaryKey>>,
}

#[async_trait]
impl EventPipe for InboxPipe {
    async fn feed(&self, work: ChainWork) -> Result<(), CommitError> {
        // Prepare the work and submit it
        let decache = work
            .trans
            .events
            .iter()
            .filter_map(|a| a.meta.get_data_key())
            .collect::<Vec<_>>();

        // Submit the work
        let ret = self.inbox.process(work).await?;

        // Clear all the caches
        let _ = self.decache.send(decache);

        // Success
        Ok(ret)
    }

    #[allow(dead_code)]
    async fn try_lock(&self, key: PrimaryKey) -> Result<bool, CommitError> {
        let mut guard = self.locks.lock().unwrap();
        if guard.contains(&key) {
            return Ok(false);
        }
        guard.insert(key.clone());

        Ok(true)
    }

    #[allow(dead_code)]
    fn unlock_local(&self, key: PrimaryKey) -> Result<(), CommitError> {
        let mut guard = self.locks.lock().unwrap();
        guard.remove(&key);
        Ok(())
    }

    #[allow(dead_code)]
    async fn unlock(&self, key: PrimaryKey) -> Result<(), CommitError> {
        Ok(self.unlock_local(key)?)
    }

    fn set_next(&mut self, _next: Arc<Box<dyn EventPipe>>) {}

    async fn conversation(&self) -> Option<Arc<ConversationSession>> {
        None
    }
}
