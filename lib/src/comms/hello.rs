#![allow(unused_imports)]
use log::{info, warn, debug};
#[cfg(not(feature="websockets"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature="websockets")]
use tungstenite::protocol::Message;

use crate::error::*;
use serde::{Serialize, Deserialize};
use crate::crypto::KeySize;
use crate::spec::*;

use super::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Hello
{
    pub domain: Option<String>,
    pub key_size: Option<KeySize>,
    pub wire_format: Option<SerializationFormat>,
}

pub(super) async fn mesh_hello_exchange_sender(stream_rx: &mut StreamRx, stream_tx: &mut StreamTx, domain: Option<String>, mut key_size: Option<KeySize>) -> Result<(Option<KeySize>, SerializationFormat), CommsError>
{
    // Send over the hello message and wait for a response
    debug!("client sending hello");
    let hello_client = Hello {
        domain,
        key_size,
        wire_format: None,
    };
    let hello_client_bytes = serde_json::to_vec(&hello_client)?;
    stream_tx.write_16bit(hello_client_bytes).await?;

    // Read the hello message from the other side
    let hello_server_bytes = stream_rx.read_16bit().await?;
    debug!("client received hello from server");
    let hello_server: Hello = serde_json::from_slice(&hello_server_bytes[..])?;

    // Upgrade the key_size if the server is bigger
    key_size = mesh_hello_upgrade_key(key_size, hello_server.key_size);
    let wire_format = match hello_server.wire_format {
        Some(a) => a,
        None => {
            debug!("server did not send wire format");
            return Err(CommsError::NoWireFormat);
        }
    };
    debug!("client wire_format={}", wire_format);
    
    Ok((
        key_size,
        wire_format
    ))
}

pub(super) async fn mesh_hello_exchange_receiver(stream_rx: &mut StreamRx, stream_tx: &mut StreamTx, mut key_size: Option<KeySize>, wire_format: SerializationFormat) -> Result<Option<KeySize>, CommsError>
{
    // Read the hello message from the other side
    let hello_client_bytes = stream_rx.read_16bit().await?;
    debug!("server received hello from client");
    let hello_client: Hello = serde_json::from_slice(&hello_client_bytes[..])?;

    // Upgrade the key_size if the client is bigger
    key_size = mesh_hello_upgrade_key(key_size, hello_client.key_size);

    // Send over the hello message and wait for a response
    debug!("server sending hello (wire_format={})", wire_format);
    let hello_server = Hello {
        domain: None,
        key_size,
        wire_format: Some(wire_format),
    };
    let hello_server_bytes = serde_json::to_vec(&hello_server)?;
    stream_tx.write_16bit(hello_server_bytes).await?;

    Ok(key_size)
}

fn mesh_hello_upgrade_key(key1: Option<KeySize>, key2: Option<KeySize>) -> Option<KeySize>
{
    // If both don't want encryption then who are we to argue about that?
    if key1.is_none() && key2.is_none() {
        return None;
    }

    // Wanting encryption takes priority over not wanting encyption
    let key1 = match key1 {
        Some(a) => a,
        None => {
            debug!("upgrading to {}bit shared secret", key2.unwrap());
            return key2;
        }
    };
    let key2 = match key2 {
        Some(a) => a,
        None => {
            debug!("upgrading to {}bit shared secret", key1);
            return Some(key1);
        }
    };

    // Upgrade the key_size if the client is bigger
    if key2 > key1 {
        debug!("upgrading to {}bit shared secret", key2);
        return Some(key2);
    }
    if key1 > key2 {
        debug!("upgrading to {}bit shared secret", key2);
        return Some(key1);
    }

    // They are identical
    return Some(key1);
}