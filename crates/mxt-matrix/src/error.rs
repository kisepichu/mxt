//! The error type of the adapter's startup calls.

use std::{fmt, path::PathBuf};

use matrix_sdk::{
    ClientBuildError, HttpError,
    encryption::{recovery::RecoveryError, secret_storage::SecretStorageError},
    ruma::api::error::ErrorKind,
};

/// Every variant that wraps another error displays its message (the session
/// file errors after the path) and forwards its source, so printing the
/// source chain shows each message once.
#[derive(Debug)]
pub enum Error {
    /// Building the client: homeserver discovery, `/versions` (including a
    /// homeserver without simplified sliding sync), or opening the stores.
    Build(ClientBuildError),
    /// Any other SDK call.
    Sdk(matrix_sdk::Error),
    /// Recovery, for example with a wrong recovery key.
    Recovery(RecoveryError),
    /// Reading or writing the session file, its temporary file or its
    /// directory (`path`).
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// Encoding or decoding the session file.
    Json {
        path: PathBuf,
        error: serde_json::Error,
    },
    /// The homeserver does not offer `m.login.password`.
    PasswordLoginUnsupported { offered: Vec<String> },
    /// The homeserver no longer accepts the access token (`M_UNKNOWN_TOKEN`).
    UnknownToken,
}

impl Error {
    /// The design's `M_UNKNOWN_TOKEN` handling applies: the session is gone
    /// and the data directory has to be set aside.
    pub fn is_unknown_token(&self) -> bool {
        matches!(self, Error::UnknownToken)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The SDK does not mark the cause of a failed discovery as the
            // error's source, so it is shown here.
            Error::Build(ClientBuildError::AutoDiscovery(cause)) => {
                write!(f, "homeserver discovery failed: {cause}")
            }
            Error::Build(error) => error.fmt(f),
            Error::Sdk(error) => error.fmt(f),
            Error::Recovery(error) => error.fmt(f),
            Error::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Error::Json { path, error } => write!(f, "{}: {error}", path.display()),
            Error::PasswordLoginUnsupported { offered } => write!(
                f,
                "the homeserver does not offer password login (m.login.password); it offers: {}",
                offered.join(", ")
            ),
            Error::UnknownToken => {
                f.write_str("the homeserver no longer accepts this session (M_UNKNOWN_TOKEN)")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Build(error) => error.source(),
            Error::Sdk(error) => error.source(),
            Error::Recovery(error) => error.source(),
            Error::Io { error, .. } => error.source(),
            Error::Json { error, .. } => error.source(),
            Error::PasswordLoginUnsupported { .. } | Error::UnknownToken => None,
        }
    }
}

impl From<ClientBuildError> for Error {
    fn from(error: ClientBuildError) -> Self {
        Error::Build(error)
    }
}

impl From<matrix_sdk::Error> for Error {
    fn from(error: matrix_sdk::Error) -> Self {
        if is_unknown_token(&error) {
            Error::UnknownToken
        } else {
            Error::Sdk(error)
        }
    }
}

impl From<HttpError> for Error {
    fn from(error: HttpError) -> Self {
        matrix_sdk::Error::from(error).into()
    }
}

impl From<RecoveryError> for Error {
    fn from(error: RecoveryError) -> Self {
        let sdk_error = match &error {
            RecoveryError::Sdk(error)
            | RecoveryError::SecretStorage(SecretStorageError::Sdk(error)) => Some(error),
            _ => None,
        };
        if sdk_error.is_some_and(is_unknown_token) {
            Error::UnknownToken
        } else {
            Error::Recovery(error)
        }
    }
}

fn is_unknown_token(error: &matrix_sdk::Error) -> bool {
    matches!(error, matrix_sdk::Error::Http(error) if is_unknown_token_http(error))
}

/// Also inside `HttpError::Cached`: the SDK fetches `/versions` lazily before
/// the first request that needs it, and hands that request's error to every
/// request that waited for it. With a saved session, that is where an
/// `M_UNKNOWN_TOKEN` shows up first.
fn is_unknown_token_http(error: &HttpError) -> bool {
    match error {
        HttpError::Cached(error) => is_unknown_token_http(error),
        error => matches!(
            error.client_api_error_kind(),
            Some(ErrorKind::UnknownToken(_))
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use matrix_sdk::{
        reqwest::StatusCode,
        ruma::api::{
            client::uiaa::UiaaResponse,
            error::{ErrorBody, FromHttpResponseError, StandardErrorBody},
        },
    };

    use super::*;

    fn server_error(errcode: &str) -> HttpError {
        let body: StandardErrorBody =
            serde_json::from_value(serde_json::json!({ "errcode": errcode, "error": "made up" }))
                .unwrap();
        let error = matrix_sdk::ruma::api::error::Error::new(
            StatusCode::UNAUTHORIZED,
            ErrorBody::Standard(body),
        );
        FromHttpResponseError::Server(UiaaResponse::MatrixError(error)).into()
    }

    #[test]
    fn unknown_token_responses_become_unknown_token() {
        assert!(Error::from(server_error("M_UNKNOWN_TOKEN")).is_unknown_token());
        assert!(
            Error::from(matrix_sdk::Error::from(server_error("M_UNKNOWN_TOKEN")))
                .is_unknown_token()
        );
        let recovery = RecoveryError::Sdk(server_error("M_UNKNOWN_TOKEN").into());
        assert!(Error::from(recovery).is_unknown_token());
        let cached = HttpError::Cached(Arc::new(server_error("M_UNKNOWN_TOKEN")));
        assert!(Error::from(cached).is_unknown_token());
    }

    #[test]
    fn other_server_errors_keep_their_message() {
        let error = Error::from(server_error("M_FORBIDDEN"));
        assert!(!error.is_unknown_token());
        assert!(error.to_string().contains("made up"), "{error}");
    }
}
