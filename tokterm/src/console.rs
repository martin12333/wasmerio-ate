#![allow(unused_imports)]
#![allow(dead_code)]
#[allow(unused_imports, dead_code)]
use tracing::{info, error, debug, trace, warn};
use xterm_js_rs::{Terminal};
use std::collections::HashMap;
use tokio::sync::mpsc;
use wasm_bindgen::JsCast;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::RwLock;
use std::path::Path;
use wasmer_wasi::vfs::FileSystem;

use crate::tty::TtyMode;

use super::eval::*;
use super::common::*;
use super::fd::*;
use super::state::*;
use super::environment::*;
use super::builtins::*;
use super::pool::ThreadPool as Pool;
use super::err;
use super::stdout::*;
use super::tty::*;
use super::reactor::*;
use super::job::*;
use super::stdio::*;
use super::bin::*;
use super::fs::*;

pub struct Console
{
    terminal: Terminal,
    state: Arc<Mutex<ConsoleState>>,
    bins: BinFactory,
    tok: TokeraSocketFactory,
    tty: Tty,
    pool: Pool,
    reactor: Arc<RwLock<Reactor>>,
    mounts: UnionFileSystem,
    stdout: Stdout,
    stderr: RawFd,
}

impl Console
{
    pub fn new(terminal: Terminal, pool: Pool) -> Console
    {
        let mut reactor = Reactor::new();

        let state = Arc::new(Mutex::new(ConsoleState::new()));
        let (stdout, mut stdout_rx) = reactor.pipe_out().unwrap();
        let (stderr, mut stderr_rx) = reactor.pipe_out().unwrap();

        let stdout = reactor.fd(stdout);
        let stdout = Stdout::new(stdout);

        let reactor = Arc::new(RwLock::new(reactor));

        // Stdout
        {
            let terminal: Terminal = terminal.clone().dyn_into().unwrap();
            wasm_bindgen_futures::spawn_local(async move {
                while let Some(data) = stdout_rx.recv().await {
                    let text = String::from_utf8_lossy(&data[..])[..].replace("\n", "\r\n");
                    terminal.write(text.as_str());
                }
            });
        }

        // Stderr
        {
            let terminal: Terminal = terminal.clone().dyn_into().unwrap();
            wasm_bindgen_futures::spawn_local(async move {
                while let Some(data) = stderr_rx.recv().await {
                    let text = String::from_utf8_lossy(&data[..])[..].replace("\n", "\r\n");
                    terminal.write(text.as_str());
                }
            });
        }

        let tty = Tty::new(stdout.clone());

        let mounts = super::fs::create_root_fs();

        Console {
            terminal,
            bins: BinFactory::new(),
            tok: TokeraSocketFactory::new(&reactor),
            state,
            stdout,
            stderr,
            tty,
            reactor,
            pool,
            mounts,
        }
    }

    pub async fn init(&mut self)
    {
        let cols = self.terminal.get_cols();
        let rows = self.terminal.get_rows();
        self.tty.set_bounds(cols, rows).await;  

        Console::update_prompt(false, &self.state, &self.tty).await;

        self.tty.draw_welcome().await;
        self.tty.draw_prompt().await;        
    }

    pub fn tty(&self) -> &Tty
    {
        &self.tty
    }

