#![allow(unused_imports)]
use tracing::{warn, debug};
use fxhash::FxHashSet;
use async_trait::async_trait;

use serde::{Serialize, de::DeserializeOwned};
use bytes::Bytes;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Weak};
use parking_lot::{Mutex, MutexGuard};

use crate::crypto::{EncryptedPrivateKey, PrivateSignKey};
use crate::{crypto::EncryptKey, session::{AteSession, AteSessionProperty}};

use super::dio_mut::*;
use super::dio::*;
use super::dao::*;
use crate::header::*;
use crate::event::*;
use crate::meta::*;
use crate::error::*;
use crate::crypto::AteHash;
use crate::spec::*;
use crate::index::*;

pub use super::vec::DaoVec;
use super::row::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DaoMutLock
{
    /// The DAO has no lock on it
    Unlocked,
    /// The DAO has been manually locked forcing serial access
    Locked,
    /// The dao is being processed thus holds a lock and should be deleted
    /// when it goes out of scope
    LockedThenDelete,
}

#[derive(Debug, Clone)]
pub struct DaoMutState
{
    pub(super) lock: DaoMutLock,
}

pub(crate) trait DaoObjCommit: DaoObj
{
    fn commit(&mut self, data_changed: bool) -> std::result::Result<(), SerializationError>;
    
    fn auth_set(&mut self, auth: MetaAuthorization) -> std::result::Result<(), SerializationError>;
}

/// Represents a data object that will be represented as one or
/// more events on the redo-log and validated in the chain-of-trust.
/// 
/// Reading this object using none-mutable behavior will incur no IO
/// on the redo-log however if you edit the object you must commit it
/// to the `Dio` before it goes out of scope or the data will be lost
/// (in Debug mode this will even trigger an assert).
///
/// Metadata about the data object can also be accessed via this object
/// which allows you to change the read/write access rights, etc.
///
/// If you change your mind on commiting the data to the redo-log then
/// you can call the `cancel` function instead.
///
/// The real version represents all operations that can be performed
/// before the obejct is actually saved and all those after
pub struct DaoMut<D>
where D: Serialize
{
    inner: Dao<D>,
    trans: Arc<DioMut>,
    state: DaoMutState,
}

impl<D> Clone
for DaoMut<D>
where D: Serialize + Clone,
{
    fn clone(&self) -> Self
    {
        DaoMut {
            inner: self.inner.clone(),
            trans: Arc::clone(&self.trans),
            state: self.state.clone(),
        }
    }
}

impl<D> std::fmt::Debug
for DaoMut<D>
where D: Serialize + std::fmt::Debug
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dao-mut(")?;
        self.inner.fmt(f)?;
        write!(f, ")")
    }
}

