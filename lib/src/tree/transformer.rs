use bytes::Bytes;
use error_chain::bail;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::error::*;
use crate::meta::*;
use crate::session::*;
use crate::transaction::*;
use crate::transform::*;

use super::*;

impl EventDataTransformer for TreeAuthorityPlugin {
    fn clone_transformer(&self) -> Box<dyn EventDataTransformer> {
        Box::new(self.clone())
    }

    #[allow(unused_variables)]
    fn data_as_underlay(
        &self,
        meta: &mut Metadata,
        with: Bytes,
        session: &'_ dyn AteSession,
        trans_meta: &TransactionMetadata,
    ) -> Result<Bytes, TransformError> {
        let mut with = self
            .signature_plugin
            .data_as_underlay(meta, with, session, trans_meta)?;

        let cache = match meta.get_confidentiality() {
            Some(a) => a._cache.as_ref(),
            None => None,
        };

        let auth_store;
        let auth = match &cache {
            Some(a) => a,
            None => {
                auth_store = self.compute_auth(meta, trans_meta, ComputePhase::AfterStore)?;
                &auth_store.read
            }
        };

        if let Some((iv, key)) = self.generate_encrypt_key(auth, session)? {
            let encrypted = key.encrypt_with_iv(&iv, &with[..]);
            meta.core.push(CoreMetadata::InitializationVector(iv));
            with = Bytes::from(encrypted);
        }

        Ok(with)
    }

    #[allow(unused_variables)]
    fn data_as_overlay(
        &self,
        meta: &Metadata,
        with: Bytes,
        session: &'_ dyn AteSession,
    ) -> Result<Bytes, TransformError> {
        let mut with = self.signature_plugin.data_as_overlay(meta, with, session)?;

        let iv = meta.get_iv().ok();
        match meta.get_confidentiality() {
            Some(confidentiality) => {
                if let Some(key) = self.get_encrypt_key(meta, confidentiality, iv, session)? {
                    let iv = match iv {
                        Some(a) => a,
                        None => {
                            bail!(TransformErrorKind::CryptoError(
                                CryptoErrorKind::NoIvPresent
                            ));
                        }
                    };
                    let decrypted = key.decrypt(&iv, &with[..]);
                    with = Bytes::from(decrypted);
                }
            }
            None if iv.is_some() => {
                bail!(TransformErrorKind::UnspecifiedReadability);
            }
            None => {}
        };

        Ok(with)
    }
}
