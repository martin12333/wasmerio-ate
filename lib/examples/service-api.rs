use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[allow(unused_imports)]
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use ate::prelude::*;

#[derive(Clone, Serialize, Deserialize)]
struct Ping {
    msg: String,
}

#[derive(Serialize, Deserialize)]
struct Pong {
    msg: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct PingError {}

#[derive(Default)]
struct PingPongTable {}

impl PingPongTable {
    async fn process(self: Arc<Self>, ping: Ping) -> Result<Pong, PingError> {
        Ok(Pong { msg: ping.msg })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), AteError> {
    ate::log_init(0, true);

    info!("creating test chain");

    // Create the chain with a public/private key to protect its integrity
    let conf = ConfAte::default();
    let builder = ChainBuilder::new(&conf).await.build();
    let chain = builder.open(&ChainKey::from("cmd")).await?;

    info!("start the service on the chain");
    let session = AteSessionUser::new();
    chain.add_service(
        &session,
        Arc::new(PingPongTable::default()),
        PingPongTable::process,
    );

    info!("sending ping");
    let pong: Result<Pong, PingError> = chain
        .invoke(Ping {
            msg: "hi".to_string(),
        })
        .await?;
    let pong = pong.unwrap();

    info!("received pong with msg [{}]", pong.msg);
    Ok(())
}