    pub async fn on_enter(&mut self)
    {
        let mode = self.tty.mode().await;

        let pushed = self.tty.draw_undo().await;
        self.tty.set_cursor_to_end().await;
        self.tty.draw_fixed(pushed.as_str()).await;
        self.tty.draw("\n\r").await;

        let mut cmd = self.tty.get_paragraph().await;

        if let TtyMode::StdIn(job) = mode {
            cmd += "\n";
            self.tty.reset_line().await;
            self.tty.clear_paragraph().await;
            let _ = job.stdin_tx.send(cmd.into_bytes()).await;
            return;
        }

        if cmd.len() <= 0 {
            self.tty.reset_line().await;
            self.tty.draw_prompt().await;
            return;
        }

        let state = self.state.clone();
        let reactor = self.reactor.clone();
        let pool = self.pool.clone();
        let (env, last_return, path) = {
            let state = self.state.lock().unwrap();
            let env = state.env.clone();
            let last_return = state.last_return;
            let path = state.path.clone();
            (env, last_return, path)
        };

        let (job, stdio) = {
            let mut reactor = reactor.write().await;
            let (stdin, stdin_tx) = match reactor.pipe_in(ReceiverMode::Stream) {
                Ok(a) => a,
                Err(_) => {
                    drop(reactor);
                    self.tty.draw("term: insufficient file handle space\r\n").await;
                    self.tty.reset_line().await;
                    self.tty.draw_prompt().await;
                    return;
                }
            };
            let stdio = Stdio {
                stdin: reactor.fd(stdin),
                stdout: reactor.fd(self.stdout.raw.clone()),
                stderr: reactor.fd(self.stderr.clone()),
                tty: self.tty.clone(),
                tok: self.tok.clone(),
                root: self.mounts.clone(),
            };

            let job = match reactor.generate_job(stdio.clone(), stdin_tx) {
                Ok((_, job)) => job,
                Err(_) => {
                    drop(reactor);
                    self.tty.draw("term: insufficient job space\r\n").await;
                    self.tty.reset_line().await;
                    self.tty.draw_prompt().await;
                    return;
                }
            };
            (job, stdio)
        };

        let ctx = EvalContext {
            env,
            bins: self.bins.clone(),
            job_list: job.job_list_tx.clone(),
            last_return,
            reactor: reactor.clone(),
            pool,
            path,
            input: cmd.clone(),
            stdio
        };
        
        let rx = eval(ctx).await;

        let mut tty = self.tty.clone();
        tty.reset_line().await;
        tty.clear_paragraph().await;
        tty.enter_mode(TtyMode::StdIn(job), &self.reactor).await;

        wasm_bindgen_futures::spawn_local(async move
        {
            let rx = rx.await;
            
            tty.reset_line().await;
            tty.clear_paragraph().await;
            tty.enter_mode(TtyMode::Console, &reactor).await;

            let record_history = if let Some(history) = tty.get_selected_history().await {
                history != cmd
            } else {
                true
            };
            tty.reset_history_cursor().await;
            
            let mut multiline_input = false;
            match rx {
                Ok(EvalPlan::Executed { code, ctx, show_result }) => {
                    debug!("eval executed (code={})", code);
                    if code != 0 && show_result {
                        let mut chars = String::new();
                        chars += err::exit_code_to_message(code);
                        chars += "\r\n";
                        tty.draw(chars.as_str()).await;
                    }
                    {
                        let mut state = state.lock().unwrap();
                        state.last_return = code;
                        state.env = ctx.env;
                        state.path = ctx.path;
                    }
                    if record_history {
                        tty.record_history(cmd).await;
                    }
                },
                Ok(EvalPlan::InternalError) => {
                    debug!("eval internal error");
                    tty.draw("term: internal error\r\n").await;
                }
                Ok(EvalPlan::MoreInput) => {
                    debug!("eval more input");
                    multiline_input = true;
                    tty.add(cmd.as_str()).await;
                }
                Ok(EvalPlan::Invalid) => {
                    debug!("eval invalid");
                    tty.draw("term: invalid command\r\n").await;
                },
                Err(err) => {
                    debug!("eval recv error (err={})", err);
                    tty.draw(format!("term: command failed - {} \r\n", err).as_str()).await;
                }
            };
            tty.reset_line().await;
            Console::update_prompt(multiline_input, &state, &tty).await;
            tty.draw_prompt().await;
        });
    }

    async fn update_prompt(multiline_input: bool, state: &Arc<Mutex<ConsoleState>>, tty: &Tty)
    {
        let (prompt, prompt_color) = {
            let state = state.lock().unwrap();
            let prompt = state.compute_prompt(multiline_input, false);
            let prompt_color = state.compute_prompt(multiline_input, true);
            (prompt, prompt_color)
        };

        tty.set_prompt(prompt, prompt_color).await;
    }

