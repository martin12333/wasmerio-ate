#![allow(unused_imports)]
use ate::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

#[cfg(not(feature = "enable_server"))]
fn main() {}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), AteError> {
    ate::log_init(0, true);

    // Create the server and listen on port 5000
    let url = url::Url::parse("ws://localhost:5000/test-chain").unwrap();
    let cfg_ate = ConfAte::default();
    #[cfg(feature = "enable_dns")]
    let cfg_mesh =
        ConfMesh::solo_from_url(&cfg_ate, &url, &IpAddr::from_str("::").unwrap(), None, None)
            .await?;
    #[cfg(not(feature = "enable_dns"))]
    let cfg_mesh = ConfMesh::solo_from_url(&cfg_ate, &url)?;
    info!("create a persistent server");
    let server = create_persistent_centralized_server(&cfg_ate, &cfg_mesh).await?;

    info!("write some data to the server");

    let key = {
        let registry = Registry::new(&cfg_ate).await.cement();
        let chain = registry
            .open(
                &url::Url::from_str("ws://localhost:5000/").unwrap(),
                &ChainKey::from("test-chain"),
                false
            )
            .await?;
        let session = AteSessionUser::new();
        let dio = chain.dio_mut(&session).await;
        let dao = dio.store("my test string".to_string())?;
        dio.commit().await?;
        dao.key().clone()
    };

    info!("read it back again on a new client");

    {
        let registry = Registry::new(&cfg_ate).await.cement();
        let chain = registry
            .open(
                &url::Url::from_str("ws://localhost:5000/").unwrap(),
                &ChainKey::from("test-chain"),
                false
            )
            .await?;
        let session = AteSessionUser::new();
        let dio = chain.dio(&session).await;
        let dao = dio.load::<String>(&key).await?;

        assert_eq!(*dao, "my test string".to_string());
    }

    server.shutdown().await;

    Ok(())
}