impl<D> DaoMut<D>
where D: Serialize
{
    pub(super) fn new(trans: Arc<DioMut>, inner: Dao<D>) -> DaoMut<D> {
        DaoMut {
            trans,
            inner,
            state: DaoMutState {
                lock: DaoMutLock::Unlocked,
            },
        }
    }

    pub fn trans(&self) -> Arc<DioMut> {
        Arc::clone(&self.trans)
    }

    pub fn set_trans(&mut self, dio: &Arc<DioMut>) {
        self.trans = Arc::clone(dio);
    }

    pub fn delete(self) -> std::result::Result<(), SerializationError> {
        let key = self.key().clone();
        let mut state = self.trans.state.lock();
        state.add_deleted(key, self.inner.row_header.parent.clone());
        Ok(())
    }

    pub fn detach(&mut self) -> std::result::Result<(), SerializationError>
    {
        self.inner.row_header.parent = None;
        self.commit(false)
    }

    pub fn attach_ext(&mut self, parent: PrimaryKey, collection_id: u64) -> std::result::Result<(), SerializationError>
    {
        self.inner.row_header.parent = Some(
            MetaParent {
                vec: MetaCollection {
                    parent_id: parent,
                    collection_id,
                },
            }
        );
        self.commit(false)
    }

    pub fn attach_orphaned(&mut self, parent: &PrimaryKey) -> std::result::Result<(), SerializationError> {
        self.attach_ext(parent.clone(), 0u64)
    }

    pub fn attach_orphaned_ext(&mut self, parent: &PrimaryKey, collection_id: u64) -> std::result::Result<(), SerializationError> {
        self.attach_ext(parent.clone(),collection_id)
    }

    pub fn add_extra_metadata(&mut self, meta: CoreMetadata) -> std::result::Result<(), SerializationError>
    {
        self.inner.row.extra_meta.push(meta);
        self.commit(true)
    }

    pub fn is_locked(&self) -> bool {
        match self.state.lock {
            DaoMutLock::Locked | DaoMutLock::LockedThenDelete => true,
            DaoMutLock::Unlocked => false
        }
    }

    pub fn attach(&mut self, parent: &dyn DaoObj, vec: &DaoVec<D>) -> std::result::Result<(), SerializationError>
    where D: Serialize
    {
        self.inner.row_header.parent = Some(
            MetaParent {
                vec: MetaCollection {
                    parent_id: parent.key().clone(),
                    collection_id: vec.vec_id,
                },
            }
        );
        self.commit(false)
    }

    async fn try_lock_ext(&mut self, new_state: DaoMutLock) -> Result<bool, LockError> {
        match self.state.lock {
            DaoMutLock::Locked | DaoMutLock::LockedThenDelete => {},
            DaoMutLock::Unlocked =>
            {
                // Attempt the lock
                let dio = self.dio();
                if dio.multi.pipe.try_lock(self.inner.row.key.clone()).await? == false {
                    return Ok(false)
                }

                // The object is now locked
                self.state.lock = new_state;
            }
        };
        Ok(true)
    }

    pub async fn try_lock(&mut self) -> Result<bool, LockError> {
        self.try_lock_ext(DaoMutLock::Locked).await
    }

    pub async fn unlock(&mut self) -> Result<bool, LockError> {
        match self.state.lock {
            DaoMutLock::Unlocked | DaoMutLock::LockedThenDelete => {
                return Ok(false);
            },
            DaoMutLock::Locked => {
                let dio = self.inner.dio();
                dio.multi.pipe.unlock(self.inner.row.key.clone()).await?;
                self.state.lock = DaoMutLock::Unlocked;
            }
        };

        Ok(true)
    }

    pub async fn try_lock_then_delete(&mut self) -> Result<bool, LockError> {
        self.try_lock_ext(DaoMutLock::LockedThenDelete).await
    }

    pub fn auth_mut<'a>(&'a mut self) -> DaoAuthGuard<'a> {
        DaoAuthGuard {
            auth: self.inner.row_header.auth.clone(),
            dao: self,
            dirty: false
        }
    }

    pub fn take(self) -> D
    {
        self.inner.row.data
    }

    pub fn parent(&self) -> Option<MetaCollection>
    {
        self.inner.parent()
    }

    pub fn parent_id(&self) -> Option<PrimaryKey>
    {
        self.inner.parent_id()
    }

    pub fn as_mut<'a>(&'a mut self) -> DaoMutGuard<'a, D>
    {
        DaoMutGuard {
            dao: self,
            dirty: false
        }
    }

    pub fn as_ref<'a>(&'a self) -> &'a D
    {
        &self.inner.row.data
    }

    pub fn as_immutable(&self) -> &Dao<D>
    {
        &self.inner
    }

    pub fn as_mut_owned(self) -> DaoMutGuardOwned<D>
    {
        DaoMutGuardOwned {
            dao: self,
            dirty: false
        }
    }
}

impl<'a, D> DaoObjCommit
for DaoMut<D>
where D: Serialize
{
    fn auth_set(&mut self, auth: MetaAuthorization) -> std::result::Result<(), SerializationError>
    {
        self.inner.row_header.auth = auth;
        self.commit(false)
    }

    fn commit(&mut self, data_changed: bool) -> std::result::Result<(), SerializationError>
    where D: Serialize
    {
        let mut state = self.trans.state.lock();

        // The local DIO lock gets released first
        state.unlock(&self.inner.row.key);

        // Next any pessimistic locks on the local chain
        match self.state.lock {
            DaoMutLock::Locked => {
                state.pipe_unlock.insert(self.inner.row.key.clone());
            },
            DaoMutLock::LockedThenDelete => {
                state.pipe_unlock.insert(self.inner.row.key.clone());
                let key = self.key().clone();
                state.add_deleted(key, self.inner.row_header.parent.clone());
                return Ok(())
            },
            _ => {}
        }

        let mut commit_data = data_changed;
        if state.dirty_header(self.inner.row_header.clone()) {
            commit_data = true;
        }
        if commit_data {
            let row_data = {
                self.inner.row.as_row_data(&self.inner.row_header)?
            };
            state.dirty_row(row_data);
        }
        Ok(())
    }
}

