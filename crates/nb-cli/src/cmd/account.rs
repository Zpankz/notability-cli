use std::io::Read as _;
use std::time::Duration;

use nb_api::auth::{
    Credentials, credentials_path, delete_credentials, import_from_cdp, load_credentials,
    require_credentials, save_credentials,
};
use nb_api::config::cdp_default;
use nb_api::{Http, NbError, Result};
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

const TOGGLES: [&str; 3] = ["ai", "learn", "transcription"];

pub fn specs() -> Vec<Spec> {
    let cdp = || {
        text(
            "cdp",
            "CDP endpoint of a browser signed in to notability.com (default $NOTABILITY_CDP or http://127.0.0.1:9222).",
        )
    };
    vec![
        Spec::new(
            "account login",
            Effect::Local,
            "Import a session (from a browser over CDP, or a token on stdin)",
            login,
        )
        .opt(cdp())
        .opt(flag(
            "token_stdin",
            "Read the nb-auth-token value from stdin instead.",
        )),
        Spec::new(
            "account refresh",
            Effect::Local,
            "Re-import the session cookie",
            refresh,
        )
        .opt(cdp()),
        Spec::new(
            "account logout",
            Effect::Local,
            "Forget the stored session",
            logout,
        ),
        Spec::new(
            "account status",
            Effect::R,
            "Is the session valid? (--offline)",
            status,
        )
        .opt(flag(
            "offline",
            "Do not call the API; only inspect the stored token.",
        )),
        Spec::new("account me", Effect::R, "Profile of the signed-in user", me).direct(),
        Spec::new(
            "account quotas",
            Effect::R,
            "AI / transcription quotas",
            quotas,
        ),
        Spec::new("account flags", Effect::R, "Feature flags", flags),
        Spec::new("account stickers", Effect::R, "Sticker packs", stickers),
        Spec::new(
            "account survey",
            Effect::R,
            "Onboarding survey answers",
            survey,
        ),
        Spec::new(
            "account calendar",
            Effect::R,
            "Upcoming calendar events",
            calendar,
        )
        .opt(num_def("days", 7.0, "Look-ahead window (days)."))
        .opt(alias(num_def("limit", 50.0, "Max events."), 'n')),
        Spec::new(
            "account graphql",
            Effect::R,
            "Raw GraphQL query (queries only)",
            graphql,
        )
        .cli_only()
        .arg(req("query", "GraphQL query document."))
        .opt(text_def("vars", "{}", "JSON variables.")),
        Spec::new(
            "account settings get",
            Effect::R,
            "Show settings",
            settings_get,
        ),
        Spec::new(
            "account settings set",
            Effect::W,
            "Set a setting value (stored as a string)",
            settings_set,
        )
        .arg(req("key", "Setting key."))
        .arg(req("value", "New value.")),
        Spec::new(
            "account settings toggle",
            Effect::W,
            "Turn a boolean setting on/off",
            settings_toggle,
        )
        .arg(req_choice("name", &TOGGLES, "Toggle."))
        .arg(req_choice("state", &["on", "off"], "New state.")),
        Spec::new(
            "account settings name",
            Effect::W,
            "Change display name",
            settings_name,
        )
        .arg(req("full_name", "New display name.")),
    ]
}

fn user(me: &Value) -> Value {
    json!({"id": me["id"], "email": me["email"], "name": me.get("name")})
}

async fn import(c: &Ctx) -> Result<Value> {
    let creds = if c.b("token_stdin") {
        let mut t = String::new();
        std::io::stdin()
            .read_to_string(&mut t)
            .map_err(|e| NbError::usage(format!("cannot read stdin: {e}")))?;
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(NbError::usage("no token on stdin"));
        }
        Credentials::new(t, "stdin")
    } else {
        let cdp = c.os("cdp").unwrap_or_else(cdp_default);
        import_from_cdp(&cdp, Duration::from_secs(10)).await?
    };
    let me = Http::new(creds.token.clone())?.me().await?;
    let path = save_credentials(&creds)?;
    let mut out = creds.redacted();
    out["saved_to"] = json!(path.display().to_string());
    out["user"] = user(&me);
    Ok(out)
}

handler!(login, |c| { import(&c).await });

handler!(refresh, |c| { import(&c).await });

handler!(logout, |_c| {
    let removed = delete_credentials()?;
    Ok(json!({"removed": removed}))
});

handler!(status, |c| {
    let creds = if c.b("offline") {
        load_credentials()?
    } else {
        Some(require_credentials()?)
    };
    let Some(creds) = creds else {
        return Err(NbError::auth(
            "Not signed in. Run `notability account login`.",
        ));
    };
    let mut out =
        json!({"credentials": creds.redacted(), "path": credentials_path().display().to_string()});
    if !c.b("offline") {
        let me = Http::new(creds.token)?.me().await?;
        out["valid"] = json!(true);
        out["user"] = json!({"id": me["id"], "email": me["email"], "name": me.get("name"), "ai_enabled": me.get("isAiEnabled")});
    }
    Ok(out)
});

handler!(me, |c| {
    let cl = c.client()?;
    let mut me = cl.http.me().await?;
    me["subscription"] = cl.http.subscription().await?;
    let accts: Vec<Value> = cl
        .http
        .connected_accounts()
        .await?
        .iter()
        .map(|a| json!({"id": a["id"], "provider": a["provider"], "providerAccountEmail": a["providerAccountEmail"], "healthStatus": a["healthStatus"]}))
        .collect();
    me["connected_accounts"] = json!(accts);
    Ok(me)
});

handler!(quotas, |c| { c.client()?.quotas().await });

handler!(flags, |c| {
    let f = c.client()?.feature_flags().await?;
    Ok(json!({"flags": f}))
});

handler!(stickers, |c| {
    let r = c.client()?.stickers().await?;
    let n = r.as_array().map_or(0, Vec::len);
    Ok(json!({"count": n, "items": r}))
});

handler!(survey, |c| { c.client()?.survey().await });

handler!(calendar, |c| {
    let cl = c.client()?;
    let days = c.ou("days")?.unwrap_or(7);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let ev = cl
        .http
        .calendar_events(now, days as u32, c.ou("limit")?.unwrap_or(50) as u32)
        .await?;
    Ok(json!({"days": days, "count": ev.len(), "items": ev}))
});

handler!(graphql, |c| {
    let q = c.s("query")?;
    if q.trim_start().starts_with("mutation") {
        return Err(NbError::usage(
            "raw mutations are disabled; use the typed commands",
        ));
    }
    let vars: Value = serde_json::from_str(&c.os("vars").unwrap_or_else(|| "{}".into()))
        .map_err(|e| NbError::usage(format!("--vars is not JSON: {e}")))?;
    c.client()?.http.graphql(&q, vars, None).await
});

handler!(settings_get, |c| { c.client()?.settings().await });

handler!(settings_set, |c| {
    c.client()?.settings_set(&c.s("key")?, &c.s("value")?).await
});

handler!(settings_toggle, |c| {
    c.client()?
        .settings_toggle(&c.s("name")?, c.s("state")? == "on")
        .await
});

handler!(settings_name, |c| {
    c.client()?.settings_name(&c.s("full_name")?).await
});
