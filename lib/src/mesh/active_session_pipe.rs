use crate::engine::timeout;
use async_trait::async_trait;
use error_chain::bail;
use fxhash::FxHashMap;
use std::ops::Rem;
use std::sync::Mutex as StdMutex;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;
use std::time::Instant;
use std::{sync::Arc, sync::Weak};
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use super::core::*;
use super::lock_request::*;
use super::msg::*;
use super::recoverable_session_pipe::*;
use super::*;
use super::session::LoadRequest;
use crate::chain::*;
use crate::conf::*;
use crate::crypto::*;
use crate::error::*;
use crate::header::*;
use crate::loader::*;
use crate::meta::*;
use crate::pipe::*;
use crate::session::*;
use crate::spec::*;
use crate::time::*;
use crate::transaction::*;
use crate::trust::*;
use crate::{anti_replay::AntiReplayPlugin, comms::*};

pub(super) struct ActiveSessionPipe {
    pub(super) key: ChainKey,
    pub(super) tx: Tx,
    pub(super) mode: RecoveryMode,
    pub(super) session: Arc<MeshSession>,
    pub(super) connected: bool,
    pub(super) likely_read_only: bool,
    pub(super) commit: Arc<StdMutex<FxHashMap<u64, mpsc::Sender<Result<u64, CommitError>>>>>,
    pub(super) lock_attempt_timeout: Duration,
    pub(super) lock_requests: Arc<StdMutex<FxHashMap<PrimaryKey, LockRequest>>>,
    pub(super) load_timeout: Duration,
    pub(super) load_requests: Arc<StdMutex<FxHashMap<u64, LoadRequest>>>,
    pub(super) outbound_conversation: Arc<ConversationSession>,
}

impl ActiveSessionPipe {
    pub(super) fn mark_connected(&mut self) {
        self.connected = true;
    }

    pub(super) fn is_connected(&self) -> bool {
        if self.connected == false {
            return false;
        }
        true
    }

    pub(super) fn on_read_only(&mut self) {
        self.likely_read_only = true;
    }

    pub(super) async fn on_disconnect(&self) -> Result<(), CommsError> {
        // Switch over to a distributed integrity mode as while we are in an offline
        // state we need to make sure we sign all the records. Its only the server
        // and the fact we trust it that we can omit signatures
        if let Some(chain) = self.session.chain.upgrade() {
            chain.single().await.set_integrity(TrustMode::Distributed);
        }

        Ok(())
    }

    pub(super) async fn feed_internal(
        &mut self,
        trans: &mut Transaction,
    ) -> Result<Option<mpsc::Receiver<Result<u64, CommitError>>>, CommitError> {
        // Convert the event data into message events
        let evts = MessageEvent::convert_to(&trans.events);

        // If the scope requires synchronization with the remote server then allocate a commit ID
        let (commit, receiver) = match &trans.scope {
            TransactionScope::Full => {
                // Generate a sender/receiver pair
                let (sender, receiver) = mpsc::channel(1);

                // Register a commit ID that will receive the response
                let id = fastrand::u64(..);
                self.commit.lock().unwrap().insert(id, sender);
                (Some(id), Some(receiver))
            }
            _ => (None, None),
        };

        // Send the same packet to all the transmit nodes (if there is only one then don't clone)
        trace!("tx wire_format={}", self.tx.wire_format);
        self.tx
            .send_all_msg(Message::Events { commit, evts })
            .await?;

        Ok(receiver)
    }
}