impl<D> std::ops::Deref
for DaoMut<D>
where D: Serialize
{
    type Target = D;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<D> DaoObj
for DaoMut<D>
where D: Serialize
{
    fn key(&self) -> &PrimaryKey {
        self.inner.key()
    }

    fn auth(&self) -> &MetaAuthorization {
        self.inner.auth()
    }

    fn dio(&self) -> &Arc<Dio> {
        self.inner.dio()
    }

    fn when_created(&self) -> u64 {
        self.inner.when_created()
    }

    fn when_updated(&self) -> u64 {
        self.inner.when_updated()
    }
}

pub struct DaoAuthGuard<'a>
{
    dao: &'a mut dyn DaoObjCommit,
    auth: MetaAuthorization,
    dirty: bool
}

impl<'a> DaoAuthGuard<'a>
{
    pub fn commit(&mut self) -> std::result::Result<(), SerializationError> {
        if self.dirty {
            self.dirty = false;
            self.dao.auth_set(self.auth.clone())?;
        }
        Ok(())
    }
}

impl<'a> Deref
for DaoAuthGuard<'a>
{
    type Target = MetaAuthorization;

    fn deref(&self) -> &Self::Target {
        &self.auth
    }
}

impl<'a> DerefMut
for DaoAuthGuard<'a>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;
        &mut self.auth
    }
}

impl<'a> Drop
for DaoAuthGuard<'a>
{
    fn drop(&mut self) {
        if self.dirty {
            self.commit().expect("Failed to commit the data header after accessing it")
        }
    }
}

pub struct DaoMutGuard<'a, D>
where D: Serialize
{
    dao: &'a mut DaoMut<D>,
    dirty: bool
}

impl<'a, D> Deref
for DaoMutGuard<'a, D>
where D: Serialize
{
    type Target = D;

    fn deref(&self) -> &Self::Target {
        &self.dao.inner.row.data
    }
}

impl<'a, D> DerefMut
for DaoMutGuard<'a, D>
where D: Serialize
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;
        &mut self.dao.inner.row.data
    }
}

impl<'a, D> Drop
for DaoMutGuard<'a, D>
where D: Serialize
{
    fn drop(&mut self) {
        if self.dirty {
            self.dao.commit(true).expect("Failed to commit the data header after accessing it");
        }
    }
}

pub struct DaoMutGuardOwned<D>
where D: Serialize
{
    dao: DaoMut<D>,
    dirty: bool
}

impl<D> DaoMutGuardOwned<D>
where D: Serialize
{
    pub fn trans(&self) -> Arc<DioMut> {
        self.dao.trans()
    }
}

impl<D> DaoObj
for DaoMutGuardOwned<D>
where D: Serialize
{
    fn key(&self) -> &PrimaryKey
    {
        self.dao.key()
    }

    fn auth(&self) -> &MetaAuthorization
    {
        self.dao.auth()
    }

    fn dio(&self) -> &Arc<Dio>
    {
        self.dao.dio()
    }

    fn when_created(&self) -> u64
    {
        self.dao.when_created()
    }

    fn when_updated(&self) -> u64
    {
        self.dao.when_updated()
    }
}

impl<D> Deref
for DaoMutGuardOwned<D>
where D: Serialize
{
    type Target = D;

    fn deref(&self) -> &Self::Target {
        &self.dao.inner.row.data
    }
}

impl<D> DerefMut
for DaoMutGuardOwned<D>
where D: Serialize
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dirty = true;
        &mut self.dao.inner.row.data
    }
}

impl<D> Drop
for DaoMutGuardOwned<D>
where D: Serialize
{
    fn drop(&mut self) {
        if self.dirty {
            self.dao.commit(true).expect("Failed to commit the data header after accessing it");
        }
    }
}

impl<D> From<DaoMut<D>>
for Dao<D>
where D: Serialize
{
    fn from(a: DaoMut<D>) -> Self {
        a.inner
    }
}