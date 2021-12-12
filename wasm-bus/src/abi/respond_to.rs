use derivative::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};

use crate::abi::CallError;
use crate::abi::CallHandle;
use crate::task::spawn;

#[derive(Derivative, Clone, Default)]
#[derivative(Debug)]
pub struct RespondToService {
    #[derivative(Debug = "ignore")]
    pub(crate) callbacks: Arc<
        Mutex<
            HashMap<
                CallHandle,
                Arc<
                    dyn Fn(
                            CallHandle,
                            Vec<u8>,
                        )
                            -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CallError>> + Send>>
                        + Send
                        + Sync,
                >,
            >,
        >,
    >,
}

impl RespondToService {
    pub fn add(
        &self,
        parent: CallHandle,
        callback: Arc<
            dyn Fn(
                    CallHandle,
                    Vec<u8>,
                )
                    -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CallError>> + Send>>
                + Send
                + Sync,
        >,
    ) {
        let mut callbacks = self.callbacks.lock().unwrap();
        callbacks.insert(parent, callback);
    }

    pub fn remove(
        &self,
        handle: &CallHandle
    ) {
        let mut callbacks = self.callbacks.lock().unwrap();
        callbacks.remove(handle);
    }

    pub fn process(&self, parent: CallHandle, handle: CallHandle, request: Vec<u8>) {
        let callback = {
            let callbacks = self.callbacks.lock().unwrap();
            if let Some(callback) = callbacks.get(&parent) {
                Arc::clone(callback)
            } else {
                spawn(async move {
                    let err: u32 = CallError::InvalidHandle.into();
                    crate::abi::syscall::fault(handle, err as u32);
                    crate::engine::BusEngine::remove(&handle);
                });
                return;
            }
        };

        spawn(async move {
            let res = callback.as_ref()(handle, request);
            match res.await {
                Ok(a) => {
                    crate::abi::syscall::reply(handle, &a[..]);
                }
                Err(err) => {
                    let err: u32 = err.into();
                    crate::abi::syscall::fault(handle, err as u32);
                }
            }
            crate::engine::BusEngine::remove(&handle);
        });
    }
}