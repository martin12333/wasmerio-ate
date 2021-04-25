#[allow(unused_imports)]
use log::{info, error, debug};
use url::Url;

use ate::prelude::*;
use ate::crypto::EncryptKey;
use ate::error::*;

use crate::model::*;

pub fn auth_url(auth: Url, email: &String) -> Url
{
    let hash = AteHash::from(email.clone());
    let hex = hash.to_hex_string().to_lowercase();
    let mut ret = auth.clone();
    ret.set_path(format!("{}-{}", ret.path(), &hex[..4]).as_str());
    ret
}

pub fn auth_chain_key(path: String, email: &String) -> ChainKey
{
    let hash = AteHash::from(email.clone());
    let hex = hash.to_hex_string().to_lowercase();
    ChainKey::new(format!("{}-{}", path, &hex[..4]))
}

pub fn command_url(auth: Url) -> Url
{
    let hex = PrimaryKey::generate().as_fixed_hex_string();
    let mut ret = auth.clone();
    ret.set_path(format!("cmd-{}", hex).as_str());
    ret
}

pub fn password_to_read_key(seed: &String, password: &String, repeat: i32, key_size: KeySize) -> EncryptKey
{
    let mut bytes = Vec::from(seed.as_bytes());
    bytes.extend(Vec::from(password.as_bytes()).iter());
    while bytes.len() < 1000 {
        bytes.push(0);
    }
    let hash = AteHash::from_bytes_sha3(password.as_bytes(), repeat);
    EncryptKey::from_seed_bytes(hash.to_bytes(), key_size)
}

pub fn estimate_user_name_as_uid(email: String) -> u32
{
    let min = ((u32::MAX as u64) * 2) / 4;
    let max = ((u32::MAX as u64) * 3) / 4;
    PrimaryKey::from_ext(AteHash::from(email), min as u64, max as u64).as_u64() as u32
}

pub fn estimate_group_name_as_gid(group: String) -> u32
{
    let min = ((u32::MAX as u64) * 3) / 4;
    let max = ((u32::MAX as u64) * 4) / 4;
    PrimaryKey::from_ext(AteHash::from(group), min as u64, max as u64).as_u64() as u32
}

pub fn conf_auth() -> ConfAte
{
    let mut cfg_ate = ConfAte::default();
    cfg_ate.configured_for(ConfiguredFor::BestSecurity);
    cfg_ate.log_format.meta = SerializationFormat::Json;
    cfg_ate.log_format.data = SerializationFormat::Json;
    cfg_ate.wire_format = SerializationFormat::Json;
    cfg_ate
}

pub(crate) fn compute_user_auth(user: &User) -> AteSession
{
    let mut session = AteSession::default();
    for auth in user.access.iter() {
        session.user.add_read_key(&auth.read);
        session.user.add_private_read_key(&auth.private_read);
        session.user.add_write_key(&auth.write);
    }
    session.user.add_identity(user.email.clone());
    session.user.add_uid(user.uid);

    session
}

pub(crate) fn compute_sudo_auth(sudo: &Sudo, session: AteSession) -> AteSession
{
    let mut session = session.clone();

    let mut role = AteGroupRole {
        purpose: AteRolePurpose::Owner,
        properties: Vec::new()
    };
    for auth in sudo.access.iter() {
        role.add_read_key(&auth.read);
        role.add_private_read_key(&auth.private_read);
        role.add_write_key(&auth.write);
    }
    role.add_identity(sudo.email.clone());
    role.add_uid(sudo.uid);
    session.sudo.replace(role);

    session
}

pub(crate) fn complete_group_auth(group: &Group, mut session: AteSession)
    -> Result<AteSession, LoadError>
{    
    // Enter a recursive loop that will expand its authorizations of the roles until
    // it expands no more or all the roles are gained.
    let mut roles = group.roles.iter().collect::<Vec<_>>();
    while roles.len() > 0 {
        let start = roles.len();
        let mut next = Vec::new();

        // Process all the roles
        let super_keys = session.private_read_keys().map(|a| a.clone()).collect::<Vec<_>>();
        for role in roles.into_iter()
        {
            // Attempt to gain access to the role using the access rights of the super session
            let mut added = false;
            for read_key in super_keys.iter() {
                if let Some(a) = role.access.unwrap(&read_key)?
                {
                    // Add access rights to the session                    
                    let b = session.get_or_create_group_role(&group.name, &role.purpose);
                    b.add_read_key(&a.read);
                    b.add_private_read_key(&a.private_read);
                    b.add_write_key(&a.write);
                    b.add_identity(group.name.clone());
                    b.add_gid(group.gid);
                    added = true;
                    break;
                }
            }

            // If we have no successfully gained access to the role then add
            // it to the try again list.
            if added == false {
                next.push(role);
            }
        }

        // If we made no more progress (no more access was granted) then its
        // time to give up
        if next.len() >= start {
            break;
        }
        roles = next;
    }

    Ok(session)
}

pub fn session_to_b64(session: AteSession) -> Result<String, SerializationError>
{
    let format = SerializationFormat::MessagePack;
    let bytes = format.serialize(&session)?;
    Ok(base64::encode(bytes))
}

pub fn b64_to_session(val: String) -> AteSession
{
    let val = val.trim().to_string();
    let format = SerializationFormat::MessagePack;
    let bytes = base64::decode(val).unwrap();
    format.deserialize( &bytes).unwrap()
}

#[allow(dead_code)]
pub fn is_public_domain(domain: &str) -> bool {
    match domain {
        "gmail.com" => true,
        "zoho.com" => true,
        "outlook.com" => true,
        "hotmail.com" => true,
        "mail.com" => true,
        "yahoo.com" => true,
        "gmx.com" => true,
        "hushmail.com" => true,
        "hush.com" => true,
        "inbox.com" => true,
        "aol.com" => true,
        "yandex.com" => true,
        _ => false
    }
}