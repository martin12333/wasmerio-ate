use ate::mesh::FatalTerminate;
use ate::prelude::*;
use ate::utils::LoadProgress;
use error_chain::bail;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use crate::fs::AteFS;
use crate::opts::*;
use crate::umount;

use fuse3::raw::prelude::*;
use fuse3::MountOptions;

fn ctrl_channel() -> tokio::sync::watch::Receiver<bool> {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    ctrlc_async::set_handler(move || {
        let _ = sender.send(true);
    })
    .unwrap();
    receiver
}

pub async fn main_mount(
    mount: OptsMount,
    conf: ConfAte,
    group: Option<String>,
    session: AteSessionType,
    no_auth: bool,
) -> Result<(), AteError> {
    let uid = match mount.uid {
        Some(a) => a,
        None => unsafe { libc::getuid() },
    };
    let gid = match mount.gid {
        Some(a) => a,
        None => unsafe { libc::getgid() },
    };

    debug!("uid: {}", uid);
    debug!("gid: {}", uid);

    let mount_options = MountOptions::default()
        .uid(uid)
        .gid(gid)
        .allow_root(mount.allow_root)
        .allow_other(mount.allow_other)
        .read_only(mount.read_only)
        .write_back(mount.write_back)
        .nonempty(mount.non_empty);

    debug!("allow_root: {}", mount.allow_root);
    debug!("allow_other: {}", mount.allow_other);
    debug!("read_only: {}", mount.read_only);
    debug!("write_back: {}", mount.write_back);
    debug!("non_empty: {}", mount.non_empty);

    let mut conf = conf.clone();
    conf.configured_for(mount.configured_for);
    conf.log_format.meta = mount.meta_format;
    conf.log_format.data = mount.data_format;
    conf.log_path = mount
        .log_path
        .as_ref()
        .map(|a| shellexpand::tilde(a).to_string());
    conf.backup_path = mount
        .backup_path
        .as_ref()
        .map(|a| shellexpand::tilde(a).to_string());
    conf.recovery_mode = mount.recovery_mode;
    conf.compact_bootstrap = mount.compact_now;
    conf.compact_mode = mount
        .compact_mode
        .with_growth_factor(mount.compact_threshold_factor)
        .with_growth_size(mount.compact_threshold_size)
        .with_timer_value(Duration::from_secs(mount.compact_timer));

    info!("configured_for: {:?}", mount.configured_for);
    info!("meta_format: {:?}", mount.meta_format);
    info!("data_format: {:?}", mount.data_format);
    info!(
        "log_path: {}",
        match conf.log_path.as_ref() {
            Some(a) => a.as_str(),
            None => "(memory)",
        }
    );
    info!("log_temp: {}", mount.temp);
    info!("mount_path: {}", mount.mount_path);
    match &mount.remote_name {
        Some(remote) => info!("remote: {}", remote.to_string()),
        None => info!("remote: local-only"),
    };

    let builder = ChainBuilder::new(&conf).await.temporal(mount.temp);

    // Create a progress bar loader
    let mut progress_local = LoadProgress::new(std::io::stdout());
    let mut progress_remote = LoadProgress::new(std::io::stdout());
    progress_local.units = pbr::Units::Bytes;
    progress_local.msg_done = "Downloading latest events from server...".to_string();
    progress_remote.msg_done =
        "Loaded the remote chain-of-trust, proceeding to mount the file system.".to_string();
    print!("Loading the chain-of-trust...");

    // We create a chain with a specific key (this is used for the file name it creates)
    debug!("chain-init");
    let registry;
    let chain = match mount.remote_name {
        None => {
            let trust = match &mount.configured_for {
                ConfiguredFor::BestSecurity | ConfiguredFor::SmallestSize => {
                    TrustMode::Centralized(CentralizedRole::Client)
                }
                _ => TrustMode::Distributed,
            };
            Ok(Arc::new(
                Chain::new_ext(
                    builder.clone(),
                    ChainKey::from("root"),
                    Some(Box::new(progress_local)),
                    true,
                    trust,
                    trust,
                )
                .await?,
            ))
        }
        Some(remote) => {
            registry = ate::mesh::Registry::new(&conf).await.temporal(mount.temp);

            let guard = registry
                .open_ext(
                    &mount.remote,
                    &ChainKey::from(remote),
                    progress_local,
                    progress_remote,
                )
                .await?;
            Ok(guard.as_arc())
        }
    };

    // Perform specific error handling (otherwise let it propogate up)
    let chain = match chain {
        Ok(a) => a,
        Err(ChainCreationError(ChainCreationErrorKind::ServerRejected(reason), _)) => {
            match reason {
                FatalTerminate::Denied { reason } => {
                    println!("Access to this file system was denied by the server");
                    println!("---");
                    println!("{}", reason);
                    std::process::exit(1);
                }
                _ => {
                    bail!(AteErrorKind::ChainCreationError(
                        ChainCreationErrorKind::ServerRejected(reason)
                    ));
                }
            }
        }
        Err(err) => {
            bail!(err);
        }
    };

    // Compute the scope
    let scope_meta = match mount.recovery_mode.is_meta_sync() {
        true => TransactionScope::Full,
        false => TransactionScope::Local,
    };
    let scope_io = match mount.recovery_mode.is_sync() {
        true => TransactionScope::Full,
        false => TransactionScope::Local,
    };

    // Create the mount point
    let mount_path = mount.mount_path.clone();
    let mount_join = Session::new(mount_options).mount_with_unprivileged(
        AteFS::new(
            chain,
            group,
            session,
            scope_io,
            scope_meta,
            no_auth,
            mount.impersonate_uid,
            mount.umask,
        )
        .await,
        mount.mount_path,
    );

    // Install a ctrl-c command
    info!("mounting file-system and entering main loop");
    let mut ctrl_c = ctrl_channel();

    // Add a panic hook that will unmount
    {
        let orig_hook = std::panic::take_hook();
        let mount_path = mount_path.clone();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = umount::unmount(std::path::Path::new(mount_path.as_str()));
            orig_hook(panic_info);
            std::process::exit(1);
        }));
    }

    // Main loop
    println!("Press ctrl-c to exit");
    select! {
        // Wait for a ctrl-c
        _ = ctrl_c.changed() => {
            umount::unmount(std::path::Path::new(mount_path.as_str()))?;
            println!("Goodbye!");
            return Ok(());
        }

        // Mount the file system
        ret = mount_join => {
            match ret {
                Err(err) if err.kind() == ErrorKind::Other => {
                    if err.to_string().contains("find fusermount binary failed") {
                        error!("Fuse3 could not be found - you may need to install fuse3 via apt/yum");
                        return Ok(())
                    }
                    error!("{}", err);
                    println!("Mount failed");
                    let _ = umount::unmount(std::path::Path::new(mount_path.as_str()));
                    std::process::exit(1);
                }
                Err(err) => {
                    error!("{}", err);
                    println!("Mount failed");
                    let _ = umount::unmount(std::path::Path::new(mount_path.as_str()));
                    std::process::exit(1);
                }
                _ => {
                    println!("Mount shutdown");
                    let _ = umount::unmount(std::path::Path::new(mount_path.as_str()));
                    return Ok(());
                }
            }
        }
    }
}
