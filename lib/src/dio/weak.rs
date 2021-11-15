#![allow(unused_imports)]
use error_chain::bail;
use std::marker::PhantomData;
use std::sync::{Arc, Weak};
use tracing::{debug, error, info, instrument, span, trace, warn, Level};
use tracing_futures::Instrument;

use super::dio::DioWeak;
use super::dio_mut::DioMutWeak;
use crate::dio::dao::*;
use crate::dio::*;
use crate::error::*;
use crate::header::*;
use serde::de::*;
use serde::*;

/// Rerepresents a reference to another data object with strong
/// type linting to make the model more solidified
///
#[derive(Serialize, Deserialize)]
pub struct DaoWeak<D> {
    pub(super) id: Option<PrimaryKey>,
    #[serde(skip)]
    pub(super) dio: DioWeak,
    #[serde(skip)]
    pub(super) dio_mut: DioMutWeak,
    #[serde(skip)]
    pub(super) _marker: PhantomData<D>,
}

impl<D> Clone for DaoWeak<D> {
    fn clone(&self) -> Self {
        DaoWeak {
            id: self.id.clone(),
            dio: self.dio.clone(),
            dio_mut: self.dio_mut.clone(),
            _marker: PhantomData,
        }
    }
}

impl<D> Default for DaoWeak<D> {
    fn default() -> Self {
        DaoWeak::new()
    }
}

impl<D> std::fmt::Debug for DaoWeak<D>
where
    D: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let type_name = std::any::type_name::<D>();
        match self.id {
            Some(id) => write!(f, "dao-weak(key={}, type={})", id, type_name),
            None => write!(f, "dao-weak(type={})", type_name),
        }
    }
}

impl<D> DaoWeak<D> {
    pub fn new() -> DaoWeak<D> {
        DaoWeak {
            id: None,
            dio: DioWeak::Uninitialized,
            dio_mut: DioMutWeak::Uninitialized,
            _marker: PhantomData,
        }
    }

    pub fn from_key(dio: &Arc<DioMut>, key: PrimaryKey) -> DaoWeak<D> {
        DaoWeak {
            id: Some(key),
            dio: DioWeak::from(&dio.dio),
            dio_mut: DioMutWeak::from(dio),
            _marker: PhantomData,
        }
    }

    pub fn key(&self) -> Option<PrimaryKey> {
        self.id
    }

    pub fn set_key(&mut self, val: PrimaryKey) {
        self.id = Some(val);
    }

    pub fn clear(&mut self) {
        self.id = None;
    }

    pub fn dio(&self) -> Option<Arc<Dio>> {
        match &self.dio {
            DioWeak::Uninitialized => None,
            DioWeak::Weak(a) => Weak::upgrade(a),
        }
    }

    pub fn dio_mut(&self) -> Option<Arc<DioMut>> {
        match &self.dio_mut {
            DioMutWeak::Uninitialized => None,
            DioMutWeak::Weak(a) => Weak::upgrade(a),
        }
    }

    /// Loads the data object (if it exists)
    pub async fn load(&self) -> Result<Option<Dao<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let id = match self.id {
            Some(a) => a,
            None => {
                return Ok(None);
            }
        };

