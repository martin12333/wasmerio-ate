#![allow(unused_imports)]
use crate::prelude::*;
use error_chain::bail;
use fxhash::FxHashMap;
use fxhash::FxHashSet;
use multimap::MultiMap;
use serde::Deserialize;
use serde::{de::DeserializeOwned, de::Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::Weak;
use std::time::Duration;
use std::{fmt::Debug, sync::Arc};
use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};
use tracing_futures::Instrument;

use super::dao::*;
use super::dao_mut::*;
use super::dio::*;
use super::row::*;
use crate::chain::ChainWork;
use crate::comms::*;
use crate::error::*;
use crate::event::*;
use crate::header::PrimaryKeyScope;
use crate::index::*;
use crate::lint::*;
use crate::meta::*;
use crate::spec::*;
use crate::time::*;
use crate::transaction::*;
use crate::tree::*;
use crate::trust::LoadResult;

use crate::crypto::{EncryptedPrivateKey, PrivateSignKey};
use crate::{
    crypto::EncryptKey,
    session::{AteSession, AteSessionProperty},
};

pub(crate) struct DioMutState {
    pub(super) store_ordered: Vec<RowHeader>,
    pub(super) store_secondary: MultiMap<MetaCollection, PrimaryKey>,
    pub(super) rows: FxHashMap<PrimaryKey, RowData>,
    pub(super) locked: FxHashSet<PrimaryKey>,
    pub(super) deleted: FxHashSet<PrimaryKey>,
    pub(super) pipe_unlock: FxHashSet<PrimaryKey>,
    pub(super) auto_cancel: bool,
}

impl DioMutState {
    /// Returns true if the row also needs to be updated
    pub(crate) fn dirty_header(&mut self, header: RowHeader) -> bool {
        if let Some(parent) = &header.parent {
            let exists = self
                .store_secondary
                .get_vec(&parent.vec)
                .iter()
                .map(|a| a.iter())
                .flatten()
                .any(|a| *a == header.key);
            if exists == false {
                self.store_secondary.insert(parent.vec.clone(), header.key);
            }
        }

        {
            // If the last row is a already there then we only need update it
            // and we don't need to do a complete data save
            if let Some(row) = self.store_ordered.iter_mut().rev().next() {
                if row.key == header.key {
                    *row = header;
                    return false;
                }
            }
        }

        self.store_ordered.push(header);
        return true;
    }

    pub(crate) fn dirty_row(&mut self, row: RowData) -> bool {
        let mut ret = true;
        if let Some(existing) = self.rows.get(&row.key) {
            if existing.data_hash == row.data_hash {
                //trace!("skipping row that has not changed [{}]", row.key);
                ret = false;
            }
        }

        let key = row.key.clone();
        let parent = row.parent.clone();

        self.rows.insert(key.clone(), row);
        if let Some(parent) = parent {
            let exists = self
                .store_secondary
                .get_vec(&parent.vec)
                .iter()
                .map(|a| a.iter())
                .flatten()
                .any(|a| *a == key);
            if exists == false {
                self.store_secondary.insert(parent.vec, key);
            }
        }

        ret
    }

    pub(super) fn lock(&mut self, key: &PrimaryKey) -> bool {
        self.locked.insert(key.clone())
    }

    pub(super) fn unlock(&mut self, key: &PrimaryKey) -> bool {
        self.locked.remove(key)
    }

    pub(super) fn is_locked(&self, key: &PrimaryKey) -> bool {
        self.locked.contains(key)
    }

    pub(super) fn add_deleted(&mut self, key: PrimaryKey, parent: Option<MetaParent>) {
        if self.lock(&key) == false {
            eprintln!("Detected concurrent write while deleting a data object ({:?}) - the delete operation will override everything else", key);
        }

        self.rows.remove(&key);
        if let Some(tree) = parent {
            if let Some(y) = self.store_secondary.get_vec_mut(&tree.vec) {
                y.retain(|x| *x == key);
            }
        }
        self.deleted.insert(key);
    }
}

