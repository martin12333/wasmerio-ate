#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};
#[cfg(unix)]
#[allow(unused_imports)]
use std::os::unix::fs::symlink;
#[allow(unused_imports)]
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::ops::Deref;
use error_chain::bail;
use async_stream::stream;
use futures_util::pin_mut;
use futures_util::stream::StreamExt;
#[cfg(feature = "enable_bridge")]
#[cfg(any(target_os = "linux", target_os = "macos"))]
use {
    std::process::Command,
    std::process::Stdio,
    std::net::Ipv4Addr,
    tokio::io::AsyncReadExt,
    tokio::io::AsyncWriteExt,
    tokio_tun::TunBuilder,
};
use ate::prelude::*;
#[allow(unused_imports)]
use wasmer_bus_mio::prelude::*;

use crate::error::*;
use crate::model::NetworkToken;
#[allow(unused_imports)]
use crate::model::HardwareAddress;
use crate::opt::*;
use crate::api::DeployApi;

use super::*;

pub async fn main_opts_network(
    opts_network: OptsNetwork,
    token_path: String,
    auth_url: url::Url,
) -> Result<(), InstanceError>
{
    #[allow(unused_variables)]
    let db_url = wasmer_auth::prelude::origin_url(&opts_network.db_url, "db");
    match opts_network.cmd
    {
        OptsNetworkCommand::For(opts) => {
            let purpose: &dyn OptsPurpose<OptsNetworkAction> = &opts.purpose;
            let mut context = PurposeContext::new(purpose, token_path.as_str(), &auth_url, Some(&db_url), true).await?;
            match context.action.clone() {
                OptsNetworkAction::List => {
                    main_opts_network_list(&mut context.api).await
                },
                OptsNetworkAction::Details(opts) => {
                    main_opts_network_details(&mut context.api, opts.name.as_str()).await
                },
                OptsNetworkAction::Cidr(opts) => {
                    main_opts_network_cidr(&mut context.api, opts.name.as_str(), opts.action).await
                },
                OptsNetworkAction::Peering(opts) => {
                    main_opts_network_peering(&mut context.api, opts.name.as_str(), opts.action).await
                },
                OptsNetworkAction::Reset(opts) => {
                    main_opts_network_reset(&mut context.api, opts.name.as_str()).await
                },
                OptsNetworkAction::Connect(opts) => {
                    main_opts_network_connect(&mut context.api, opts.name.as_str(), token_path, opts.export).await
                },
                OptsNetworkAction::Create(opts) => {
                    let mut instance_authority = db_url.domain()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "wasmer.sh".to_string());
                    if instance_authority == "localhost" {
                        instance_authority = "wasmer.sh".to_string();
                    }
                    main_opts_network_create(&mut context.api, opts.name, purpose.group_name(), db_url, instance_authority, opts.force).await
                },
                OptsNetworkAction::Kill(opts) => {
                    main_opts_network_kill(&mut context.api, opts.name.as_str(), opts.force).await
                },
            }
        },
        OptsNetworkCommand::Reconnect(opts) => {
            main_opts_network_reconnect(opts.token, token_path).await
        },
        OptsNetworkCommand::Disconnect => {
            main_opts_network_disconnect(token_path).await;
            Ok(())
        },
        OptsNetworkCommand::Monitor(opts) => {
            let net_url = wasmer_auth::prelude::origin_url(&opts.net_url, "net");
            main_opts_network_monitor(token_path, net_url, opts_network.security).await
        },
        #[cfg(feature = "enable_bridge")]
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        OptsNetworkCommand::Bridge(opts) => {
            let net_url = wasmer_auth::prelude::origin_url(&opts.net_url, "net");
            main_opts_network_bridge(opts, token_path, net_url, opts_network.security).await
        }
    }
}

