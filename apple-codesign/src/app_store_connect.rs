// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use {
    crate::AppleCodesignError,
    jsonwebtoken::{Algorithm, EncodingKey, Header},
    log::error,
    reqwest::blocking::Client,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    std::{path::Path, sync::Mutex, time::SystemTime},
};

pub const ITUNES_PRODUCER_SERVICE_URL: &str = "https://contentdelivery.itunes.apple.com/WebObjects/MZLabelService.woa/json/MZITunesProducerService";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ConnectTokenRequest {
    iss: String,
    iat: u64,
    exp: u64,
    aud: String,
}

/// A JWT Token for use with App Store Connect API.
pub type AppStoreConnectToken = String;

/// Represents a private key used to create JWT tokens for use with App Store Connect.
///
/// See https://developer.apple.com/documentation/appstoreconnectapi/creating_api_keys_for_app_store_connect_api
/// and https://developer.apple.com/documentation/appstoreconnectapi/generating_tokens_for_api_requests
/// for more details.
///
/// This entity holds the necessary metadata to issue new JWT tokens.
///
/// App Store Connect API tokens/JWTs are derived from:
///
/// * A key identifier. This is a short alphanumeric string like `DEADBEEF42`.
/// * An issuer ID. This is likely a UUID.
/// * A private key. Likely ECDSA.
///
/// All these are issued by Apple. You can log in to App Store Connect and see/manage your keys
/// at https://appstoreconnect.apple.com/access/api.
#[derive(Clone)]
pub struct ConnectTokenEncoder {
    key_id: String,
    issuer_id: String,
    encoding_key: EncodingKey,
}

impl ConnectTokenEncoder {
    /// Construct an instance from an [EncodingKey] instance.
    ///
    /// This is the lowest level API and ultimately what all constructors use.
    pub fn from_jwt_encoding_key(
        key_id: String,
        issuer_id: String,
        encoding_key: EncodingKey,
    ) -> Self {
        Self {
            key_id,
            issuer_id,
            encoding_key,
        }
    }

    /// Construct an instance from a DER encoded ECDSA private key.
    pub fn from_ecdsa_der(
        key_id: String,
        issuer_id: String,
        der_data: &[u8],
    ) -> Result<Self, AppleCodesignError> {
        let encoding_key = EncodingKey::from_ec_der(der_data);

        Ok(Self::from_jwt_encoding_key(key_id, issuer_id, encoding_key))
    }

    /// Create a token from a PEM encoded ECDSA private key.
    pub fn from_ecdsa_pem(
        key_id: String,
        issuer_id: String,
        pem_data: &[u8],
    ) -> Result<Self, AppleCodesignError> {
        let encoding_key = EncodingKey::from_ec_pem(pem_data)?;

        Ok(Self::from_jwt_encoding_key(key_id, issuer_id, encoding_key))
    }

    /// Create a token from a PEM encoded ECDSA private key in a filesystem path.
    pub fn from_ecdsa_pem_path(
        key_id: String,
        issuer_id: String,
        path: impl AsRef<Path>,
    ) -> Result<Self, AppleCodesignError> {
        let data = std::fs::read(path.as_ref())?;

        Self::from_ecdsa_pem(key_id, issuer_id, &data)
    }

    /// Attempt to construct in instance from an API Key ID.
    ///
    /// e.g. `DEADBEEF42`. This looks for an `AuthKey_<id>.p8` file in default search
    /// locations like `~/.appstoreconnect/private_keys`.
    pub fn from_api_key_id(key_id: String, issuer_id: String) -> Result<Self, AppleCodesignError> {
        let mut search_paths = vec![std::env::current_dir()?.join("private_keys")];

        if let Some(home) = dirs::home_dir() {
            search_paths.extend([
                home.join("private_keys"),
                home.join(".private_keys"),
                home.join(".appstoreconnect").join("private_keys"),
            ]);
        }

        // AuthKey_<apiKey>.p8
        let filename = format!("AuthKey_{}.p8", key_id);

        for path in search_paths {
            let candidate = path.join(&filename);

            if candidate.exists() {
                return Self::from_ecdsa_pem_path(key_id, issuer_id, candidate);
            }
        }

        Err(AppleCodesignError::AppStoreConnectApiKeyNotFound)
    }

    /// Mint a new JWT token.
    ///
    /// Using the private key and key metadata bound to this instance, we issue a new JWT
    /// for the requested duration.
    pub fn new_token(&self, duration: u64) -> Result<AppStoreConnectToken, AppleCodesignError> {
        let header = Header {
            kid: Some(self.key_id.clone()),
            alg: Algorithm::ES256,
            ..Default::default()
        };

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("calculating UNIX time should never fail")
            .as_secs();

        let claims = ConnectTokenRequest {
            iss: self.issuer_id.clone(),
            iat: now,
            exp: now + duration,
            aud: "appstoreconnect-v1".to_string(),
        };

        let token = jsonwebtoken::encode(&header, &claims, &self.encoding_key)?;

        Ok(token)
    }
}

// The following structs are related to the Notary API, as documented at
// https://developer.apple.com/documentation/notaryapi.

/// A notification that the notary service sends you when notarization finishes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubmissionRequestNotification {
    pub channel: String,
    pub target: String,
}

/// Data that you provide when starting a submission to the notary service.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubmissionRequest {
    pub notifications: Vec<NewSubmissionRequestNotification>,
    pub sha256: String,
    pub submission_name: String,
}