impl DioMutState {
    fn new() -> DioMutState {
        DioMutState {
            store_ordered: Vec::new(),
            rows: FxHashMap::default(),
            store_secondary: MultiMap::new(),
            locked: FxHashSet::default(),
            deleted: FxHashSet::default(),
            pipe_unlock: FxHashSet::default(),
            auto_cancel: true,
        }
    }

    fn clear(&mut self) {
        self.store_ordered.clear();
        self.rows.clear();
        self.store_secondary.clear();
        self.locked.clear();
        self.deleted.clear();
        self.pipe_unlock.clear();
    }
}

/// Represents a series of mutations that the user is making on a particular chain-of-trust
/// with a specific set of facts attached to a session. All changes are stored in memory
/// until the commit function is invoked which will feed them into the chain.
///
/// If you decide to abort the transaction then call the `cancel` function before it goes
/// out of scope however if you mutate data and do not call `commit` then the data will be
/// lost (or an assert will be triggerd when in Debug mode).
///
/// These objects are multi-thread safe and allow for very high concurrency through async
/// operations.
///
/// When setting the scope for the DIO it will behave differently when the commit function
/// is invoked based on what scope you set for the transaction.
pub struct DioMut {
    pub dio: Arc<Dio>,
    pub scope: TransactionScope,
    pub(crate) state: Mutex<DioMutState>,
    pub(super) conversation: Option<Arc<ConversationSession>>,
    #[cfg(feature = "enable_dio_backtrace")]
    pub backtrace_new: backtrace::Backtrace,
}

pub(crate) struct DioMutScope {
    _inner: DioScope,
    pop: Option<Arc<DioMut>>,
    _negative: Rc<()>,
}

impl DioMutScope {
    pub fn new(trans: &Arc<DioMut>) -> Self {
        DioMutScope {
            _inner: DioScope::new(&trans.dio),
            pop: DioMut::current_set(Some(Arc::clone(trans))),
            _negative: Rc::new(()),
        }
    }
}

impl Drop for DioMutScope {
    fn drop(&mut self) {
        DioMut::current_set(self.pop.take());
    }
}

pub(crate) enum DioMutWeak {
    Uninitialized,
    Weak(Weak<DioMut>),
}

impl Default for DioMutWeak {
    fn default() -> Self {
        match DioMut::current_get() {
            Some(a) => DioMutWeak::Weak(Arc::downgrade(&a)),
            None => DioMutWeak::Uninitialized,
        }
    }
}

impl Clone for DioMutWeak {
    fn clone(&self) -> Self {
        match self {
            Self::Uninitialized => Self::default(),
            Self::Weak(a) => Self::Weak(Weak::clone(a)),
        }
    }
}

impl From<&Arc<DioMut>> for DioMutWeak {
    fn from(val: &Arc<DioMut>) -> Self {
        DioMutWeak::Weak(Arc::downgrade(val))
    }
}

impl DioMut {
    thread_local! {
        static CURRENT: RefCell<Option<Arc<DioMut>>> = RefCell::new(None)
    }

    pub(crate) fn current_get() -> Option<Arc<DioMut>> {
        DioMut::CURRENT.with(|trans| {
            let trans = trans.borrow();
            return trans.clone();
        })
    }

    fn current_set(val: Option<Arc<DioMut>>) -> Option<Arc<DioMut>> {
        DioMut::CURRENT.with(|trans| {
            let mut trans = trans.borrow_mut();
            match val {
                Some(a) => trans.replace(a),
                None => trans.take(),
            }
        })
    }

    pub async fn new(dio: &Arc<Dio>, scope: TransactionScope) -> Arc<DioMut> {
        TaskEngine::run_until(DioMut::__new(dio, scope)).await
    }

    pub(crate) async fn __new(dio: &Arc<Dio>, scope: TransactionScope) -> Arc<DioMut> {
        let ret = DioMut {
            dio: Arc::clone(dio),
            scope,
            state: Mutex::new(DioMutState::new()),
            conversation: dio.chain.pipe.conversation().await,
            #[cfg(feature = "enable_dio_backtrace")]
            backtrace_new: backtrace::Backtrace::new(),
        };
        Arc::new(ret)
    }

