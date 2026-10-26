//! Endpoints and the web client identity (web-26.43.7), overridable through the environment.

use std::sync::LazyLock;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub struct Endpoints {
    pub api: String,
    pub collab: String,
    pub graphql: String,
    pub socket: String,
}

pub static ENDPOINTS: LazyLock<Endpoints> = LazyLock::new(|| {
    let api = env_or("NOTABILITY_API_BASE", "https://notability.com")
        .trim_end_matches('/')
        .to_string();
    let collab = env_or("NOTABILITY_COLLAB_BASE", &format!("{api}/collab-api"));
    let socket = format!(
        "{}/collab-api/socket.io/",
        api.replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    );
    Endpoints {
        graphql: format!("{api}/graphql"),
        api,
        collab,
        socket,
    }
});

/// `client-*` headers the web app sends (the server enforces forced updates on old versions).
pub static CLIENT_HEADERS: LazyLock<[(&'static str, String); 4]> = LazyLock::new(|| {
    [
        ("client-platform", "WEB".to_string()),
        (
            "client-version",
            env_or("NOTABILITY_CLIENT_VERSION", "26.43.7"),
        ),
        (
            "client-git-hash",
            env_or(
                "NOTABILITY_CLIENT_GIT_HASH",
                "87273d83f7c62268c52d63d2ce48f16a4572e590",
            ),
        ),
        (
            "client-build-time",
            env_or("NOTABILITY_CLIENT_BUILD_TIME", "1792773738731"),
        ),
    ]
});

pub const USER_AGENT: &str = concat!(
    "notability-cli/",
    env!("CARGO_PKG_VERSION"),
    " (+unofficial)"
);
pub const AUTH_COOKIE: &str = "nb-auth-token";
pub const GOOGLE_CAL_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
pub const MS_CAL_SCOPE: &str = "Calendars.Read";

pub fn cdp_default() -> String {
    env_or("NOTABILITY_CDP", "http://127.0.0.1:9222")
}