pub async fn main_opts_network_list(
    api: &mut DeployApi,
) -> Result<(), InstanceError>
{
    println!("|-------name-------|-peerings");
    let instances = api.instances().await;

    let instances = instances.iter_ext(true, true).await?;
    let instances_ext = {
        let api = api.clone();
        stream! {
            for instance in instances {
                let name = instance.name.clone();
                yield
                (
                    api.instance_chain(instance.name.as_str())
                        .await
                        .map(|chain| (instance, chain)),
                    name,
                )
            }
        }
    };
    pin_mut!(instances_ext);

    while let Some((res, name)) = instances_ext.next().await {
        let (wallet_instance, _) = match res {
            Ok(a) => a,
            Err(err) => {
                debug!("error loading wallet instance - {} - {}", name, err);
                println!(
                    "- {:<16} - {:<19} - {}",
                    name, "error", err
                );
                continue;
            }
        };
        let mut peerings = String::new();
        if let Ok(service_instance) = api.instance_load(wallet_instance.deref()).await {
            for peer in service_instance.subnet.peerings.iter() {
                if peerings.len() > 0 { peerings.push_str(","); }
                peerings.push_str(peer.name.as_str());
            }
        }
        println!(
            "- {:<16} - {}",
            wallet_instance.name,
            peerings
        );
    }
    Ok(())
}

pub async fn main_opts_network_details(
    api: &mut DeployApi,
    network_name: &str,
) -> Result<(), InstanceError>
{
    let network = api.instance_find(network_name)
        .await;
    let network = match network {
        Ok(a) => a,
        Err(InstanceError(InstanceErrorKind::InvalidInstance, _)) => {
            eprintln!("An network does not exist for this token.");
            std::process::exit(1);
        }
        Err(err) => {
            bail!(err);
        }
    };

    println!("Network");
    println!("{}", serde_json::to_string_pretty(network.deref()).unwrap());

    if let Ok(service_instance) = api.instance_load(network.deref()).await {
        println!("{}", serde_json::to_string_pretty(&service_instance.subnet).unwrap());

        for node in service_instance.mesh_nodes.iter().await? {
            println!("");
            println!("Mesh Node");
            println!("Key: {}", node.key());
            println!("Address: {}", node.node_addr);
            
            if node.switch_ports.len() > 0 {
                println!("Switch Ports:");
                for switch_port in node.switch_ports.iter() {
                    println!("- {}", switch_port);
                }
            }
            if node.dhcp_reservation.len() > 0 {
                println!("DHCP");
                for (mac, ip) in node.dhcp_reservation.iter() {
                    println!("- {} - {},", mac, ip.addr4);
                }
            }
        }
    }

    Ok(())
}

pub async fn main_opts_network_cidr(
    api: &mut DeployApi,
    network_name: &str,
    action: OptsCidrAction,
) -> Result<(), InstanceError> {
    let (instance, _) = api.instance_action(network_name).await?;
    let instance = instance?;
    
    main_opts_cidr(instance, action).await?;

    Ok(())
}

pub async fn main_opts_network_peering(
    api: &mut DeployApi,
    network_name: &str,
    action: OptsPeeringAction,
) -> Result<(), InstanceError> {
    let (instance, wallet_instance) = api.instance_action(network_name).await?;
    let instance = instance?;
    
    main_opts_peering(api, instance, wallet_instance, action).await?;

    Ok(())
}

pub async fn main_opts_network_reset(
    api: &mut DeployApi,
    network_name: &str,
) -> Result<(), InstanceError> {
    main_opts_instance_reset(api, network_name).await
}

pub async fn main_opts_network_connect(
    api: &mut DeployApi,
    network_name: &str,
    token_path: String,
    export: bool,
) -> Result<(), InstanceError>
{
    // Get the specifics around the network we will be connecting too
    let (instance, _) = api.instance_action(network_name).await?;
    let instance = instance?;
    let chain = instance.chain.clone();
    let access_token = instance.subnet.network_token.clone();

    // Build the access token
    let token = NetworkToken {
        chain: ChainKey::from(chain.clone()),
        access_token: access_token.clone(),
    };

    // If we are exporting then just throw it out to STDOUT
    if export {
        println!("{}", token);
        return Ok(());
    }

    // Save the token
    save_access_token(token_path, &token).await?;
    Ok(())            
}

