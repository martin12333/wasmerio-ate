use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::sync::Arc;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::chain::Chain;
use crate::error::*;
use crate::event::*;
use crate::header::*;
use crate::service::ServiceHook;
use crate::session::AteSession;

use super::*;

#[async_trait]
pub trait Service
where
    Self: Send + Sync,
{
    fn filter(&self, evt: &EventWeakData) -> bool;

    async fn notify(&self, key: PrimaryKey) -> Result<(), InvokeError>;
}

impl Chain {
    pub fn add_service<CTX, REQ, RES, ERR, C, F>(
        self: &Arc<Self>,
        session: &'_ dyn AteSession,
        context: Arc<CTX>,
        callback: C,
    ) -> Arc<ServiceHook>
    where
        CTX: Send + Sync + 'static,
        REQ: DeserializeOwned + Send + Sync + Sized + 'static,
        RES: Serialize + Send + Sync + Sized + 'static,
        ERR: Serialize + Send + Sync + Sized + 'static,
        C: Fn(Arc<CTX>, REQ) -> F + Send + 'static,
        F: Future<Output = Result<RES, ERR>> + Send + 'static,
    {
        let svr = ServiceHandler::new(context, callback);
        let svr: Arc<dyn ServiceInvoker> = svr;
        self.add_generic_service(session.clone_session(), &svr)
    }

    pub fn add_generic_service(
        self: &Arc<Self>,
        session: Box<dyn AteSession>,
        handler: &Arc<dyn ServiceInvoker>,
    ) -> Arc<ServiceHook> {
        let ret = Arc::new(ServiceHook::new(self, session, handler));

        {
            let svr = Arc::clone(&ret);
            let svr: Arc<dyn Service> = svr;
            let mut guard = self.inside_sync.write().unwrap();
            guard.services.push(svr);
        }
        ret
    }
}
