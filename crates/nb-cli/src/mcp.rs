//! MCP surface: gate excludes, server instructions, resources and the `--http` transport.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;

use incurs::cli::Cli;
use incurs::mcp::{
    McpDiscovery, McpHttpConfig, McpHttpServer, McpResource, McpResourceContents, McpResourceError,
    McpResourceReadResult, McpResourceReader, McpResourceRegistry, McpResourceTemplate,
    McpServeOptions, McpToolFilter,
};
use nb_domain::{Client, export};
use serde_json::{Value, json};

use crate::spec::{Effect, Spec};

/// Startup switches that shape the MCP surface.
#[derive(Debug, Default, Clone)]
pub struct Gate {
    pub allow_writes: bool,
    pub allow_ai: bool,
    pub flat: bool,
    pub include: Vec<String>,
    pub http: Option<SocketAddr>,
}

impl Gate {
    /// Is this command's tool hidden under the current gate?
    pub fn hides(&self, s: &Spec) -> bool {
        let needs_writes = s.effect.writes();
        let needs_ai = s.effect.ai();
        (needs_writes && !self.allow_writes)
            || (needs_ai && !self.allow_ai)
            || s.effect == Effect::Local
            || (s.local_device && self.http.is_some())
    }

    /// Tool names to exclude (plain names match exactly).
    pub fn excludes(&self, specs: &[Spec]) -> Vec<String> {
        specs
            .iter()
            .filter(|s| self.hides(s))
            .map(Spec::tool)
            .collect()
    }

    pub fn options(&self, specs: &[Spec]) -> McpServeOptions {
        McpServeOptions {
            version: Some(env!("CARGO_PKG_VERSION").into()),
            instructions: Some(self.instructions()),
            tools: McpToolFilter {
                discovery: if self.flat {
                    McpDiscovery::Direct
                } else {
                    McpDiscovery::Progressive
                },
                include: self.include.clone(),
                exclude: self.excludes(specs),
            },
            resources: resources(),
            ..Default::default()
        }
    }

    fn instructions(&self) -> String {
        let mut s = String::from(
            "Notability library tools. Reference a note by its id, an id prefix of at least 6 characters, \
             its exact title, or folder/path/title. Start with notes_list / notes_search / notes_get; \
             use search_tools to discover the rest (tool names are CLI paths joined with '_', e.g. content_math_add).\n",
        );
        s.push_str(match (self.allow_writes, self.allow_ai) {
            (false, false) => "This server is read-only: write and AI tools are not available (start with --allow-writes / --allow-ai).\n",
            (true, false) => "Writes are enabled; AI tools are not (--allow-ai).\n",
            (false, true) => "AI read tools are enabled; writes are not (--allow-writes).\n",
            (true, true) => "Writes and AI tools are enabled.\n",
        });
        if self.allow_writes {
            s.push_str("Destructive tools (destructiveHint) return a dry-run preview unless called with confirm: true; confirm with the user first.\n");
        }
        s.push_str("Large results: pass token_limit, and follow meta.next_cursor with cursor on list tools.");
        s
    }
}

fn resources() -> McpResourceRegistry {
    let res = |uri: &str, name: &str, mime: &str, desc: &str| McpResource {
        uri: uri.into(),
        name: name.into(),
        title: None,
        description: Some(desc.into()),
        mime_type: Some(mime.into()),
        size: None,
        icons: Vec::new(),
        meta: BTreeMap::new(),
    };
    McpResourceRegistry {
        resources: vec![
            res(
                FOLDERS,
                "folders",
                JSON,
                "Every folder with its full path, note count, colour and emoji.",
            ),
            res(
                RECENT,
                "recent-notes",
                JSON,
                "The 100 most recently edited notes (id, title, folder path, content types).",
            ),
        ],
        templates: vec![McpResourceTemplate {
            uri_template: format!("{NOTE}{{note_id}}"),
            name: "note".into(),
            title: None,
            description: Some(
                "A note rendered as Markdown (typed text, AI summaries, transcripts).".into(),
            ),
            mime_type: Some(MD.into()),
            icons: Vec::new(),
            meta: BTreeMap::new(),
        }],
        read: Some(McpResourceReader::new(
            |req| async move { read(&req.uri).await },
        )),
        ..Default::default()
    }
}

const NOTE: &str = "notability://note/";
const FOLDERS: &str = "notability://folders";
const RECENT: &str = "notability://notes/recent";
const JSON: &str = "application/json";
const MD: &str = "text/markdown";

fn rerr(code: i32, message: impl Into<String>) -> McpResourceError {
    McpResourceError {
        code,
        message: message.into(),
        data: None,
    }
}

async fn read(uri: &str) -> Result<McpResourceReadResult, McpResourceError> {
    let (mime, text) = render(uri).await.map_err(|e| match e.kind {
        nb_api::Kind::NotFound => rerr(-32002, e.message),
        _ => rerr(-32603, e.message),
    })?;
    Ok(McpResourceReadResult {
        contents: vec![McpResourceContents::Text {
            uri: uri.into(),
            mime_type: Some(mime.into()),
            text,
            meta: BTreeMap::new(),
        }],
        meta: BTreeMap::new(),
    })
}

async fn render(uri: &str) -> nb_api::Result<(&'static str, String)> {
    let creds = nb_api::auth::require_credentials()?;
    let cl = Client::new(nb_api::Http::new(creds.token)?);
    if let Some(id) = uri.strip_prefix(NOTE) {
        let d = export::note_details(&cl, id, true, true).await?;
        return Ok((MD, export::to_markdown(&d)));
    }
    let lib = cl.library().await?;
    let body: Value = match uri {
        FOLDERS => {
            let mut counts: HashMap<Option<&str>, usize> = HashMap::new();
            for n in lib.all_notes(false) {
                *counts.entry(n.folder_id.as_deref()).or_default() += 1;
            }
            lib.all_folders()
                .iter()
                .map(|f| {
                    json!({"id": f.id, "path": lib.folder_path(Some(&f.id)),
                           "notes": counts.get(&Some(f.id.as_str())).copied().unwrap_or(0),
                           "color": f.color, "emoji": f.emoji, "locked": f.locked})
                })
                .collect()
        }
        RECENT => {
            let fm = lib.folder_map();
            let mut notes = lib.all_notes(false);
            notes.sort_by(|a, b| {
                b.updated_at
                    .unwrap_or(0.0)
                    .total_cmp(&a.updated_at.unwrap_or(0.0))
            });
            notes
                .iter()
                .take(100)
                .map(|n| {
                    let mut v = n.summary(&fm);
                    v["folder_path"] = json!(lib.folder_path(n.folder_id.as_deref()));
                    v["uri"] = json!(format!("{NOTE}{}", n.id));
                    v
                })
                .collect()
        }
        _ => {
            return Err(nb_api::NbError::not_found(format!(
                "unknown resource {uri}"
            )));
        }
    };
    Ok((JSON, body.to_string()))
}

/// Serves `/mcp` only: incurs' full router would also expose every command over REST, ungated.
pub async fn serve_http(cli: &Cli, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let server = McpHttpServer::from_cli(cli, McpHttpConfig::default())?;
    let app = incurs::http::mcp_router(server);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("notability MCP listening on http://{addr}/mcp");
    axum::serve(listener, app).await?;
    Ok(())
}
