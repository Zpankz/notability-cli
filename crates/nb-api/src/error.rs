//! One error type with stable exit codes.

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Generic,
    Usage,
    Auth,
    NotFound,
    RateLimited,
    Server,
    Quota,
    Unsupported,
    WriteRejected,
}

impl Kind {
    pub const fn exit_code(self) -> i32 {
        match self {
            Kind::Generic => 1,
            Kind::Usage => 2,
            Kind::Auth => 3,
            Kind::NotFound => 4,
            Kind::RateLimited => 5,
            Kind::Server => 6,
            Kind::Quota => 7,
            Kind::Unsupported => 8,
            Kind::WriteRejected => 9,
        }
    }

    /// Stable error code string.
    pub const fn code(self) -> &'static str {
        match self {
            Kind::Generic => "error",
            Kind::Usage => "usage",
            Kind::Auth => "auth",
            Kind::NotFound => "not_found",
            Kind::RateLimited => "rate_limited",
            Kind::Server => "server",
            Kind::Quota => "quota",
            Kind::Unsupported => "unsupported",
            Kind::WriteRejected => "write_rejected",
        }
    }
}

/// Exit-code table shown in `--help`.
pub const EXIT_CODES: [(i32, &str); 10] = [
    (0, "success"),
    (1, "generic error"),
    (2, "usage error"),
    (
        3,
        "authentication missing/expired (run `notability account login`)",
    ),
    (4, "not found"),
    (5, "rate limited"),
    (6, "server / network error"),
    (7, "AI quota or subscription required"),
    (8, "operation not supported by this client"),
    (9, "write rejected by the sync server"),
];

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct NbError {
    pub kind: Kind,
    pub message: String,
    pub key: Option<String>,
    pub status: Option<u16>,
}

pub type Result<T, E = NbError> = std::result::Result<T, E>;

macro_rules! ctor {
    ($($name:ident => $kind:ident),* $(,)?) => {
        $(pub fn $name(message: impl Into<String>) -> Self {
            Self { kind: Kind::$kind, message: message.into(), key: None, status: None }
        })*
    };
}

impl NbError {
    ctor! {
        generic => Generic, usage => Usage, auth => Auth, not_found => NotFound,
        rate_limited => RateLimited, server => Server, quota => Quota,
        unsupported => Unsupported, write_rejected => WriteRejected,
    }

    pub fn with_key(mut self, key: Option<impl Into<String>>) -> Self {
        self.key = key.map(Into::into);
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }

    /// `{code, message, key?, status?}`.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("code".into(), json!(self.kind.code()));
        m.insert("message".into(), json!(self.message));
        if let Some(k) = &self.key {
            m.insert("key".into(), json!(k));
        }
        if let Some(s) = self.status {
            m.insert("status".into(), json!(s));
        }
        Value::Object(m)
    }
}

impl From<nb_protocol::codec::CodecError> for NbError {
    fn from(e: nb_protocol::codec::CodecError) -> Self {
        NbError::server(format!("edit-log codec: {e}"))
    }
}

impl From<std::io::Error> for NbError {
    fn from(e: std::io::Error) -> Self {
        NbError::generic(e.to_string())
    }
}