    pub async fn on_ctrl_l(&mut self)
    {
        self.tty.reset_line().await;
        self.tty.draw_prompt().await;
        self.terminal.clear();
    }

    pub async fn on_ctrl_c(&mut self, job: Option<Job>)
    {
        if job.is_none() {
            self.tty.draw("\r\n").await;
        }
        else {
            self.tty.draw("^C\r\n").await;
        }

        let mode = self.tty.mode().await;
        match mode {
            TtyMode::Null => {
            },
            TtyMode::Console => {
                self.tty.clear_paragraph().await;
                self.tty.reset_line().await;
                Console::update_prompt(false, &self.state, &self.tty).await;
                self.tty.draw_prompt().await;
            },
            TtyMode::StdIn(job) => {
                {
                    let mut reactor = self.reactor.write().await;
                    reactor.close_job(job, err::ERR_TERMINATED);
                }
                self.tty.enter_mode(TtyMode::Null, &self.reactor).await;
            }
        }
    }

    pub async fn on_resize(&mut self)
    {
        let cols = self.terminal.get_cols();
        let rows = self.terminal.get_rows();
        self.tty.set_bounds(cols, rows).await;
    }

    pub async fn on_parse(&mut self, data: &str, job: Option<Job>) {
        // debug!("on_parse {}", data.as_bytes().iter().map(|byte| format!("\\u{{{:04X}}}", byte).to_owned()).collect::<Vec<String>>().join(""));
        match data {
            "\r" => {
                self.on_enter().await;
            },
            "\u{0003}" => { // Ctrl-C
                self.on_ctrl_c(job).await;
            },
            "\u{007F}" => {
                self.tty.backspace().await;
            },
            "\u{001B}\u{005B}\u{0044}" => {
                self.tty.cursor_left().await;
            },
            "\u{001B}\u{005B}\u{0043}" => {
                self.tty.cursor_right().await;
            },
            "\u{0001}" => {
                self.tty.set_cursor_to_start().await;
            },
            "\u{001B}\u{005B}\u{0041}" => {
                if job.is_none() {
                    self.tty.cursor_up().await;
                }
            },
            "\u{001B}\u{005B}\u{0042}" => {
                if job.is_none() {
                    self.tty.cursor_down().await;
                }
            },
            "\u{000C}" => {
                self.on_ctrl_l().await;
            },
            data => {
                self.tty.add(data).await;
            }
        }
    }

    pub async fn on_key(&mut self, _key_code: u32, _key: String, _alt_key: bool, _ctrl_key: bool, _meta_key: bool)
    {
        // Do nothing for now
    }

    pub async fn on_data(&mut self, mut data: String) {
        let mode = self.tty.mode().await;
        match mode {
            TtyMode::StdIn(job) => {
                // Ctrl-C is not fed to the process and always actioned
                if data == "\u{0003}" {
                    self.on_ctrl_c(Some(job)).await

                // Buffered input will only be sent to the process once a return key is pressed
                // which allows the line to be 'edited' in the terminal before its submitted
                } else if self.tty.is_buffering() {
                    self.on_parse(&data, Some(job)).await

                // When we are sending unbuffered keys the return key is turned into a newline so that its compatible
                // with things like the rpassword crate which simple reads a line of input with a line feed terminator
                // from TTY.
                } else if data == "\r" {
                    data = "\n".to_string();
                    let _ = job.stdin_tx.send(data.into_bytes()).await;

                // Otherwise we just feed the bytes into the STDIN for the process to handle
                } else {
                    let _ = job.stdin_tx.send(data.into_bytes()).await;
                }
            }
            TtyMode::Null => {
            }
            TtyMode::Console => {
                self.on_parse(&data, None).await
            }
        }
    }
}