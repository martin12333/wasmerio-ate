#![allow(unused_imports)]
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
#[cfg(feature = "enable_full")]
use tokio::net::TcpStream as TokioTcpStream;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use crate::conf::ConfAte;
use crate::engine::TaskEngine;

use {
    trust_dns_client::client::*, trust_dns_client::op::DnsResponse, trust_dns_client::tcp::*,
    trust_dns_proto::iocompat::AsyncIoTokioAsStd, trust_dns_proto::DnssecDnsHandle,
};

pub use {trust_dns_client::error::ClientError, trust_dns_client::rr::*};

pub enum DnsClient {
    Dns {
        cfg: ConfAte,
        client: MemoizeClientHandle<AsyncClient>,
    },
    DnsSec {
        cfg: ConfAte,
        client: DnssecDnsHandle<MemoizeClientHandle<AsyncClient>>,
    },
}

impl DnsClient {
    #[cfg(feature = "enable_full")]
    pub async fn connect(cfg: &ConfAte) -> DnsClient {
        debug!("using DNS server: {}", cfg.dns_server);
        let addr: SocketAddr = (cfg.dns_server.clone(), 53)
            .to_socket_addrs()
            .unwrap()
            .next()
            .unwrap();

        let (stream, sender) = TcpClientStream::<AsyncIoTokioAsStd<TokioTcpStream>>::new(addr);
        let client = AsyncClient::new(stream, sender, None);
        let (client, bg) = client.await.expect("client failed to connect");
        TaskEngine::spawn(bg);

        let client = MemoizeClientHandle::new(client);

        match cfg.dns_sec {
            false => {
                debug!("configured for DNSSec");
                DnsClient::Dns {
                    cfg: cfg.clone(),
                    client,
                }
            }
            true => {
                debug!("configured for plain DNS");
                DnsClient::DnsSec {
                    cfg: cfg.clone(),
                    client: DnssecDnsHandle::new(client.clone()),
                }
            }
        }
    }

    pub async fn reconnect(&mut self) {
        let cfg = match self {
            DnsClient::Dns { cfg, client: _ } => cfg.clone(),
            DnsClient::DnsSec { cfg, client: _ } => cfg.clone(),
        };

        *self = DnsClient::connect(&cfg).await;
    }

    pub async fn query(
        &mut self,
        name: Name,
        query_class: DNSClass,
        query_type: RecordType,
    ) -> Result<DnsResponse, ClientError> {
        let ret = {
            match self {
                DnsClient::Dns { cfg: _, client: c } => {
                    c.query(name.clone(), query_class, query_type).await
                }
                DnsClient::DnsSec { cfg: _, client: c } => {
                    c.query(name.clone(), query_class, query_type).await
                }
            }
        };

        match ret {
            Ok(a) => Ok(a),
            Err(_) => {
                self.reconnect().await;

                match self {
                    DnsClient::Dns { cfg: _, client: c } => {
                        c.query(name, query_class, query_type).await
                    }
                    DnsClient::DnsSec { cfg: _, client: c } => {
                        c.query(name, query_class, query_type).await
                    }
                }
            }
        }
    }
}