pub async fn main_opts_network_create(
    api: &mut DeployApi,
    network_name: Option<String>,
    group: Option<String>,
    db_url: url::Url,
    instance_authority: String,
    force: bool,
) -> Result<(), InstanceError> {
    main_opts_instance_create(api, network_name, group, db_url, instance_authority, force).await
}

pub async fn main_opts_network_kill(
    api: &mut DeployApi,
    network_name: &str,
    force: bool,
) -> Result<(), InstanceError> {
    main_opts_instance_kill(api, network_name, force).await
}

pub async fn main_opts_network_reconnect(
    token: String,
    token_path: String,
) -> Result<(), InstanceError>
{
    // Decode the token
    let token = FromStr::from_str(token.as_str())?;

    // Save the token
    save_access_token(token_path, &token).await?;
    Ok(())            
}

pub async fn main_opts_network_disconnect(token_path: String)
{
    clear_access_token(token_path).await;
}

pub async fn main_opts_network_monitor(
    token_path: String,
    net_url: url::Url,
    security: StreamSecurity,
) -> Result<(), InstanceError>
{
    let token_path = if let Ok(t) = std::env::var("NETWORK_TOKEN_PATH") {
        t
    } else {
        shellexpand::tilde(token_path.as_str()).to_string()
    };

    let port = Port::new(TokenSource::ByPath(token_path), net_url, security)?;
    let socket = port.bind_raw()
        .await
        .map_err(|err| {
            let err = format!("failed to open raw socket - {}", err);
            error!("{}", err);
            InstanceErrorKind::InternalError(0)
        })?;
    socket.set_promiscuous(true)
        .await
        .map_err(|err| {
            let err = format!("failed to set promiscuous - {}", err);
            error!("{}", err);
            InstanceErrorKind::InternalError(0)
        })?;
    
    println!("Monitoring {}", port.chain());
    while let Ok(data) = socket.recv().await {
        tcpdump(&data[..]);
    }
    Ok(())
}

#[cfg(not(feature = "smoltcp"))]
fn tcpdump(data: &[u8]) {
    if data.len() > 18 {
        let end = data.len() - 18;
        let dst = hex::encode(&data[0..6]).to_uppercase();
        let src = hex::encode(&data[6..12]).to_uppercase();
        let ty = hex::encode(&data[12..14]).to_uppercase();
        let data = hex::encode(&data[14..end]).to_uppercase();
        println!("{}->{} ({}): {}", src, dst, ty, data);
    } else {
        println!("JUNK 0x{}", hex::encode(data).to_uppercase());
    }    
}

#[cfg(feature = "smoltcp")]
fn tcpdump(data: &[u8]) {
    let pck = smoltcp::wire::PrettyPrinter::<smoltcp::wire::EthernetFrame<&[u8]>>::new("", &data);
    println!("{}", pck);
}

