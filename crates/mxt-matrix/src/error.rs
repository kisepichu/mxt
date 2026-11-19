//! The error type of the adapter.

use std::{fmt, path::PathBuf, sync::Arc};

use matrix_sdk::{
    ClientBuildError, HttpError, NotificationSettingsError, StoreError,
    encryption::{recovery::RecoveryError, secret_storage::SecretStorageError},
    event_cache::EventCacheError,
    room::{edit::EditError, reply::ReplyError},
    ruma::api::error::{Error as ServerError, ErrorKind},
};
use matrix_sdk_ui::timeline;

/// Every variant that wraps another error displays its message (the session
/// file errors after the path) and forwards its source, so printing the
/// source chain shows each message once.
#[derive(Debug)]
pub enum Error {
    /// Building the client: homeserver discovery or opening the stores.
    Build(ClientBuildError),
    /// Any other SDK call.
    Sdk(matrix_sdk::Error),
    /// Recovery, for example with a wrong recovery key.
    Recovery(RecoveryError),
    /// Reading or writing a file (`path`): the session file, its temporary
    /// file or its directory, a file to upload, or the download directory.
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
    /// Building a room's timeline, paginating it, or acting on its items
    /// (sending, reacting, deleting).
    Timeline(timeline::Error),
    /// An effect names a room this client does not know.
    UnknownRoom,
    /// An effect names an event with an ID that is not a Matrix event ID.
    InvalidEventId,
    /// An effect acts on the items of a room whose timeline is not open.
    TimelineNotOpen,
    /// Muting or unmuting a room.
    NotificationSettings(NotificationSettingsError),
    /// A download asks for a message that has no attachment.
    NoAttachment,
}

impl Error {
    /// The design's `M_UNKNOWN_TOKEN` handling applies: the session is gone
    /// and the data directory has to be set aside.
    pub fn is_unknown_token(&self) -> bool {
        matches!(self, Error::UnknownToken)
    }

    /// The error for the status line and `warn` logs, see `Brief`; it names
    /// no file either.
    pub(crate) fn brief(&self) -> Brief<'_> {
        let response = match self {
            Error::Sdk(error)
            | Error::Recovery(
                RecoveryError::Sdk(error)
                | RecoveryError::SecretStorage(SecretStorageError::Sdk(error)),
            ) => sdk_response(error),
            // Only a shared pagination error is left (see `timeline_error`),
            // and it keeps the URL of a failed request.
            Error::Timeline(timeline::Error::EventCacheError(
                EventCacheError::PaginationError(error),
            )) => {
                if is_unreachable(error) {
                    return Brief {
                        error: &"the request failed",
                        response: None,
                    };
                }
                sdk_response(error)
            }
            // The path names the file, which only `debug` logs get.
            Error::Io { error, .. } => {
                return Brief {
                    error,
                    response: None,
                };
            }
            // The ID of a room's rule is the room ID.
            Error::NotificationSettings(NotificationSettingsError::RuleNotFound(_)) => {
                return Brief {
                    error: &"the room's push rule was not found",
                    response: None,
                };
            }
            _ => None,
        };
        Brief {
            error: self,
            response,
        }
    }
}

/// An SDK error for the status line and `warn` logs, see `Brief`. The send
/// queue shares its errors, so `without_url` cannot take a failed request's
/// URL out; such a request shows no more than that it failed, as in
/// `Error::brief`.
pub(crate) fn brief_sdk(error: &matrix_sdk::Error) -> Brief<'_> {
    if is_unreachable(error) {
        return Brief {
            error: &"the request failed",
            response: None,
        };
    }
    Brief {
        error,
        response: sdk_response(error),
    }
}

/// A failed request names its URL, which holds the room ID; the status line
/// and the `warn` log get the error without it.
pub(crate) fn without_url(error: matrix_sdk::Error) -> matrix_sdk::Error {
    match error {
        matrix_sdk::Error::Http(error) => match *error {
            HttpError::Reqwest(error) => HttpError::Reqwest(error.without_url()).into(),
            error => error.into(),
        },
        error => error,
    }
}

