use ate_crypto::SerializationFormat;
use serde::*;
pub use wasmer_bus::prelude::CallHandle;
pub use wasmer_bus::prelude::BusError;
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstanceCall {
    #[serde(default)]
    pub parent: Option<u64>,
    pub handle: u64,
    pub format: SerializationFormat,
    pub binary: String,
    pub topic: String,
}

impl fmt::Display
for InstanceCall
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.topic)?;
        if let Some(parent) = self.parent {
            write!(f, "parent={},", parent)?;
        }
        write!(f, ")")
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum InstanceCommand {
    Shell,
    Call(InstanceCall),
}

impl fmt::Display
for InstanceCommand
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstanceCommand::Shell => write!(f, "shell"),
            InstanceCommand::Call(call) => write!(f, "call({})", call),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum InstanceReply {
    FeedBytes {
        handle: CallHandle,
        format: SerializationFormat,
        data: Vec<u8>
    },
    Stdout {
        data: Vec<u8>
    },
    Stderr {
        data: Vec<u8>
    },
    Error {
        handle: CallHandle,
        error: BusError
    },
    Terminate {
        handle: CallHandle
    },
    Exit
}

impl fmt::Display
for InstanceReply
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstanceReply::Stdout { data } => write!(f, "stdout(len={})", data.len()),
            InstanceReply::Stderr{ data } => write!(f, "stdout(len={})", data.len()),
            InstanceReply::FeedBytes { handle, format, data} => write!(f, "feed-bytes(handle={}, format={}, len={})", handle, format, data.len()),
            InstanceReply::Error { handle, error } => write!(f, "error(handle={}, {})", handle, error),
            InstanceReply::Terminate { handle, .. } => write!(f, "terminate(handle={})", handle),
            InstanceReply::Exit => write!(f, "exit"),
        }
    }
}