    pub fn store<D>(self: &Arc<Self>, data: D) -> Result<DaoMut<D>, SerializationError>
    where
        D: Clone + Serialize + DeserializeOwned,
    {
        self.store_with_format(data, None, self.dio.log_format)
    }

    pub fn store_with_key<D>(
        self: &Arc<Self>,
        data: D,
        key: PrimaryKey,
    ) -> Result<DaoMut<D>, SerializationError>
    where
        D: Clone + Serialize + DeserializeOwned,
    {
        self.store_with_format(data, Some(key.clone()), self.dio.log_format)
    }

    pub fn store_with_format<D>(
        self: &Arc<Self>,
        data: D,
        key: Option<PrimaryKey>,
        format: Option<MessageFormat>,
    ) -> Result<DaoMut<D>, SerializationError>
    where
        D: Clone + Serialize + DeserializeOwned,
    {
        let format = match format {
            Some(a) => a,
            None => self.default_format(),
        };

        let key = match key {
            Some(k) => k,
            None => PrimaryKey::generate(),
        };

        // We serialize then deserialize the object so that vectors and ref
        // objects get the proper references needed for the system to work
        let _pop1 = DioMutScope::new(self);
        let _pop2 = PrimaryKeyScope::new(key);
        let data = data.clone();

        let row_header = RowHeader {
            key: key.clone(),
            parent: None,
            auth: MetaAuthorization {
                read: ReadOption::Inherit,
                write: WriteOption::Inherit,
            },
        };
        let row = Row {
            key,
            type_name: std::any::type_name::<D>().to_string(),
            data,
            collections: FxHashSet::default(),
            format,
            created: 0,
            updated: 0,
            extra_meta: Vec::new(),
            is_new: true,
        };

        let mut ret: DaoMut<D> =
            DaoMut::new(Arc::clone(self), Dao::new(&self.dio, row_header, row));
        ret.commit(true, true)?;
        Ok(ret)
    }

    async fn run_async<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        let key_str = self.chain.key().to_string();
        TaskEngine::run_until(future.instrument(span!(Level::DEBUG, "dio", key = key_str.as_str())))
            .await
    }

    pub async fn delete(&self, key: &PrimaryKey) -> Result<(), SerializationError> {
        self.run_async(self.__delete(key)).await
    }

    async fn __delete(&self, key: &PrimaryKey) -> Result<(), SerializationError> {
        {
            let mut state = self.state.lock().unwrap();
            if state.is_locked(key) {
                bail!(SerializationErrorKind::ObjectStillLocked(key.clone()));
            }
            if state.deleted.contains(&key) {
                bail!(SerializationErrorKind::AlreadyDeleted(key.clone()));
            }
            state.store_ordered.retain(|a| a.key != *key);
        }

        let parent = self.multi.lookup_parent(key).await;
        self.state.lock().unwrap().add_deleted(key.clone(), parent);
        Ok(())
    }
}

impl Chain {
    /// Opens a data access layer that allows mutable changes to data.
    /// Transaction consistency on commit will be guarranted for local redo log files
    pub async fn dio_mut(self: &Arc<Chain>, session: &'_ dyn AteSession) -> Arc<DioMut> {
        TaskEngine::run_until(self.__dio_trans(session, TransactionScope::Local)).await
    }

    /// Opens a data access layer that allows mutable changes to data (in a fire-and-forget mode).
    /// No transaction consistency on commits will be enforced
    pub async fn dio_fire(self: &Arc<Chain>, session: &'_ dyn AteSession) -> Arc<DioMut> {
        TaskEngine::run_until(self.__dio_trans(session, TransactionScope::None)).await
    }

    /// Opens a data access layer that allows mutable changes to data.
    /// Transaction consistency on commit will be guarranted for all remote replicas
    pub async fn dio_full(self: &Arc<Chain>, session: &'_ dyn AteSession) -> Arc<DioMut> {
        TaskEngine::run_until(self.__dio_trans(session, TransactionScope::Full)).await
    }

