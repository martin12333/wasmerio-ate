#![allow(unused_imports)]
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};

use std::borrow::Borrow;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::Semaphore;
use std::rc::Rc;
use std::cell::RefCell;
use std::ops::Deref;
use std::ops::DerefMut;
use derivative::*;

use js_sys::{JsString, Promise};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::sync::Arc;
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, WorkerOptions, WorkerType};
use xterm_js_rs::Terminal;
use term_lib::api::ThreadLocal;
use term_lib::common::MAX_MPSC;

use super::common::*;
use super::fd::*;
use super::interval::*;
use super::tty::Tty;

pub type BoxRun<'a, T> =
    Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T> + 'static>> + Send + 'a>;

pub type BoxRunWithThreadLocal<'a, T> =
    Box<dyn FnOnce(Rc<RefCell<ThreadLocal>>) -> Pin<Box<dyn Future<Output = T> + 'static>> + Send + 'a>;

trait AssertSendSync: Send + Sync {}
impl AssertSendSync for WebThreadPool {}

#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct WebThreadPool {
    pool_reactors: Arc<PoolState>,
    pool_stateful: Arc<PoolState>,
    pool_blocking: Arc<PoolState>,
}

enum Message {
    Run(BoxRun<'static, ()>),
    RunWithThreadLocal(BoxRunWithThreadLocal<'static, ()>),
}

impl Debug
for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Run(_) => write!(f, "run-shared"),
            Message::RunWithThreadLocal(_) => write!(f, "run-dedicated"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PoolType {
    Shared,
    Stateful,
    Dedicated,
}

struct IdleThread
{
    idx: usize,
    work: mpsc::Sender<Message>
}

impl IdleThread
{
    fn consume(self, msg: Message) {
        let _ = self.work.blocking_send(msg);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct PoolState {
    #[derivative(Debug = "ignore")]
    idle_rx: Mutex<mpsc::Receiver<IdleThread>>,
    idle_tx: mpsc::Sender<IdleThread>,
    size: AtomicUsize,
    min_size: usize,
    max_size: usize,
    #[allow(dead_code)]
    type_: PoolType,
}

pub struct ThreadState {
    pool: Arc<PoolState>,
    #[allow(dead_code)]
    idx: usize,
    tx: mpsc::Sender<Message>,
    rx: Mutex<Option<mpsc::Receiver<Message>>>,
    init: Mutex<Option<Message>>,
}

#[wasm_bindgen]
pub struct LoaderHelper {}
#[wasm_bindgen]
impl LoaderHelper {
    #[wasm_bindgen(js_name = mainJS)]
    pub fn main_js(&self) -> JsString {
        #[wasm_bindgen]
        extern "C" {
            #[wasm_bindgen(js_namespace = ["import", "meta"], js_name = url)]
            static URL: JsString;
        }

        URL.clone()
    }
}

#[wasm_bindgen(module = "/public/worker.js")]
extern "C" {
    #[wasm_bindgen(js_name = "startWorker")]
    fn start_worker(
        module: JsValue,
        memory: JsValue,
        shared_data: JsValue,
        opts: WorkerOptions,
        builder: LoaderHelper,
    ) -> Promise;
}

impl WebThreadPool {
    pub fn new(size: usize) -> Result<WebThreadPool, JsValue> {
        info!("pool::create(size={})", size);

        let (tx1, rx1) = mpsc::channel(MAX_MPSC);
        let (tx2, rx2) = mpsc::channel(MAX_MPSC);
        let (tx3, rx3) = mpsc::channel(MAX_MPSC);

        let pool_reactors = Arc::new(PoolState {
            idle_rx: Mutex::new(rx1),
            idle_tx: tx1,
            size: AtomicUsize::new(0),
            min_size: 1,
            max_size: size,
            type_: PoolType::Shared,
        });

        let pool_stateful = Arc::new(PoolState {
            idle_rx: Mutex::new(rx2),
            idle_tx: tx2,
            size: AtomicUsize::new(0),
            min_size: 1,
            max_size: 1000usize,
            type_: PoolType::Stateful,
        });

        let pool_blocking = Arc::new(PoolState {
            idle_rx: Mutex::new(rx3),
            idle_tx: tx3,
            size: AtomicUsize::new(0),
            min_size: 1,
            max_size: 1000usize,
            type_: PoolType::Dedicated,
        });

        let pool = WebThreadPool {
            pool_reactors,
            pool_stateful,
            pool_blocking,
        };

        Ok(pool)
    }

    pub fn new_with_max_threads() -> Result<WebThreadPool, JsValue> {
        #[wasm_bindgen]
        extern "C" {
            #[wasm_bindgen(js_namespace = navigator, js_name = hardwareConcurrency)]
            static HARDWARE_CONCURRENCY: usize;
        }
        let pool_size = std::cmp::max(*HARDWARE_CONCURRENCY, 1);
        debug!("pool::max_threads={}", pool_size);
        Self::new(pool_size)
    }

    pub fn spawn_shared(&self, task: BoxRun<'static, ()>) {
        self.pool_reactors.spawn(Message::Run(task));
    }

    pub fn spawn_stateful(&self, task: BoxRunWithThreadLocal<'static, ()>) {
        self.pool_stateful.spawn(Message::RunWithThreadLocal(task));
    }

    pub fn spawn_dedicated(&self, task: BoxRun<'static, ()>) {
        self.pool_blocking.spawn(Message::Run(task));
    }
}

impl PoolState {
    fn spawn(self: &Arc<Self>, msg: Message) {
        let thread  = {
            let mut idle_rx = self.idle_rx.lock().unwrap();
            idle_rx.try_recv().ok()
        };

        if let Some(thread) = thread {
            thread.consume(msg);
            return;
        }

        let (tx, rx) = mpsc::channel(MAX_MPSC);
        let idx = self.size.fetch_add(1usize, Ordering::Release);
        let state = Arc::new(
            ThreadState
            {
                pool: Arc::clone(self),
                idx,
                tx,
                rx: Mutex::new(Some(rx)),
                init: Mutex::new(Some(msg)),
            }
        );
        Self::start_worker_now(idx, state, None);
    }

    pub fn start_worker_now(
        idx: usize,
        state: Arc<ThreadState>,
        should_warn_on_error: Option<Terminal>,
    ) {
        let mut opts = WorkerOptions::new();
        opts.type_(WorkerType::Module);
        opts.name(&*format!("Worker-{:?}-{}", state.pool.type_, idx));

        let ptr = Arc::into_raw(state);

        let result = wasm_bindgen_futures::JsFuture::from(start_worker(
            wasm_bindgen::module(),
            wasm_bindgen::memory(),
            JsValue::from(ptr as u32),
            opts,
            LoaderHelper {},
        ));

        wasm_bindgen_futures::spawn_local(async move {
            let ret = result.await;
            if let Err(err) = ret {
                let err = err
                    .as_string()
                    .unwrap_or_else(|| "unknown error".to_string());
                error!("failed to start worker thread - {}", err);

                if let Some(term) = should_warn_on_error {
                    term.write(
                        Tty::BAD_WORKER
                            .replace("\n", "\r\n")
                            .replace("\\x1B", "\x1B")
                            .replace("{error}", err.as_str())
                            .as_str(),
                    );
                }

                return;
            }
        });
    }
}

impl ThreadState {
    fn work(state: Arc<ThreadState>) {
        let thread_index = state.idx;
        info!("worker started (index={}, type={:?})", thread_index, state.pool.type_);

        // Load the work queue receiver where other people will
        // send us the work that needs to be done
        let mut work_rx = {
            let mut lock = state.rx.lock().unwrap();
            lock.take().unwrap()
        };

        // Load the initial work
        let mut work = {
            let mut lock = state.init.lock().unwrap();
            lock.take()
        };
        
        // The work is done in an asynchronous engine (that supports Javascript)
        let thread_local = Rc::new(RefCell::new(ThreadLocal::default()));
        let work_tx = state.tx.clone();
        let pool = Arc::clone(&state.pool);
        let driver = async move {
            let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();

            loop {
                // Process work until we need to go idle
                while let Some(task) = work
                {
                    match task {
                        Message::RunWithThreadLocal(task) => {
                            let thread_local = thread_local.clone();
                            task(thread_local).await;
                        }
                        Message::Run(task) => {
                            let future = task();
                            wasm_bindgen_futures::spawn_local(async move {
                                future.await;
                            });
                        }
                    }

                    // Grab the next work
                    work = work_rx.try_recv().ok();
                }

                // If there iss already an idle thread thats older then
                // keep that one (otherwise ditch it) - this creates negative
                // pressure on the pool size.
                // The reason we keep older threads is to maximize cache hits such
                // as module compile caches.
                if let Ok(mut lock) = state.pool.idle_rx.try_lock() {
                    if let Ok(other) = lock.try_recv() {
                        if other.idx < thread_index {
                            drop(lock);
                            if let Ok(_) = state.pool.idle_tx.send(other).await {
                                info!("worked closed (index={}, type={:?})", thread_index, pool.type_);
                                break;
                            }
                        }
                    }
                }

                // Now register ourselves as idle
                let idle = IdleThread {
                    idx: thread_index,
                    work: work_tx.clone()
                };
                if let Err(_) = state.pool.idle_tx.send(idle).await {
                    info!("pool is closed (thread_index={}, type={:?})", thread_index, pool.type_);
                    break;
                }

                // Do a blocking recv (if this fails the thread is closed)
                work = match work_rx.recv().await {
                    Some(a) => Some(a),
                    None => {
                        info!("worked closed (index={}, type={:?})", thread_index, pool.type_);
                        break;
                    }
                };
            }

            global.close();
        };
        wasm_bindgen_futures::spawn_local(driver);
    }
}

#[wasm_bindgen(skip_typescript)]
pub fn worker_entry_point(state_ptr: u32) {
    let state = unsafe { Arc::<ThreadState>::from_raw(state_ptr as *const ThreadState) };

    let name = js_sys::global()
        .unchecked_into::<DedicatedWorkerGlobalScope>()
        .name();
    debug!("{}: Entry", name);
    ThreadState::work(state);
}
