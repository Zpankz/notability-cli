//! Notability domain layer: library model, note resolution and mutations, edit-log state and
//! op builders, search with the content cache, imports and text export.

pub mod client;
pub mod export;
pub mod ids;
pub mod live;
pub mod media;
pub mod models;
pub mod organize;
pub mod recorder;
pub mod recordings;
pub mod remote;
pub mod search;
pub mod services;
pub mod session;
pub mod state;
pub mod study;

pub use client::Client;
pub use models::{Folder, Library, Note};
pub use session::{EditSession, Selection};
pub use state::NoteState;
