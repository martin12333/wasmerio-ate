#[allow(unused_imports)]
use log::{info, warn, debug, error};
use std::sync::Arc;
use regex::Regex;

use async_trait::async_trait;
use ate::{error::ChainCreationError, prelude::*};

pub struct ChainFlow {
    cfg: ConfAte,
    regex: Regex,
    mode: TrustMode,
    auth: Option<url::Url>,
    registry: Arc<Registry>,
}

impl ChainFlow
{
    pub async fn new(cfg: &ConfAte, auth: Option<url::Url>, mode: TrustMode) -> Self {        
        ChainFlow {
            cfg: cfg.clone(),
            regex: Regex::new("^/([a-z0-9\\.!#$%&'*+/=?^_`{|}~-]{1,})/([a-z0-9\\.!#$%&'*+/=?^_`{|}~-]{1,})/([a-zA-Z0-9_]{1,})$").unwrap(),
            mode,
            auth,
            registry: ate::mesh::Registry::new(cfg, true).await
        }
    }
}

#[async_trait]
impl OpenFlow
for ChainFlow
{
    async fn open(&self, mut builder: ChainBuilder, key: &ChainKey) -> Result<OpenAction, ChainCreationError>
    {
        // Extract the identity from the supplied path (we can only create chains that are actually
        // owned by the specific user)
        let path = key.name.clone();
        if let Some(captures) = self.regex.captures(path.as_str())
        {
            // Build the email address using the captures
            let email = format!("{}@{}", captures.get(2).unwrap().as_str(), captures.get(1).unwrap().as_str());
            let dbname = captures.get(3).unwrap().as_str().to_string();

            // Check for very naughty parameters
            if email.contains("..") || dbname.contains("..") || email.contains("~") || dbname.contains("~") {
                return Ok(OpenAction::Deny(format!("The chain-key ({}) contains forbidden characters.", key.to_string()).to_string()));
            }

            // Grab the public write key from the authentication server for this user
            if let Some(auth) = &self.auth {
                let registry = ate::mesh::Registry::new(&ate_auth::conf_auth(), true).await;
                let advert = match ate_auth::query_command(Arc::clone(&registry), email.clone(), auth.clone()).await {
                    Ok(a) => a.advert,
                    Err(err) => {
                        return Ok(OpenAction::Deny(format!("Failed to create the chain as the query to the authentication server failed - {}.", err.to_string()).to_string()));
                    }
                };
                let root_key = advert.auth;
                builder = builder.add_root_public_key(&root_key);
            }
            
            let chain = builder
                .build()
                .open(key)
                .await?;

            // We have opened the chain
            return match self.mode {
                TrustMode::Centralized => Ok(OpenAction::CentralizedChain(chain)),
                TrustMode::Distributed => Ok(OpenAction::DistributedChain(chain)),
            };
        }

        // Ask the authentication server for the public key for this user
        return Ok(OpenAction::Deny(format!("The chain-key ({}) does not match a valid pattern - it must be in the format of /gmail.com/joe.blogs/mydb where the owner of this chain is the user joe.blogs@gmail.com.", key.to_string()).to_string()));
    }
}