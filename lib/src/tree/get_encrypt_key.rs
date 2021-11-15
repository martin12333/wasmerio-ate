#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::crypto::*;
use crate::error::*;
use crate::meta::*;
use crate::session::*;
use crate::transaction::*;

use super::*;

impl TreeAuthorityPlugin {
    pub(super) fn get_encrypt_key(
        &self,
        meta: &Metadata,
        confidentiality: &MetaConfidentiality,
        iv: Option<&InitializationVector>,
        session: &'_ dyn AteSession,
    ) -> Result<Option<EncryptKey>, TransformError> {
        let trans_meta = TransactionMetadata::default();
        let auth_store;
        let auth = match &confidentiality._cache {
            Some(a) => a,
            None => {
                auth_store = self.compute_auth(meta, &trans_meta, ComputePhase::AfterStore)?;
                &auth_store.read
            }
        };

        match auth {
            ReadOption::Inherit => Err(TransformErrorKind::UnspecifiedReadability.into()),
            ReadOption::Everyone(key) => {
                if let Some(_iv) = iv {
                    if let Some(key) = key {
                        return Ok(Some(key.clone()));
                    }
                }
                Ok(None)
            }
            ReadOption::Specific(key_hash, derived) => {
                for key in session.read_keys(AteSessionKeyCategory::AllKeys) {
                    if key.hash() == *key_hash {
                        let inner = derived.transmute(key)?;
                        if inner.short_hash() == confidentiality.hash {
                            return Ok(Some(inner));
                        }
                    }
                }
                for key in session.private_read_keys(AteSessionKeyCategory::AllKeys) {
                    if key.hash() == *key_hash {
                        let inner = derived.transmute_private(key)?;
                        if inner.short_hash() == confidentiality.hash {
                            return Ok(Some(inner));
                        }
                    }
                }
                Err(TransformErrorKind::MissingReadKey(key_hash.clone()).into())
            }
        }
    }
}
