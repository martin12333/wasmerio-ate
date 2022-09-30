#![allow(unused_imports)]
use ate::prelude::*;
use error_chain::bail;
use std::io::stdout;
use std::io::Write;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};
use url::Url;

use crate::cmd::*;
use crate::error::*;
use crate::helper::*;
use crate::opt::*;
use crate::prelude::*;
use crate::request::*;

pub async fn group_user_remove_command(
    registry: &Registry,
    session: &AteSessionGroup,
    purpose: AteRolePurpose,
    username: String,
    auth: Url,
) -> Result<GroupUserRemoveResponse, GroupUserRemoveError> {
    // Open a command chain
    let group = session.identity().to_string();
    let chain = registry.open_cmd(&auth).await?;

    // First we query the user that needs to be removed so that we can get their public encrypt key
    let query = query_command(registry, username, auth).await?;

    // Determine what level of authentication we will associate the role with
    let who = match purpose {
        AteRolePurpose::Owner => query.advert.sudo_encrypt,
        _ => query.advert.nominal_encrypt,
    };

    // Make the create request and fire it over to the authentication server
    let create = GroupUserRemoveRequest {
        group,
        session: session.clone(),
        who: who.hash(),
        purpose,
    };

    let response: Result<GroupUserRemoveResponse, GroupUserRemoveFailed> =
        chain.invoke(create).await?;
    let result = response?;
    debug!("key: {}", result.key);
    Ok(result)
}

pub async fn main_group_user_remove(
    purpose: Option<AteRolePurpose>,
    username: Option<String>,
    auth: Url,
    session: &AteSessionGroup,
    hint_group: &str,
) -> Result<(), GroupUserRemoveError> {
    let purpose = match purpose {
        Some(a) => a,
        None => {
            print!("Role: ");
            stdout().lock().flush()?;
            let mut s = String::new();
            std::io::stdin()
                .read_line(&mut s)
                .expect("Did not enter a valid role purpose");
            match AteRolePurpose::from_str(s.trim()) {
                Ok(a) => a,
                Err(_err) => {
                    bail!(GroupUserRemoveErrorKind::InvalidPurpose);
                }
            }
        }
    };

    let username = match username {
        Some(a) => a,
        None => {
            print!("Username: ");
            stdout().lock().flush()?;
            let mut s = String::new();
            std::io::stdin()
                .read_line(&mut s)
                .expect("Did not enter a valid username");
            s.trim().to_string()
        }
    };

    // Remove a user from a group using the authentication server
    let registry = ate::mesh::Registry::new(&conf_cmd()).await.cement();
    let result = group_user_remove_command(&registry, &session, purpose, username, auth).await?;

    println!("{} user removed (id={})", hint_group, result.key);

    Ok(())
}