impl ActiveSessionPipe {
    pub(super) async fn feed(&mut self, trans: &mut Transaction) -> Result<Option<mpsc::Receiver<Result<u64, CommitError>>>, CommitError> {
        // Only transmit the packet if we are meant to
        let ret = if trans.transmit == true {
            // If we are likely in a read only situation then all transactions
            // should go to the server in synchronous mode until we can confirm
            // normal writability is restored
            if self.likely_read_only && self.mode.should_go_readonly() {
                trans.scope = TransactionScope::Full;
            }

            // If we are still connecting then don't do it
            if self.connected == false {
                if self.mode.should_error_out() {
                    return Err(CommitErrorKind::CommsError(CommsErrorKind::Disconnected).into());
                } else if self.mode.should_go_readonly() {
                    return Err(CommitErrorKind::CommsError(CommsErrorKind::ReadOnly).into());
                } else {
                    return Ok(None);
                }
            }

            // Feed the transaction into the pipe
            self.feed_internal(trans).await?
        } else {
            None
        };

        Ok(ret)
    }

    pub(super) async fn load_many(&mut self, leafs: Vec<AteHash>) -> Result<Vec<Option<Bytes>>, LoadError> {
        // Register a load ID that will receive the response
        let (tx, mut rx) = mpsc::channel(1);
        let id = fastrand::u64(..);
        self.load_requests.lock().unwrap().insert(id, LoadRequest {
            records: leafs.clone(),
            tx
        });

        // Inform the server that we want these records
        self.tx
            .send_all_msg(Message::LoadMany { id, leafs: leafs })
            .await
            .map_err(|err| {
                trace!("load failed: {}", err);
                LoadErrorKind::Disconnected
            })?;

        // Wait for the response from the server (or a timeout)
        match crate::engine::timeout(self.load_timeout, rx.recv()).await {
            Ok(Some(a)) => {
                self.likely_read_only = false;
                return a;
            }
            Ok(None) => {
                self.load_requests.lock().unwrap().remove(&id);
                bail!(LoadErrorKind::Disconnected);
            }
            Err(_) => {
                self.load_requests.lock().unwrap().remove(&id);
                bail!(LoadErrorKind::Timeout)
            },
        };
    }

    pub(super) async fn try_lock(&mut self, key: PrimaryKey) -> Result<bool, CommitError> {
        // If we are still connecting then don't do it
        if self.connected == false {
            bail!(CommitErrorKind::LockError(CommsErrorKind::Disconnected));
        }

        // Write an entry into the lookup table
        let (tx, mut rx) = watch::channel(false);
        let my_lock = LockRequest {
            needed: 1,
            positive: 0,
            negative: 0,
            tx,
        };
        self.lock_requests
            .lock()
            .unwrap()
            .insert(key.clone(), my_lock);

        // Send a message up to the main server asking for a lock on the data object
        trace!("tx lock key={}", key);
        self.tx
            .send_all_msg(Message::Lock { key: key.clone() })
            .await?;

        // Wait for the response from the server
        let ret = match crate::engine::timeout(self.lock_attempt_timeout, rx.changed()).await {
            Ok(a) => {
                self.likely_read_only = false;
                if let Err(_) = a {
                    bail!(CommitErrorKind::LockError(
                        CommsErrorKind::Disconnected.into()
                    ));
                }
                *rx.borrow()
            }
            Err(_) => {
                self.lock_requests.lock().unwrap().remove(&key);
                bail!(CommitErrorKind::LockError(CommsErrorKind::Timeout.into()))
            },
        };
        Ok(ret)
    }

    pub(super) async fn unlock(&mut self, key: PrimaryKey) -> Result<(), CommitError> {
        // If we are still connecting then don't do it
        if self.connected == false {
            bail!(CommitErrorKind::CommsError(CommsErrorKind::Disconnected));
        }

        // Send a message up to the main server asking for an unlock on the data object
        trace!("tx unlock key={}", key);
        self.tx
            .send_all_msg(Message::Unlock { key: key.clone() })
            .await?;

        // Success
        Ok(())
    }

    pub(super) fn conversation(&self) -> Option<Arc<ConversationSession>> {
        Some(Arc::clone(&self.outbound_conversation))
    }
}

impl Drop for ActiveSessionPipe {
    fn drop(&mut self) {
        #[cfg(feature = "enable_verbose")]
        debug!("drop {}", self.key.to_string());
    }
}
