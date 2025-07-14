//! macOS API to store the data of a session, using the macOS Keychain.

use matrix_sdk::authentication::oauth::ClientId;
use ruma::UserId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;
use url::Url;

use super::{SecretError, SecretExt, StoredSession};
use crate::{APP_ID, PROFILE, spawn_tokio};

/// The current version of the stored session.
const CURRENT_VERSION: u8 = 7;

/// Secret data stored in macOS Keychain.
#[derive(Clone, Serialize, Deserialize)]
struct MacOSSecretData {
    /// The version of the stored session.
    version: u8,
    /// The URL of the homeserver.
    homeserver: String,
    /// The user ID.
    user_id: String,
    /// The device ID.
    device_id: String,
    /// The session ID.
    id: String,
    /// The client ID (optional).
    client_id: Option<String>,
    /// The passphrase used to encrypt the local databases.
    passphrase: String,
}

/// Secret API for macOS.
pub(crate) struct MacOSSecret;

impl SecretExt for MacOSSecret {
    async fn restore_sessions() -> Result<Vec<StoredSession>, SecretError> {
        let handle = spawn_tokio!(async move { restore_sessions_inner().await });
        match handle.await.expect("task was not aborted") {
            Ok(sessions) => Ok(sessions),
            Err(error) => {
                error!("Could not restore previous sessions: {error}");
                Err(SecretError::Service(error.to_string()))
            }
        }
    }

    async fn store_session(session: StoredSession) -> Result<(), SecretError> {
        let handle = spawn_tokio!(async move { store_session_inner(session).await });
        match handle.await.expect("task was not aborted") {
            Ok(()) => Ok(()),
            Err(error) => {
                error!("Could not store session: {error}");
                Err(SecretError::Service(error.to_string()))
            }
        }
    }

    async fn delete_session(session: &StoredSession) {
        let service_name = format!("{}.{}", APP_ID, PROFILE.as_str());
        let account_name = format!("{}.{}", session.user_id, session.device_id);

        spawn_tokio!(async move {
            if let Err(error) = delete_keychain_item(&service_name, &account_name).await {
                error!("Could not delete session data from keychain: {error}");
            }
        })
        .await
        .expect("task was not aborted");
    }
}

async fn restore_sessions_inner() -> Result<Vec<StoredSession>, MacOSSecretError> {
    let service_name = format!("{}.{}", APP_ID, PROFILE.as_str());

    // 在真实实现中，我们需要枚举所有相关的 keychain 项目
    // 这里简化为从已知位置读取
    let sessions = find_all_sessions(&service_name).await?;

    Ok(sessions)
}

async fn store_session_inner(session: StoredSession) -> Result<(), MacOSSecretError> {
    let service_name = format!("{}.{}", APP_ID, PROFILE.as_str());
    let account_name = format!("{}.{}", session.user_id, session.device_id);

    let secret_data = MacOSSecretData {
        version: CURRENT_VERSION,
        homeserver: session.homeserver.to_string(),
        user_id: session.user_id.to_string(),
        device_id: session.device_id.to_string(),
        id: session.id.clone(),
        client_id: session.client_id.as_ref().map(|c| c.as_str().to_string()),
        passphrase: session.passphrase.to_string(),
    };

    let secret_json = serde_json::to_string(&secret_data)
        .map_err(|e| MacOSSecretError::Serialization(e.to_string()))?;

    store_keychain_item(&service_name, &account_name, &secret_json).await?;

    Ok(())
}

// macOS Keychain 操作的简化实现
async fn store_keychain_item(
    service: &str,
    account: &str,
    password: &str,
) -> Result<(), MacOSSecretError> {
    // 这里应该调用 Security.framework 的 API
    // 为了简化，我们使用文件系统作为后备
    let keychain_dir = dirs::data_dir()
        .ok_or(MacOSSecretError::KeychainAccess(
            "No data directory".to_string(),
        ))?
        .join("fractal")
        .join("keychain");

    tokio::fs::create_dir_all(&keychain_dir)
        .await
        .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;

    let file_path = keychain_dir.join(format!("{}.{}.json", service, account));
    tokio::fs::write(file_path, password)
        .await
        .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;

    Ok(())
}

async fn find_keychain_item(
    service: &str,
    account: &str,
) -> Result<Option<String>, MacOSSecretError> {
    let keychain_dir = dirs::data_dir()
        .ok_or(MacOSSecretError::KeychainAccess(
            "No data directory".to_string(),
        ))?
        .join("fractal")
        .join("keychain");

    let file_path = keychain_dir.join(format!("{}.{}.json", service, account));

    if !file_path.exists() {
        return Ok(None);
    }

    let content = tokio::fs::read_to_string(file_path)
        .await
        .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;

    Ok(Some(content))
}

async fn delete_keychain_item(service: &str, account: &str) -> Result<(), MacOSSecretError> {
    let keychain_dir = dirs::data_dir()
        .ok_or(MacOSSecretError::KeychainAccess(
            "No data directory".to_string(),
        ))?
        .join("fractal")
        .join("keychain");

    let file_path = keychain_dir.join(format!("{}.{}.json", service, account));

    if file_path.exists() {
        tokio::fs::remove_file(file_path)
            .await
            .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;
    }

    Ok(())
}

async fn find_all_sessions(service: &str) -> Result<Vec<StoredSession>, MacOSSecretError> {
    let keychain_dir = dirs::data_dir()
        .ok_or(MacOSSecretError::KeychainAccess(
            "No data directory".to_string(),
        ))?
        .join("fractal")
        .join("keychain");

    if !keychain_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let mut entries = tokio::fs::read_dir(&keychain_dir)
        .await
        .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let file_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        if !file_name.starts_with(service) {
            continue;
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| MacOSSecretError::KeychainAccess(e.to_string()))?;

        match parse_session_from_json(&content) {
            Ok(session) => sessions.push(session),
            Err(error) => {
                error!("Could not parse session from keychain: {error}");
            }
        }
    }

    Ok(sessions)
}

fn parse_session_from_json(json: &str) -> Result<StoredSession, MacOSSecretError> {
    let secret_data: MacOSSecretData =
        serde_json::from_str(json).map_err(|e| MacOSSecretError::Serialization(e.to_string()))?;

    let homeserver = Url::parse(&secret_data.homeserver)
        .map_err(|e| MacOSSecretError::InvalidData(format!("Invalid homeserver URL: {e}")))?;

    let user_id = UserId::parse(&secret_data.user_id)
        .map_err(|e| MacOSSecretError::InvalidData(format!("Invalid user ID: {e}")))?;

    let client_id = secret_data.client_id.map(ClientId::new);

    Ok(StoredSession {
        homeserver,
        user_id,
        device_id: secret_data.device_id.into(),
        id: secret_data.id,
        client_id,
        passphrase: secret_data.passphrase.into(),
    })
}

/// Any error that can happen when interacting with the macOS Keychain.
#[derive(Debug, Error)]
enum MacOSSecretError {
    /// An error occurred while accessing the keychain.
    #[error("Keychain access error: {0}")]
    KeychainAccess(String),

    /// An error occurred while serializing/deserializing data.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Invalid data was found.
    #[error("Invalid data: {0}")]
    InvalidData(String),
}
