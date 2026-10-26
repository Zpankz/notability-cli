//! Notability web API client: HTTP + GraphQL, the `/metadata` Socket.IO namespace, collab-api
//! bundles and appends, uploads, Learn chat and calendar reads.

pub mod asr;
pub mod auth;
pub mod calendar;
pub mod chat;
pub mod collab;
pub mod config;
pub mod error;
pub mod graphql;
pub mod http;
pub mod live;
pub mod queries;
pub mod socketio;
pub mod upload;

pub use error::{Kind, NbError, Result};
pub use http::Http;
