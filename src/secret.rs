use std::{collections::HashMap, fmt, path::PathBuf, string::FromUtf8Error};

use gettextrs::gettext;
use gtk::{gio, glib};
use libsecret::{
    password_clear_future, password_search_sync, password_store_binary_future, prelude::*,
    Retrievable, Schema, SchemaAttributeType, SchemaFlags, SearchFlags, Value, COLLECTION_DEFAULT,
};
use log::error;
use matrix_sdk::ruma::{DeviceId, OwnedDeviceId, OwnedUserId, UserId};
use serde::{Deserialize, Serialize};
use serde_json::error::Error as JsonError;
use url::Url;

use crate::{config::APP_ID, gettext_f, ErrorSubpage, UserFacingError};

/// Any error that can happen when interacting with the secret service.
#[derive(Debug, Clone)]
pub enum SecretError {
    CorruptSession((String, Retrievable)),
    Libsecret(glib::Error),
    Unknown,
}

impl SecretError {
    /// Get the error subpage that matches `self`.
    pub fn error_subpage(&self) -> ErrorSubpage {
        match self {
            Self::CorruptSession(_) => ErrorSubpage::SecretErrorSession,
            _ => ErrorSubpage::SecretErrorOther,
        }
    }
}

impl From<glib::Error> for SecretError {
    fn from(error: glib::Error) -> Self {
        Self::Libsecret(error)
    }
}