    /// Opens a data access layer that allows mutable changes to data.
    /// Transaction consistency on commit must be specified
    pub async fn dio_trans(
        self: &Arc<Chain>,
        session: &'_ dyn AteSession,
        scope: TransactionScope,
    ) -> Arc<DioMut> {
        TaskEngine::run_until(self.__dio_trans(session, scope)).await
    }

    /// Opens a data access layer that allows mutable changes to data.
    /// Transaction consistency on commit must be specified
    pub(crate) async fn __dio_trans(
        self: &Arc<Chain>,
        session: &'_ dyn AteSession,
        scope: TransactionScope,
    ) -> Arc<DioMut> {
        let dio = self.__dio(session).await;
        dio.__trans(scope).await
    }
}

impl DioMut {
    pub fn has_uncommitted(&self) -> bool {
        let state = self.state.lock().unwrap();
        if state.store_ordered.is_empty() && state.deleted.is_empty() {
            return false;
        }
        return true;
    }

    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.clear();
    }

    pub fn auto_cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.auto_cancel = true;
    }

    pub fn auto_panic(&self) {
        let mut state = self.state.lock().unwrap();
        state.auto_cancel = false;
    }

    pub(crate) fn default_format(&self) -> MessageFormat {
        self.dio.multi.default_format.clone()
    }

    pub async fn commit(&self) -> Result<(), CommitError> {
        let timeout = Duration::from_secs(30);
        self.run_async(self.__commit(timeout)).await
    }

    pub async fn commit_ext(&self, timeout: Duration) -> Result<(), CommitError> {
        self.run_async(self.__commit(timeout)).await
    }

    async fn __commit(&self, timeout: Duration) -> Result<(), CommitError> {
        let (rows, deleted, unlocks) = {
            // If we have no dirty records
            let mut state = self.state.lock().unwrap();
            if state.store_ordered.is_empty() && state.deleted.is_empty() {
                return Ok(());
            }

            // Grab the rows from the state datachain
            let rows = state
                .store_ordered
                .iter()
                .filter(|a| state.deleted.contains(&a.key) == false)
                .filter_map(|a| match state.rows.get(&a.key) {
                    Some(b) => Some((a.clone(), b.clone())),
                    None => None,
                })
                .collect::<Vec<_>>();
            let deleted = state.deleted.iter().map(|a| a.clone()).collect::<Vec<_>>();
            let unlocks = state
                .pipe_unlock
                .iter()
                .map(|a| a.clone())
                .collect::<Vec<_>>();

            // Clear them all down as we have them now
            state.clear();

            // Now we process them
            trace!(
                "commit stored={} deleted={} unlocks={}",
                rows.len(),
                deleted.len(),
                unlocks.len()
            );
            (rows, deleted, unlocks)
        };

        // Declare variables
        let mut evts = Vec::new();
        let mut trans_meta = TransactionMetadata::default();

        {
            // Take all the locks we need to perform the commit actions
            let multi_lock = self.multi.lock().await;
            let session = self.session();

            // Determine the format of the message
            let format = match self.log_format {
                Some(a) => a,
                None => self.multi.default_format,
            };

            // Convert all the events that we are storing into serialize data
            for (row_header, row) in rows {
                // Debug output
                #[cfg(feature = "enable_verbose")]
                trace!("store: {}@{}", row.type_name, row.key.as_hex_string());

                // Build a new clean metadata header
                let mut meta = Metadata::for_data(row.key);
                meta.core
                    .push(CoreMetadata::Timestamp(self.time.current_timestamp()?));
                if row_header.auth.is_relevant() {
                    meta.core
                        .push(CoreMetadata::Authorization(row_header.auth.clone()));
                }
                if let Some(parent) = &row_header.parent {
                    meta.core.push(CoreMetadata::Parent(parent.clone()))
                } else {
                    if multi_lock.inside_async.disable_new_roots == true && row.is_new == true {
                        bail!(CommitErrorKind::NewRootsAreDisabled);
                    }
                }
                for extra in row.extra_meta.iter() {
                    meta.core.push(extra.clone());
                }
                if self.dio.chain.cfg_ate.record_type_name {
                    if meta.get_type_name().is_none() {
                        meta.core.push(CoreMetadata::Type(MetaType {
                            type_name: row.type_name.clone(),
                        }));
                    }
                }

                // Compute all the extra metadata for an event
                let extra_meta = multi_lock.metadata_lint_event(
                    &mut meta,
                    session.deref(),
                    &trans_meta,
                    &row.type_name,
                )?;
                meta.core.extend(extra_meta);

                // Add the data to the transaction metadata object
                if let Some(key) = meta.get_data_key() {
                    trans_meta.auth.insert(
                        key,
                        match meta.get_authorization() {
                            Some(a) => a.clone(),
                            None => MetaAuthorization {
                                read: ReadOption::Inherit,
                                write: WriteOption::Inherit,
                            },
                        },
                    );
                    if let Some(parent) = meta.get_parent() {
                        if parent.vec.parent_id != key {
                            trans_meta.parents.insert(key, parent.clone());
                        }
                    };
                }

                // Perform any transformation (e.g. data encryption and compression)
                let data = multi_lock.data_as_underlay(
                    &mut meta,
                    row.data.clone(),
                    session.deref(),
                    &trans_meta,
                )?;

                // Only once all the rows are processed will we ship it to the redo log
                let evt = EventData {
                    meta: meta,
                    data_bytes: Some(data),
                    format: row.format,
                };
                evts.push(evt);
            }

            // Build events that will represent tombstones on all these records (they will be sent after the writes)
            for key in deleted {
                let mut meta = Metadata::default();
                meta.core
                    .push(CoreMetadata::Timestamp(self.time.current_timestamp()?));
                meta.core
                    .push(CoreMetadata::Authorization(MetaAuthorization {
                        read: ReadOption::Everyone(None),
                        write: WriteOption::Nobody,
                    }));
                if let Some(parent) = multi_lock.inside_async.chain.lookup_parent(&key) {
                    meta.core.push(CoreMetadata::Parent(parent))
                }
                meta.add_tombstone(key);

                // Compute all the extra metadata for an event
                let extra_meta = multi_lock.metadata_lint_event(
                    &mut meta,
                    session.deref(),
                    &trans_meta,
                    "[unknown]",
                )?;
                meta.core.extend(extra_meta);

                let evt = EventData {
                    meta: meta,
                    data_bytes: None,
                    format,
                };
                evts.push(evt);
            }

            // Lint the data
            let mut lints = Vec::new();
            for evt in evts.iter() {
                lints.push(LintData {
                    data: evt,
                    header: evt.as_header()?,
                });
            }

            let meta = multi_lock.metadata_lint_many(
                &lints,
                session.deref(),
                self.conversation.as_ref(),
            )?;

            // If it has data then insert it at the front of these events
            if meta.len() > 0 {
                evts.insert(
                    0,
                    EventData {
                        meta: Metadata { core: meta },
                        data_bytes: None,
                        format,
                    },
                );
            }
        }

        #[cfg(feature = "enable_verbose")]
        {
            for evt in evts.iter() {
                trace!("event: {}", evt.meta);
            }
        }

        // Create the transaction
        let trans = Transaction {
            scope: self.scope.clone(),
            transmit: true,
            events: evts,
            timeout,
            conversation: match &self.conversation {
                Some(c) => Some(Arc::clone(c)),
                None => None,
            },
        };
        trace!("commit events={}", trans.events.len());

        // Process the transaction in the chain using its pipe
        self.multi.pipe.feed(ChainWork { trans: trans }).await?;

        // Last thing we do is kick off an unlock operation using fire and forget
        let unlock_multi = self.multi.clone();
        let unlock_me = unlocks.iter().map(|a| a.clone()).collect::<Vec<_>>();
        for key in unlock_me {
            let _ = unlock_multi.pipe.unlock(key).await;
        }

        // Success
        Ok(())
    }
}