        let ret = {
            if let Some(dio) = self.dio_mut() {
                match dio.load::<D>(&id).await {
                    Ok(a) => Some(a.inner),
                    Err(LoadError(LoadErrorKind::NotFound(_), _)) => None,
                    Err(err) => {
                        bail!(err);
                    }
                }
            } else {
                let dio = match self.dio() {
                    Some(a) => a,
                    None => bail!(LoadErrorKind::WeakDio),
                };

                match dio.load::<D>(&id).await {
                    Ok(a) => Some(a),
                    Err(LoadError(LoadErrorKind::NotFound(_), _)) => None,
                    Err(err) => {
                        bail!(err);
                    }
                }
            }
        };
        Ok(ret)
    }

    /// Loads the data object (if it exists)
    pub async fn load_mut(&mut self) -> Result<Option<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let id = match self.id {
            Some(a) => a,
            None => {
                return Ok(None);
            }
        };

        let dio = match self.dio_mut() {
            Some(a) => a,
            None => bail!(LoadErrorKind::WeakDio),
        };

        let ret = match dio.load::<D>(&id).await {
            Ok(a) => Some(a),
            Err(LoadError(LoadErrorKind::NotFound(_), _)) => None,
            Err(err) => {
                bail!(err);
            }
        };
        Ok(ret)
    }

    /// Stores the data within this reference
    pub fn store(&mut self, value: D) -> Result<DaoMut<D>, SerializationError>
    where
        D: Clone + Serialize + DeserializeOwned,
    {
        let dio = match self.dio_mut() {
            Some(a) => a,
            None => bail!(SerializationErrorKind::WeakDio),
        };

        let ret = dio.store::<D>(value)?;
        self.id = Some(ret.key().clone());
        Ok(ret)
    }

    /// Loads the data object or uses a default if none exists
    pub async fn unwrap_or(&mut self, default: D) -> Result<D, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        match self.load().await? {
            Some(a) => Ok(a.take()),
            None => Ok(default),
        }
    }

    /// Loads the data object or uses a default if none exists
    pub async fn unwrap_or_else<F: FnOnce() -> D>(&mut self, f: F) -> Result<D, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        match self.load().await? {
            Some(a) => Ok(a.take()),
            None => Ok(f()),
        }
    }

    /// Loads the data object or creates a new one (if it does not exist)
    pub async fn unwrap_or_default(&mut self) -> Result<D, LoadError>
    where
        D: Serialize + DeserializeOwned + Default,
    {
        Ok(self
            .unwrap_or_else(|| {
                let ret: D = Default::default();
                ret
            })
            .await?)
    }

    pub async fn expect(&self, msg: &str) -> Dao<D>
    where
        D: Serialize + DeserializeOwned,
    {
        match self.load().await {
            Ok(Some(a)) => a,
            Ok(None) => {
                panic!("{}", msg);
            }
            Err(err) => {
                panic!("{}: {:?}", msg, err);
            }
        }
    }

    pub async fn unwrap(&self) -> Dao<D>
    where
        D: Serialize + DeserializeOwned,
    {
        self.load()
            .await
            .ok()
            .flatten()
            .expect("called `DaoRef::unwrap()` that failed to load")
    }

    pub async fn take(&mut self) -> Result<Option<DaoMut<D>>, LoadError>
    where
        D: Serialize + DeserializeOwned,
    {
        let key = self.id.take();
        self.id = None;

        let id = match key {
            Some(a) => a,
            None => {
                return Ok(None);
            }
        };

        let dio = match self.dio_mut() {
            Some(a) => a,
            None => bail!(LoadErrorKind::WeakDio),
        };

        let ret = match dio.load::<D>(&id).await {
            Ok(a) => Some(a),
            Err(LoadError(LoadErrorKind::NotFound(_), _)) => None,
            Err(err) => {
                bail!(err);
            }
        };
        Ok(ret)
    }

    pub async fn replace(&mut self, value: D) -> Result<Option<DaoMut<D>>, LoadError>
    where
        D: Clone + Serialize + DeserializeOwned,
    {
        let dio = match self.dio_mut() {
            Some(a) => a,
            None => bail!(LoadErrorKind::WeakDio),
        };

        let ret = dio.store::<D>(value)?;

        let key = self.id.replace(ret.key().clone());
        let id = match key {
            Some(a) => a,
            None => {
                return Ok(None);
            }
        };

        let ret = match dio.load::<D>(&id).await {
            Ok(a) => Some(a),
            Err(LoadError(LoadErrorKind::NotFound(_), _)) => None,
            Err(err) => {
                bail!(err);
            }
        };
        Ok(ret)
    }

    pub async fn is_some(&self) -> Result<bool, LoadError> {
        let id = match self.id {
            Some(a) => a,
            None => {
                return Ok(false);
            }
        };

        let ret = {
            if let Some(dio) = self.dio_mut() {
                dio.exists(&id).await
            } else {
                let dio = match self.dio() {
                    Some(a) => a,
                    None => bail!(LoadErrorKind::WeakDio),
                };
                dio.exists(&id).await
            }
        };
        Ok(ret)
    }

    pub async fn is_none(&self) -> Result<bool, LoadError> {
        Ok(!self.is_some().await?)
    }
}