impl UserFacingError for SecretError {
    fn to_user_facing(self) -> String {
        match self {
            Self::CorruptSession((message, _)) => message,
            Self::Libsecret(error) if error.is::<libsecret::Error>() => {
                match error.kind::<libsecret::Error>() {
                    Some(libsecret::Error::Protocol) => error.message().to_owned(),
                    Some(libsecret::Error::IsLocked) => {
                        gettext("Could not unlock the secret storage")
                    }
                    _ => gettext(
                        "An unknown error occurred when interacting with the secret storage",
                    ),
                }
            }
            _ => gettext("An unknown error occurred when interacting with the secret storage"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredSession {
    pub homeserver: Url,
    pub user_id: OwnedUserId,
    pub device_id: OwnedDeviceId,
    pub path: PathBuf,
    pub secret: Secret,
}

impl StoredSession {
    /// Build self from a secret.
    pub async fn try_from_secret_item(item: Retrievable) -> Result<Self, SecretError> {
        let attr = item.attributes();

        let homeserver = match attr.get("homeserver") {
            Some(string) => match Url::parse(string) {
                Ok(homeserver) => homeserver,
                Err(err) => {
                    error!(
                        "Could not parse 'homeserver' attribute in stored session: {:?}",
                        err
                    );
                    return Err(SecretError::CorruptSession((
                        gettext("Malformed homeserver in stored session"),
                        item,
                    )));
                }
            },
            None => {
                return Err(SecretError::CorruptSession((
                    gettext("Could not find homeserver in stored session"),
                    item,
                )));
            }
        };
        let user_id = match attr.get("user") {
            Some(string) => match UserId::parse(string.as_str()) {
                Ok(user_id) => user_id,
                Err(err) => {
                    error!(
                        "Could not parse 'user' attribute in stored session: {:?}",
                        err
                    );
                    return Err(SecretError::CorruptSession((
                        gettext("Malformed user ID in stored session"),
                        item,
                    )));
                }
            },
            None => {
                return Err(SecretError::CorruptSession((
                    gettext("Could not find user ID in stored session"),
                    item,
                )));
            }
        };
        let device_id = match attr.get("device-id") {
            Some(string) => <&DeviceId>::from(string.as_str()).to_owned(),
            None => {
                return Err(SecretError::CorruptSession((
                    gettext("Could not find device ID in stored session"),
                    item,
                )));
            }
        };
        let path = match attr.get("db-path") {
            Some(string) => PathBuf::from(string),
            None => {
                return Err(SecretError::CorruptSession((
                    gettext("Could not find database path in stored session"),
                    item,
                )));
            }
        };
        let secret = match item.retrieve_secret_future().await {
            Ok(Some(value)) => match Secret::from_utf8(value.get()) {
                Ok(secret) => secret,
                Err(err) => {
                    error!("Could not parse secret in stored session: {:?}", err);
                    return Err(SecretError::CorruptSession((
                        gettext("Malformed secret in stored session"),
                        item,
                    )));
                }
            },
            Ok(None) => {
                return Err(SecretError::CorruptSession((
                    gettext("No secret in stored session"),
                    item,
                )));
            }
            Err(err) => {
                error!("Could not get secret in stored session: {:?}", err);
                return Err(SecretError::CorruptSession((
                    gettext("Could not get secret in stored session"),
                    item,
                )));
            }
        };

        Ok(Self {
            homeserver,
            user_id,
            device_id,
            path,
            secret,
        })
    }

    /// Build a secret from `self`.
    ///
    /// Returns an (attributes, secret) tuple.
    pub fn to_secret_item(&self) -> (HashMap<&str, &str>, Value) {
        let attributes = HashMap::from([
            ("homeserver", self.homeserver.as_str()),
            ("user", self.user_id.as_str()),
            ("device-id", self.device_id.as_str()),
            ("db-path", self.path.to_str().unwrap()),
        ]);

        let secret = Value::new(&self.secret.to_string(), "application/json");

        (attributes, secret)
    }
}

/// A possible error value when converting a `Secret` from a UTF-8 byte vector.
#[derive(Debug)]
pub enum FromUtf8SecretError {
    Str(FromUtf8Error),
    Json(JsonError),
}

impl From<FromUtf8Error> for FromUtf8SecretError {
    fn from(err: FromUtf8Error) -> Self {
        Self::Str(err)
    }
}

impl From<JsonError> for FromUtf8SecretError {
    fn from(err: JsonError) -> Self {
        Self::Json(err)
    }
}

/// A `Secret` that can be stored in the `SecretService`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Secret {
    pub access_token: String,
    pub passphrase: String,
}

impl Secret {
    /// Converts a vector of bytes to a `Secret`.
    pub fn from_utf8(vec: Vec<u8>) -> Result<Self, FromUtf8SecretError> {
        let s = String::from_utf8(vec)?;
        Ok(serde_json::from_str(&s)?)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap())
    }
}

/// The `Schema` of the items in the `SecretService`.
fn schema() -> Schema {
    let attributes = HashMap::from([
        ("homeserver", SchemaAttributeType::String),
        ("user", SchemaAttributeType::String),
        ("device-id", SchemaAttributeType::String),
        ("db-path", SchemaAttributeType::String),
    ]);

    Schema::new(APP_ID, SchemaFlags::NONE, attributes)
}

/// Retrieves all sessions stored to the `SecretService`
pub async fn restore_sessions() -> Result<Vec<StoredSession>, SecretError> {
    let items = password_search_sync(
        Some(&schema()),
        HashMap::new(),
        SearchFlags::ALL | SearchFlags::UNLOCK | SearchFlags::LOAD_SECRETS,
        gio::Cancellable::NONE,
    )?;
    let mut sessions = Vec::with_capacity(items.len());

    for item in items {
        sessions.push(StoredSession::try_from_secret_item(item).await?);
    }

    Ok(sessions)
}

/// Writes a session to the `SecretService`, overwriting any previously stored
/// session with the same `homeserver`, `username` and `device-id`.
pub async fn store_session(session: &StoredSession) -> Result<(), SecretError> {
    let (attributes, secret) = session.to_secret_item();

    password_store_binary_future(
        Some(&schema()),
        attributes,
        Some(&COLLECTION_DEFAULT),
        &gettext_f(
            // Translators: Do NOT translate the content between '{' and '}', this is a variable
            // name.
            "Fractal: Matrix credentials for {user_id}",
            &[("user_id", session.user_id.as_str())],
        ),
        &secret,
    )
    .await?;

    Ok(())
}

/// Removes a session from the `SecretService`
pub async fn remove_session(session: &StoredSession) -> Result<(), SecretError> {
    let (attributes, _) = session.to_secret_item();

    password_clear_future(Some(&schema()), attributes).await?;

    Ok(())
}

/// Removes an item from the `SecretService`
pub async fn remove_item(item: &Retrievable) -> Result<(), SecretError> {
    let attributes = item.attributes();
    let mut attr = HashMap::with_capacity(attributes.len());

    for (key, value) in attributes.iter() {
        attr.insert(key.as_str(), value.as_str());
    }
    password_clear_future(Some(&schema()), attr).await?;

    Ok(())
}
