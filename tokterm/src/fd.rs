#![allow(dead_code)]
#![allow(unused_imports)]
use bytes::{Buf, BytesMut};
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;
use std::io::prelude::*;
use std::io::SeekFrom;
use std::sync::Mutex;
use std::{
    pin::Pin,
    sync::Arc,
    task::{self, Context, Poll, Waker},
};
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};
use wasmer_wasi::vfs::{FileDescriptor, VirtualFile};
use wasmer_wasi::{types as wasi_types, WasiFile, WasiFsError};

use super::common::*;
use super::err::*;
use super::pipe::*;
use super::poll::*;
use super::reactor::*;
use super::state::*;

#[derive(Debug, Clone)]
pub struct Fd {
    pub(crate) blocking: Arc<AtomicBool>,
    pub(crate) sender: Option<mpsc::Sender<Vec<u8>>>,
    pub(crate) receiver: Option<Arc<Mutex<ReactorPipeReceiver>>>,
}

impl Fd {
    pub fn new(
        tx: Option<mpsc::Sender<Vec<u8>>>,
        rx: Option<Arc<Mutex<ReactorPipeReceiver>>>,
    ) -> Fd {
        Fd {
            blocking: Arc::new(AtomicBool::new(true)),
            sender: tx,
            receiver: rx,
        }
    }

    pub fn combine(fd1: &Fd, fd2: &Fd) -> Fd {
        let mut ret = Fd {
            blocking: Arc::new(AtomicBool::new(fd1.blocking.load(Ordering::Relaxed))),
            sender: None,
            receiver: None,
        };

        if let Some(a) = fd1.sender.as_ref() {
            ret.sender = Some(a.clone());
        } else if let Some(a) = fd2.sender.as_ref() {
            ret.sender = Some(a.clone());
        }

        if let Some(a) = fd1.receiver.as_ref() {
            ret.receiver = Some(a.clone());
        } else if let Some(a) = fd2.receiver.as_ref() {
            ret.receiver = Some(a.clone());
        }

        ret
    }

    pub fn set_blocking(&self, blocking: bool) {
        self.blocking.store(blocking, Ordering::Relaxed);
    }

    pub async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(sender) = self.sender.as_mut() {
            let buf_len = buf.len();
            let buf = buf.to_vec();
            if let Err(_err) = sender.send(buf).await {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(buf_len)
        } else {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
    }

    pub(crate) async fn write_clear_line(&mut self) {
        let _ = self.write("\r\x1b[0K\r".as_bytes()).await;
    }

    pub(crate) fn blocking_write_clear_line(&mut self) {
        let _ = self.blocking_write("\r\x1b[0K\r".as_bytes());
    }

    pub fn blocking_write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(sender) = self.sender.as_mut() {
            let buf_len = buf.len();
            let buf = buf.to_vec();
            if let Err(_err) = sender.blocking_send(buf) {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(buf_len)
        } else {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
    }

    pub fn poll(&mut self) -> PollResult {
        poll_fd(self.receiver.as_mut(), self.sender.as_mut())
    }
}

impl Seek for Fd {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(io::ErrorKind::Other, "can not seek pipes"))
    }
}
impl Write for Fd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(sender) = self.sender.as_mut() {
            let buf_len = buf.len();
            let buf = buf.to_vec();
            let ret = if self.blocking.load(Ordering::Relaxed) {
                sender.blocking_send(buf)
            } else {
                match sender.try_send(buf) {
                    Ok(ret) => Ok(ret),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        return Err(std::io::ErrorKind::WouldBlock.into());
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return Ok(0);
                    }
                }
            };
            if let Err(_err) = ret {
                //return Err(std::io::ErrorKind::BrokenPipe.into());
                return Ok(0);
            }
            Ok(buf_len)
        } else {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Fd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(receiver) = self.receiver.as_mut() {
            let mut receiver = receiver.lock().unwrap();
            if receiver.buffer.has_remaining() == false {
                if receiver.mode == ReceiverMode::Message(true) {
                    receiver.mode = ReceiverMode::Message(false);
                    return Ok(0usize);
                }
                let ret = if self.blocking.load(Ordering::Relaxed) {
                    receiver.rx.blocking_recv()
                } else {
                    match receiver.rx.try_recv() {
                        Ok(ret) => Some(ret),
                        Err(mpsc::error::TryRecvError::Empty) => {
                            return Err(std::io::ErrorKind::WouldBlock.into());
                        }
                        Err(mpsc::error::TryRecvError::Disconnected) => None,
                    }
                };
                if let Some(data) = ret {
                    receiver.buffer.extend_from_slice(&data[..]);
                    if receiver.mode == ReceiverMode::Message(false) {
                        receiver.mode = ReceiverMode::Message(true);
                    }
                }
            }
            if receiver.buffer.has_remaining() {
                let max = receiver.buffer.remaining().min(buf.len());
                buf[0..max].copy_from_slice(&receiver.buffer[..max]);
                receiver.buffer.advance(max);
                return Ok(max);
            }
            Ok(0usize)
        } else {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
    }
}

impl VirtualFile for Fd {
    fn last_accessed(&self) -> u64 {
        0
    }
    fn last_modified(&self) -> u64 {
        0
    }
    fn created_time(&self) -> u64 {
        0
    }
    fn size(&self) -> u64 {
        0
    }
    fn set_len(&mut self, _new_size: wasi_types::__wasi_filesize_t) -> Result<(), WasiFsError> {
        Err(WasiFsError::PermissionDenied)
    }

    fn unlink(&mut self) -> Result<(), WasiFsError> {
        Ok(())
    }

    fn bytes_available(&self) -> Result<usize, WasiFsError> {
        Ok(0)
    }

    fn get_fd(&self) -> Option<FileDescriptor> {
        None
    }
}