/// Whether the request failed in the network layer, so no answer of the
/// homeserver arrived: a connection, timeout, reset or truncated body. Every
/// answer, an error response too, is an `HttpError::Api`; the SDK classifies
/// the same way. These are the errors `without_url` removes the URL from.
pub(crate) fn is_unreachable(error: &matrix_sdk::Error) -> bool {
    matches!(error, matrix_sdk::Error::Http(error) if matches!(**error, HttpError::Reqwest(_)))
}

/// A timeline error, with the request URL removed where the SDK error is not
/// shared (see `without_url`). `Error::brief` leaves out the URL a shared one
/// keeps.
pub(crate) fn timeline_error(error: timeline::Error) -> Error {
    match error {
        timeline::Error::Sdk(error) => without_url(error).into(),
        // The event cache hands one pagination's error to every caller that
        // waited for it; a shared one keeps its URL.
        timeline::Error::EventCacheError(EventCacheError::PaginationError(error)) => {
            match Arc::try_unwrap(error) {
                Ok(error) => without_url(error).into(),
                Err(error) => Error::Timeline(EventCacheError::PaginationError(error).into()),
            }
        }
        // Replies and edits fetch their target event when the event cache
        // lacks it, and removing a reaction redacts it at once; as an SDK
        // error, `Error::brief` shortens the homeserver's answer like any
        // other.
        timeline::Error::ReplyError(ReplyError::Fetch(error))
        | timeline::Error::EditError(timeline::EditError::RoomError(EditError::Fetch(error))) => {
            without_url(*error).into()
        }
        timeline::Error::RedactError(timeline::RedactError::HttpError(error)) => {
            without_url(error.into()).into()
        }
        error => Error::Timeline(error),
    }
}

/// An error for the status line and `warn` logs. A homeserver's error
/// response shows only its status code and error code, because its message
/// can name rooms (Synapse: "User … not in room !…"). The full error belongs
/// at `debug`.
pub(crate) struct Brief<'a> {
    error: &'a dyn fmt::Display,
    response: Option<&'a ServerError>,
}

impl fmt::Display for Brief<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(response) = self.response else {
            return self.error.fmt(f);
        };
        let status = response.status_code.as_u16();
        match response.error_kind() {
            Some(kind) => write!(f, "the homeserver answered {status} {}", kind.errcode()),
            None => write!(f, "the homeserver answered {status}"),
        }
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
            Error::Timeline(error) => error.fmt(f),
            Error::UnknownRoom => f.write_str("the room is not known to this client"),
            Error::InvalidEventId => f.write_str("the event ID is not valid"),
            Error::TimelineNotOpen => f.write_str("the room's timeline is not open"),
            Error::NotificationSettings(error) => error.fmt(f),
            Error::NoAttachment => f.write_str("the message has no attachment"),
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
            Error::Timeline(error) => error.source(),
            Error::NotificationSettings(error) => error.source(),
            Error::PasswordLoginUnsupported { .. }
            | Error::UnknownToken
            | Error::UnknownRoom
            | Error::InvalidEventId
            | Error::TimelineNotOpen
            | Error::NoAttachment => None,
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

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
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

fn sdk_response(error: &matrix_sdk::Error) -> Option<&ServerError> {
    match error {
        matrix_sdk::Error::Http(error) => http_response(error),
        _ => None,
    }
}

