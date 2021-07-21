#[allow(unused_imports)]
use log::{info, warn, debug};

use crate::error::*;
use serde::{Serialize, Deserialize};
use crate::crypto::KeySize;
use crate::spec::*;

use super::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HelloMetadata
{
    pub path: String,
    pub encryption: Option<KeySize>,
    pub wire_format: SerializationFormat,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SenderHello
{
    pub path: String,
    pub domain: Option<String>,
    pub key_size: Option<KeySize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ReceiverHello
{
    pub encryption: Option<KeySize>,
    pub wire_format: SerializationFormat,
}

pub(super) async fn mesh_hello_exchange_sender(stream_rx: &mut StreamRx, stream_tx: &mut StreamTx, hello_path: String, domain: Option<String>, key_size: Option<KeySize>) -> Result<HelloMetadata, CommsError>
{
    // Send over the hello message and wait for a response
    debug!("client sending hello");
    let hello_client = SenderHello {
        path: hello_path.clone(),
        domain,
        key_size,
    };
    let hello_client_bytes = serde_json::to_vec(&hello_client)?;
    stream_tx.write_16bit(hello_client_bytes, false).await?;

    // Read the hello message from the other side
    let hello_server_bytes = stream_rx.read_16bit().await?;
    debug!("client received hello from server");
    let hello_server: ReceiverHello = serde_json::from_slice(&hello_server_bytes[..])?;

    // Upgrade the key_size if the server is bigger
    debug!("client encryption={}", match &hello_server.encryption {
        Some(a) => a.as_str(),
        None => "none"
    });
    debug!("client wire_format={}", hello_server.wire_format);
    
    Ok(HelloMetadata {
        path: hello_path,
        encryption: hello_server.encryption,
        wire_format: hello_server.wire_format,
    })
}

pub(super) async fn mesh_hello_exchange_receiver(stream_rx: &mut StreamRx, stream_tx: &mut StreamTx, key_size: Option<KeySize>, wire_format: SerializationFormat) -> Result<HelloMetadata, CommsError>
{
    // Read the hello message from the other side
    let hello_client_bytes = stream_rx.read_16bit().await?;
    debug!("server received hello from client");
    let hello_client: SenderHello = serde_json::from_slice(&hello_client_bytes[..])?;
    
    // Upgrade the key_size if the client is bigger
    let encryption = mesh_hello_upgrade_key(key_size, hello_client.key_size);
    
    // Send over the hello message and wait for a response
    debug!("server sending hello (wire_format={})", wire_format);
    let hello_server = ReceiverHello {
        encryption,
        wire_format,
    };
    let hello_server_bytes = serde_json::to_vec(&hello_server)?;
    stream_tx.write_16bit(hello_server_bytes, false).await?;

    Ok(HelloMetadata {
        path: hello_client.path,
        encryption,
        wire_format,
    })
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