/// Information that you use to upload your software for notarization.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubmissionResponseDataAttributes {
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub aws_session_token: String,
    pub bucket: String,
    pub object: String,
}

/// Information that the notary service provides for uploading your software for notarization and
/// tracking the submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubmissionResponseData {
    pub attributes: NewSubmissionResponseDataAttributes,
    pub id: String,
    pub r#type: String,
}

/// The notary service’s response to a software submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubmissionResponse {
    pub data: NewSubmissionResponseData,
    pub meta: Value,
}

const APPLE_NOTARY_SUBMIT_SOFTWARE_URL: &str =
    "https://appstoreconnect.apple.com/notary/v2/submissions";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum SubmissionResponseStatus {
    Accepted,
    #[serde(rename = "In Progress")]
    InProgress,
    Invalid,
    Rejected,
    #[serde(other)]
    Unknown,
}

/// Information about the status of a submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionResponseDataAttributes {
    pub created_date: String,
    pub name: String,
    pub status: SubmissionResponseStatus,
}

/// Information that the service provides about the status of a notarization submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionResponseData {
    pub attributes: SubmissionResponseDataAttributes,
    pub id: String,
    pub r#type: String,
}

/// The notary service’s response to a request for the status of a submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionResponse {
    pub data: SubmissionResponseData,
    pub meta: Value,
}

impl SubmissionResponse {
    /// Convert the instance into a [Result].
    ///
    /// Will yield [Err] if the notarization/upload was not successful.
    pub fn into_result(self) -> Result<Self, AppleCodesignError> {
        match self.data.attributes.status {
            SubmissionResponseStatus::Accepted => Ok(self),
            SubmissionResponseStatus::InProgress => Err(AppleCodesignError::NotarizeIncomplete),
            SubmissionResponseStatus::Invalid => Err(AppleCodesignError::NotarizeInvalid),
            SubmissionResponseStatus::Rejected => Err(AppleCodesignError::NotarizeRejected(
                0,
                "Notarization error".into(),
            )),
            SubmissionResponseStatus::Unknown => Err(AppleCodesignError::NotarizeInvalid),
        }
    }
}

/// Information about the log associated with the submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionLogResponseDataAttributes {
    developer_log_url: String,
}

/// Data that indicates how to get the log information for a particular submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionLogResponseData {
    pub attributes: SubmissionLogResponseDataAttributes,
    pub id: String,
    pub r#type: String,
}

/// The notary service’s response to a request for the log information about a completed submission.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionLogResponse {
    pub data: SubmissionLogResponseData,
    pub meta: Value,
}

/// A client for App Store Connect API.
///
/// The client isn't generic. Don't get any ideas.
pub struct AppStoreConnectClient {
    client: Client,
    connect_token: ConnectTokenEncoder,
    token: Mutex<Option<AppStoreConnectToken>>,
}

impl AppStoreConnectClient {
    pub fn new(connect_token: ConnectTokenEncoder) -> Result<Self, AppleCodesignError> {
        Ok(Self {
            client: crate::ticket_lookup::default_client()?,
            connect_token,
            token: Mutex::new(None),
        })
    }

    fn get_token(&self) -> Result<String, AppleCodesignError> {
        let mut token = self.token.lock().unwrap();

        // TODO need to handle token expiration.
        if token.is_none() {
            token.replace(self.connect_token.new_token(300)?);
        }

        Ok(token.as_ref().unwrap().clone())
    }

    /// Create a submission to the Notary API.
    pub fn create_submission(
        &self,
        sha256: &str,
        submission_name: &str,
    ) -> Result<NewSubmissionResponse, AppleCodesignError> {
        let token = self.get_token()?;

        let body = NewSubmissionRequest {
            notifications: Vec::new(),
            sha256: sha256.to_string(),
            submission_name: submission_name.to_string(),
        };
        let req = self
            .client
            .post(APPLE_NOTARY_SUBMIT_SOFTWARE_URL)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&body);

        let response = req.send()?;

        if response.status() == 200 {
            let res_data = response.json::<NewSubmissionResponse>()?;

            Ok(res_data)
        } else {
            error!("non-200 from Notary API NewSubmissionRequest");
            error!("{}", response.text()?);

            Err(AppleCodesignError::NotarizeServerError)
        }
    }

    /// Fetch the status of a Notary API submission.
    pub fn get_submission(
        &self,
        submission_id: &str,
    ) -> Result<SubmissionResponse, AppleCodesignError> {
        let token = self.get_token()?;

        let req = self
            .client
            .get(format!(
                "{}/{}",
                APPLE_NOTARY_SUBMIT_SOFTWARE_URL, submission_id
            ))
            .bearer_auth(token)
            .header("Accept", "application/json");

        let response = req.send()?;

        let res_data = response.json::<SubmissionResponse>()?;

        Ok(res_data)
    }

    /// Fetch details about a single completed notarization.
    pub fn get_submission_log(&self, submission_id: &str) -> Result<Value, AppleCodesignError> {
        let token = self.get_token()?;

        let req = self
            .client
            .get(format!(
                "{}/{}/logs",
                APPLE_NOTARY_SUBMIT_SOFTWARE_URL, submission_id
            ))
            .bearer_auth(token)
            .header("Accept", "application/json");

        let response = req.send()?;

        let res_data = response.json::<SubmissionLogResponse>()?;

        let url = res_data.data.attributes.developer_log_url;

        let logs = self.client.get(url).send()?.json::<Value>()?;

        Ok(logs)
    }
}