#[cfg(feature = "enable_bridge")]
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub async fn main_opts_network_bridge(
    bridge: OptsNetworkBridge,
    token_path: String,
    net_url: url::Url,
    security: StreamSecurity,
) -> Result<(), InstanceError>
{
    let token_path = if let Ok(t) = std::env::var("NETWORK_TOKEN_PATH") {
        t
    } else {
        shellexpand::tilde(token_path.as_str()).to_string()
    };

    std::env::set_var("NETWORK_TOKEN_PATH", token_path.as_str());
    ::sudo::with_env(&(vec!("NETWORK_TOKEN_PATH")[..])).unwrap();

    if bridge.daemon {
        if let Ok(fork::Fork::Parent(_)) = fork::daemon(true, true) {
            return Ok(())
        }
    }

    let port = Port::new(TokenSource::ByPath(token_path), net_url, security)?;
    let hw = port.hardware_address()
        .await?
        .ok_or_else(|| {
            error!("the hardware address (MAC) on the port has not been set");
            InstanceErrorKind::InternalError(0)
        })?;
    let hw: [u8; 6] = hw.into();
    
    let (ip4, netmask4) = port.dhcp_acquire().await?;

    print!("connected:");
    if let Ok(Some(mac)) = port.hardware_address().await {
        print!(" mac={}", mac);
    }
    if let Ok(Some(ip)) = port.addr_ipv4().await {
        print!(" ip={}", ip);
    }
    println!("");

    let mtu = bridge.mtu.unwrap_or(1500);

    let socket = port.bind_raw()
        .await
        .map_err(|err| {
            let err = format!("failed to open raw socket - {}", err);
            error!("{}", err);
            InstanceErrorKind::InternalError(0)
        })?;

    if bridge.promiscuous {
        socket.set_promiscuous(true)
            .await
            .map_err(|err| {
                let err = format!("failed to set promiscuous - {}", err);
                error!("{}", err);
                InstanceErrorKind::InternalError(0)
            })?;
    }

    let name_id = fastrand::u64(..);
    let name = format!("ate{}", hex::encode(name_id.to_ne_bytes()).to_uppercase());
    let name = &name[..15];

    let tap = TunBuilder::new()
        .name(name)
        .tap(true)
        .packet_info(false)
        .mtu(mtu as i32)
        .mac(hw.clone())
        //.up()
        .address(ip4)
        .netmask(netmask4)
        .broadcast(Ipv4Addr::BROADCAST)
        .try_build()
        .map_err(|err| {
            let err = format!("failed to build tun/tap device - {}", err);
            error!("{}", err);
            InstanceErrorKind::InternalError(0)
        })?;

    let (mut reader, mut writer) = tokio::io::split(tap);

    std::thread::sleep(std::time::Duration::from_millis(200));
    cmd("ip", &["link", "set", "dev", name, "down"]);
    if bridge.promiscuous {
        cmd("ip", &["link", "set", "dev", name, "promisc", "on"]);    
    }
    let hw = hex::encode(hw.as_slice());
    let hw = format!("{}:{}:{}:{}:{}:{}", &hw[0..2], &hw[2..4], &hw[4..6], &hw[6..8], &hw[8..10], &hw[10..12]);
    let _ = cmd("ip", &["link", "set", "dev", name, "address", hw.as_str()]);
    let _ = cmd("ip", &["link", "set", "dev", name, "up"]);

    loop {
        let mut buf = [0u8; 2048];
        tokio::select! {
            n = reader.read(&mut buf) => {
                match n {
                    Ok(n) => {
                        let buf = (&buf[..n]).to_vec();
                        socket.send(buf).await?;
                    }
                    Err(err) => {
                        error!("TAP device closed - {}", err);
                        break;
                    }
                }
            },
            data = socket.recv() => {
                let data = data?;
                writer.write(&data[..]).await?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "enable_bridge")]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn cmd_inner(cmd: &str, args: &[&str], stderr: Stdio, stdout: Stdio) -> Result<std::process::ExitStatus, std::io::Error> {
    Command::new(cmd)
        .args(args)
        .stderr(stderr)
        .stdout(stdout)
        .spawn()
        .unwrap()
        .wait()
}

#[cfg(feature = "enable_bridge")]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn cmd(cmd: &str, args: &[&str]) {
    let ecode = cmd_inner(cmd, args, Stdio::inherit(), Stdio::inherit()).unwrap();
    assert!(ecode.success(), "Failed to execte {}", cmd);
    std::thread::sleep(std::time::Duration::from_millis(10));
}

pub use wasmer_bus_mio::prelude::TokenSource;
pub use wasmer_bus_mio::prelude::Port;
pub use wasmer_bus_mio::prelude::load_access_token;
pub use wasmer_bus_mio::prelude::save_access_token;
pub use wasmer_bus_mio::prelude::clear_access_token;
