//! Identifiers and timestamps of the core.
//!
//! The identifiers are mxt's own newtypes, not `ruma` types, so the core does
//! not depend on the SDK. They are not validated: the matrix adapter converts
//! identifiers that ruma has already parsed.

use std::fmt;

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// No validation: the adapter converts already-valid ruma identifiers.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id! {
    /// A room ID such as `!room:example.org`.
    RoomId
}

string_id! {
    /// An event ID assigned by the server.
    EventId
}

string_id! {
    /// The transaction ID of a local echo, before the server assigns an event ID.
    TxnId
}

string_id! {
    /// A user ID such as `@alice:example.org`.
    UserId
}

/// Identifies a `MatrixEffect::Send` in its answer, `Msg::SendQueued` or
/// `Msg::SendFailed`. The core numbers its sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SendId(pub(crate) u64);

impl SendId {
    /// The core numbers its sends and an adapter only passes the IDs back,
    /// so this is only for the adapters' tests (the `testing` feature).
    #[cfg(any(test, feature = "testing"))]
    pub fn new(n: u64) -> SendId {
        SendId(n)
    }
}

/// UTC milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    pub fn from_millis(ms: u64) -> Self {
        Self(ms)
    }

    pub fn as_millis(self) -> u64 {
        self.0
    }
}