fn http_response(error: &HttpError) -> Option<&ServerError> {
    match error {
        HttpError::Cached(error) => http_response(error),
        error => error.as_client_api_error(),
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
        let body: StandardErrorBody = serde_json::from_value(serde_json::json!({
            "errcode": errcode,
            "error": "made up, in !room:example.org",
        }))
        .unwrap();
        let error = matrix_sdk::ruma::api::error::Error::new(
            StatusCode::FORBIDDEN,
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

    #[test]
    fn brief_server_errors_show_only_the_status_and_the_error_code() {
        let expected = "the homeserver answered 403 M_FORBIDDEN";
        let error = Error::from(server_error("M_FORBIDDEN"));
        assert_eq!(error.brief().to_string(), expected);
        let cached = Error::from(HttpError::Cached(Arc::new(server_error("M_FORBIDDEN"))));
        assert_eq!(cached.brief().to_string(), expected);
    }

    #[test]
    fn brief_other_errors_show_their_message() {
        assert_eq!(
            Error::UnknownRoom.brief().to_string(),
            Error::UnknownRoom.to_string()
        );
    }

    #[test]
    fn brief_file_and_push_rule_errors_name_no_file_and_no_room() {
        let file = Error::Io {
            path: "/home/alice/Downloads/plans.pdf".into(),
            error: std::io::ErrorKind::PermissionDenied.into(),
        };
        assert!(!file.brief().to_string().contains("plans"), "{file}");
        assert!(
            file.to_string().contains("plans.pdf"),
            "debug names the file"
        );
        let rule = NotificationSettingsError::RuleNotFound("!room:example.org".to_owned());
        let rule = Error::NotificationSettings(rule);
        assert!(!rule.brief().to_string().contains("!room"), "{rule}");
    }

    #[test]
    fn reqwest_errors_are_unreachable_and_server_answers_are_not() {
        // Any reqwest error, not only a connect or timeout error (here one
        // from building the request): no answer of the homeserver arrived.
        let request = matrix_sdk::reqwest::Proxy::all("http://[").unwrap_err();
        assert!(is_unreachable(&HttpError::Reqwest(request).into()));
        assert!(!is_unreachable(&server_error("M_UNKNOWN").into()));
    }

    #[test]
    fn brief_pagination_errors_have_no_url_also_when_shared() {
        let url = "https://example.org/_matrix/client/v3/rooms/!room:example.org/messages";
        let failed_request = || {
            let error = matrix_sdk::reqwest::Proxy::all("http://[").unwrap_err();
            let error = error.with_url(matrix_sdk::reqwest::Url::parse(url).unwrap());
            Arc::new(matrix_sdk::Error::from(HttpError::Reqwest(error)))
        };
        let pagination =
            |error| timeline::Error::EventCacheError(EventCacheError::PaginationError(error));
        let alone = timeline_error(pagination(failed_request()));
        assert!(!alone.brief().to_string().contains("!room"), "{alone}");
        let shared = failed_request();
        let _waiting = shared.clone();
        let shared = timeline_error(pagination(shared));
        assert!(shared.to_string().contains("!room"), "debug keeps the URL");
        assert_eq!(shared.brief().to_string(), "the request failed");
    }

    /// A request that failed in the network layer, with a URL that names
    /// `!room:example.org`.
    fn failed_request() -> HttpError {
        let url = "https://example.org/_matrix/client/v3/rooms/!room:example.org/event/$event";
        let error = matrix_sdk::reqwest::Proxy::all("http://[").unwrap_err();
        HttpError::Reqwest(error.with_url(matrix_sdk::reqwest::Url::parse(url).unwrap()))
    }

    #[test]
    fn brief_errors_of_fetching_targets_and_removing_reactions_name_no_room() {
        let reply = |error: HttpError| {
            timeline::Error::ReplyError(ReplyError::Fetch(Box::new(error.into())))
        };
        let edit = |error: HttpError| {
            let fetch = EditError::Fetch(Box::new(error.into()));
            timeline::Error::EditError(timeline::EditError::RoomError(fetch))
        };
        let unreact = |error| timeline::Error::RedactError(timeline::RedactError::HttpError(error));
        for error in [
            reply(failed_request()),
            edit(failed_request()),
            unreact(failed_request()),
        ] {
            let error = timeline_error(error);
            assert!(!error.brief().to_string().contains("!room"), "{error}");
        }
        for refused in [
            reply(server_error("M_FORBIDDEN")),
            unreact(server_error("M_FORBIDDEN")),
        ] {
            assert_eq!(
                timeline_error(refused).brief().to_string(),
                "the homeserver answered 403 M_FORBIDDEN"
            );
        }
    }

    #[test]
    fn brief_send_queue_errors_name_no_room() {
        // The send queue's errors are shared, so the URL stays in them.
        let failed = matrix_sdk::Error::from(failed_request());
        assert_eq!(brief_sdk(&failed).to_string(), "the request failed");
        let refused = matrix_sdk::Error::from(server_error("M_FORBIDDEN"));
        assert_eq!(
            brief_sdk(&refused).to_string(),
            "the homeserver answered 403 M_FORBIDDEN"
        );
    }
}