impl std::ops::Deref for DioMut {
    type Target = Dio;

    fn deref(&self) -> &Self::Target {
        self.dio.deref()
    }
}

impl Drop for DioMut {
    fn drop(&mut self) {
        // Check if auto-cancel is enabled
        if self.has_uncommitted() & self.state.lock().unwrap().auto_cancel {
            debug!("Data objects have been discarded due to auto-cancel and uncommitted changes");
            #[cfg(feature = "enable_dio_backtrace")]
            debug!("{:?}", self.backtrace_new);
            self.cancel();
        }

        // If the DIO has uncommitted changes then warn the caller
        debug_assert!(self.has_uncommitted() == false, "dio-has-uncommitted - the DIO has uncommitted data in it - call the .commit() method before the DIO goes out of scope.");
    }
}

impl DioMut {
    pub async fn load<D>(self: &Arc<Self>, key: &PrimaryKey) -> Result<DaoMut<D>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let ret: DaoMut<D> = TaskEngine::run_until(self.__load(key)).await?;
        Ok(ret)
    }

    pub async fn try_load<D>(
        self: &Arc<Self>,
        key: &PrimaryKey,
    ) -> Result<Option<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        match self.load(key).await {
            Ok(a) => Ok(Some(a)),
            Err(LoadError(LoadErrorKind::NotFound(_), _)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn __load<D>(self: &Arc<Self>, key: &PrimaryKey) -> Result<DaoMut<D>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        {
            let state = self.state.lock().unwrap();
            let _pop1 = DioMutScope::new(self);

            if state.is_locked(key) {
                bail!(LoadErrorKind::ObjectStillLocked(key.clone()));
            }
            if state.deleted.contains(&key) {
                bail!(LoadErrorKind::AlreadyDeleted(key.clone()));
            }
            if let Some(dao) = state.rows.get(key) {
                let (row_header, row) = Row::from_row_data(&self.dio, dao.deref())?;
                return Ok(DaoMut::new(
                    Arc::clone(self),
                    Dao::<D>::new(&self.dio, row_header, row),
                ));
            }
        }

        {
            let state = self.dio.state.lock().unwrap();
            let _pop1 = DioMutScope::new(self);
            if let Some((dao, leaf)) = state.cache_load.get(key) {
                let (row_header, row) =
                    Row::from_event(&self.dio, dao.deref(), leaf.created, leaf.updated)?;
                return Ok(DaoMut::new(
                    Arc::clone(self),
                    Dao::new(&self.dio, row_header, row),
                ));
            }
        }

        let leaf = match self.multi.lookup_primary(key).await {
            Some(a) => a,
            None => bail!(LoadErrorKind::NotFound(key.clone())),
        };
        Ok(self.load_from_entry(leaf).await?)
    }

    pub(crate) async fn load_from_entry<D>(
        self: &Arc<Self>,
        leaf: EventLeaf,
    ) -> Result<DaoMut<D>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.run_async(self.__load_from_entry(leaf)).await
    }

    pub(super) async fn __load_from_entry<D>(
        self: &Arc<Self>,
        leaf: EventLeaf,
    ) -> Result<DaoMut<D>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let evt = self.multi.load(leaf).await?;
        let session = self.session();

        let _pop1 = DioMutScope::new(self);
        Ok(self.load_from_event(session.as_ref(), evt.data, evt.header.as_header()?, leaf)?)
    }

    pub(crate) fn load_from_event<D>(
        self: &Arc<Self>,
        session: &'_ dyn AteSession,
        mut data: EventData,
        header: EventHeader,
        leaf: EventLeaf,
    ) -> Result<DaoMut<D>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        data.data_bytes = match data.data_bytes {
            Some(data) => Some(self.multi.data_as_overlay(&header.meta, data, session)?),
            None => None,
        };

        let mut state = self.dio.state.lock().unwrap();
        let _pop1 = DioMutScope::new(self);

        match header.meta.get_data_key() {
            Some(key) => {
                let (row_header, row) =
                    Row::from_event(&self.dio, &data, leaf.created, leaf.updated)?;
                state.cache_load.insert(key.clone(), (Arc::new(data), leaf));
                Ok(DaoMut::new(
                    Arc::clone(self),
                    Dao::new(&self.dio, row_header, row),
                ))
            }
            None => Err(LoadErrorKind::NoPrimaryKey.into()),
        }
    }

    pub async fn load_and_take<D>(self: &Arc<Self>, key: &PrimaryKey) -> Result<D, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.run_async(self.__load_and_take(key)).await
    }

    async fn __load_and_take<D>(self: &Arc<Self>, key: &PrimaryKey) -> Result<D, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let ret: DaoMut<D> = self.__load(key).await?;
        Ok(ret.take())
    }

    pub async fn load_raw(self: &Arc<Self>, key: &PrimaryKey) -> Result<EventData, LoadError> {
        self.run_async(self.dio.__load_raw(key)).await
    }

    pub async fn exists(&self, key: &PrimaryKey) -> bool {
        self.run_async(self.__exists(key)).await
    }

    async fn __exists(&self, key: &PrimaryKey) -> bool {
        {
            let state = self.state.lock().unwrap();
            if state.deleted.contains(&key) {
                return false;
            }
            if let Some(_) = state.rows.get(key) {
                return true;
            }
        }
        self.dio.__exists(key).await
    }

    pub async fn try_lock(self: &Arc<Self>, key: PrimaryKey) -> Result<bool, CommitError> {
        self.multi.pipe.try_lock(key).await
    }

    pub async fn unlock(self: &Arc<Self>, key: PrimaryKey) -> Result<(), CommitError> {
        self.multi.pipe.unlock(key).await
    }

    pub async fn children<D>(
        self: &Arc<Self>,
        parent_id: PrimaryKey,
        collection_id: u64,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.children_ext(parent_id, collection_id, false, false)
            .await
    }

    pub async fn children_ext<D>(
        self: &Arc<Self>,
        parent_id: PrimaryKey,
        collection_id: u64,
        allow_missing_keys: bool,
        allow_serialization_error: bool,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.run_async(self.__children_ext(
            parent_id,
            collection_id,
            allow_missing_keys,
            allow_serialization_error,
        ))
        .await
    }

    pub async fn __children_ext<D>(
        self: &Arc<Self>,
        parent_id: PrimaryKey,
        collection_id: u64,
        allow_missing_keys: bool,
        allow_serialization_error: bool,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        // Build the secondary index key
        let collection_key = MetaCollection {
            parent_id,
            collection_id,
        };

        // Build a list of keys
        let keys = match self.multi.lookup_secondary_raw(&collection_key).await {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };

        // Perform the lower level calls
        let mut ret: Vec<DaoMut<D>> = self
            .__load_many_ext(
                keys.into_iter(),
                allow_missing_keys,
                allow_serialization_error,
            )
            .await?;

        // Build an already loaded list
        let mut already = FxHashSet::default();
        for a in ret.iter() {
            already.insert(a.key().clone());
        }

        // Now we search the secondary local index so any objects we have
        // added in this transaction scope are returned
        let state = self.state.lock().unwrap();
        let _pop1 = DioMutScope::new(self);
        if let Some(vec) = state.store_secondary.get_vec(&collection_key) {
            for a in vec {
                // This is an OR of two lists so its likely that the object
                // may already be in the return list
                if already.contains(a) {
                    continue;
                }
                if state.deleted.contains(a) {
                    continue;
                }

                // If its still locked then that is a problem
                if state.is_locked(a) {
                    bail!(LoadErrorKind::ObjectStillLocked(a.clone()));
                }

                if let Some(dao) = state.rows.get(a) {
                    let (row_header, row) = Row::from_row_data(&self.dio, dao.deref())?;

                    already.insert(row.key.clone());
                    let dao: Dao<D> = Dao::new(&self.dio, row_header, row);
                    ret.push(DaoMut::new(Arc::clone(self), dao));
                }
            }
        }

        Ok(ret)
    }

    pub async fn load_many<D>(
        self: &Arc<Self>,
        keys: impl Iterator<Item = PrimaryKey>,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.load_many_ext(keys, false, false).await
    }

    pub async fn load_many_ext<D>(
        self: &Arc<Self>,
        keys: impl Iterator<Item = PrimaryKey>,
        allow_missing_keys: bool,
        allow_serialization_error: bool,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        self.run_async(self.__load_many_ext(keys, allow_missing_keys, allow_serialization_error))
            .await
    }

    async fn __load_many_ext<D>(
        self: &Arc<Self>,
        keys: impl Iterator<Item = PrimaryKey>,
        allow_missing_keys: bool,
        allow_serialization_error: bool,
    ) -> Result<Vec<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        // This is the main return list
        let mut already = FxHashSet::default();
        let mut ret = Vec::new();

        let inside_async = self.multi.inside_async.read().await;

        // We either find existing objects in the cache or build a list of objects to load
        let to_load = {
            let mut to_load = Vec::new();

            let state = self.state.lock().unwrap();
            let inner_state = self.dio.state.lock().unwrap();
            let _pop1 = DioMutScope::new(self);

            for key in keys {
                if state.is_locked(&key) {
                    bail!(LoadErrorKind::ObjectStillLocked(key));
                }
                if state.deleted.contains(&key) {
                    continue;
                }
                if let Some(dao) = state.rows.get(&key) {
                    let (row_header, row) = Row::from_row_data(&self.dio, dao.deref())?;
                    already.insert(row.key.clone());
                    ret.push(Dao::new(&self.dio, row_header, row));
                    continue;
                }
                if let Some((dao, leaf)) = inner_state.cache_load.get(&key) {
                    let (row_header, row) =
                        Row::from_event(&self.dio, dao.deref(), leaf.created, leaf.updated)?;
                    already.insert(row.key.clone());
                    ret.push(Dao::new(&self.dio, row_header, row));
                    continue;
                }

                to_load.push(match inside_async.chain.lookup_primary(&key) {
                    Some(a) => a,
                    None => continue,
                });
            }

            to_load
        };

        // Load all the objects that have not yet been loaded
        let to_load = inside_async.chain.load_many(to_load).await?;

        // Now process all the objects
        let ret = {
            let state = self.state.lock().unwrap();
            let mut inner_state = self.dio.state.lock().unwrap();
            let _pop1 = DioMutScope::new(self);

            let session = self.session();
            for mut evt in to_load {
                let mut header = evt.header.as_header()?;

                let key = match header.meta.get_data_key() {
                    Some(k) => k,
                    None => {
                        continue;
                    }
                };

                if state.is_locked(&key) {
                    bail!(LoadErrorKind::ObjectStillLocked(key.clone()));
                }
                if state.deleted.contains(&key) {
                    continue;
                }

                if let Some(dao) = state.rows.get(&key) {
                    let (row_header, row) = Row::from_row_data(&self.dio, dao.deref())?;

                    already.insert(row.key.clone());
                    ret.push(Dao::new(&self.dio, row_header, row));
                    continue;
                }
                if let Some((dao, leaf)) = inner_state.cache_load.get(&key) {
                    let (row_header, row) =
                        Row::from_event(&self.dio, dao.deref(), leaf.created, leaf.updated)?;

                    already.insert(row.key.clone());
                    ret.push(Dao::new(&self.dio, row_header, row));
                }

                let (row_header, row) = match self.dio.__process_load_row(
                    session.as_ref(),
                    &mut evt,
                    &mut header.meta,
                    allow_missing_keys,
                    allow_serialization_error,
                )? {
                    Some(a) => a,
                    None => {
                        continue;
                    }
                };

                inner_state
                    .cache_load
                    .insert(row.key.clone(), (Arc::new(evt.data), evt.leaf));

                already.insert(row.key.clone());
                ret.push(Dao::new(&self.dio, row_header, row));
            }
            ret
        };

        Ok(ret
            .into_iter()
            .map(|a: Dao<D>| DaoMut::new(Arc::clone(self), a))
            .collect::<Vec<_>>())
    }

    pub(crate) fn data_as_overlay(
        self: &Arc<Self>,
        session: &'_ dyn AteSession,
        data: &mut EventData,
    ) -> Result<(), TransformError> {
        self.dio.data_as_overlay(session, data)?;
        Ok(())
    }

    pub fn session<'a>(&'a self) -> DioSessionGuard<'a> {
        self.dio.session()
    }

    pub fn session_mut<'a>(&'a self) -> DioSessionGuardMut<'a> {
        self.dio.session_mut()
    }
}
