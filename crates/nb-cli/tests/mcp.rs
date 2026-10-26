//! MCP disclosure and gating, over the in-process transport, stdio, and HTTP.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;

use common::{argv, bin};
use futures::StreamExt;
use incurs::mcp::{McpHttpBody, McpHttpConfig, McpHttpRequest, McpHttpServer};
use nb_cli::{build, prescan};
use serde_json::{Value, json};

struct Server(McpHttpServer);

impl Server {
    fn new(flags: &[&str]) -> Self {
        let (gate, _) = prescan(argv(flags)).unwrap();
        Self(McpHttpServer::from_cli(&build(&gate), McpHttpConfig::default()).unwrap())
    }

    async fn rpc(&self, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let headers = [
            ("host", "localhost"),
            ("accept", "application/json, text/event-stream"),
            ("content-type", "application/json"),
            ("mcp-protocol-version", "2025-06-18"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .to_vec();
        let res = self
            .0
            .handle(McpHttpRequest {
                method: "POST".into(),
                path: "/mcp".into(),
                headers,
                body: body.to_string().into_bytes(),
            })
            .await;
        let raw = match res.body {
            McpHttpBody::Empty => String::new(),
            McpHttpBody::Full(b) => String::from_utf8(b).unwrap(),
            McpHttpBody::EventStream(s) => s.collect::<Vec<_>>().await.concat(),
        };
        parse_rpc(&raw)
    }

    async fn tools(&self) -> Vec<Value> {
        self.rpc("tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn call(&self, name: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name": name, "arguments": args}))
            .await
    }

    async fn search(&self, query: &str) -> Vec<String> {
        let r = self
            .call("search_tools", json!({"query": query, "limit": 200}))
            .await;
        r["result"]["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
}

/// A JSON body, or the last `data:` event of a stream.
fn parse_rpc(raw: &str) -> Value {
    let text = raw
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("data:"))
        .unwrap_or(raw);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {raw}"))
}

fn names(tools: &[Value]) -> Vec<&str> {
    tools.iter().map(|t| t["name"].as_str().unwrap()).collect()
}

const META: [&str; 4] = [
    "search_tools",
    "get_tool_details",
    "call_read_tool",
    "call_write_tool",
];
const DIRECT: [&str; 4] = ["account_me", "notes_get", "notes_list", "notes_search"];

#[tokio::test]
async fn default_is_read_only() {
    let s = Server::new(&["--mcp"]);
    let tools = s.tools().await;
    let mut got = names(&tools);
    got.sort();
    let mut want = [META, DIRECT].concat();
    want.sort();
    assert_eq!(got, want);

    let found = s.search("").await;
    assert!(found.len() > 30, "{}", found.len());
    for w in [
        "notes_delete",
        "notes_create",
        "content_text_append",
        "chat_ask",
        "account_login",
        "account_graphql",
        "share_invite",
    ] {
        assert!(!found.iter().any(|f| f == w), "{w} leaked");
    }
    let r = s
        .call(
            "call_write_tool",
            json!({"name": "notes_delete", "arguments": {"note": "abcdef"}}),
        )
        .await;
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown tool"),
        "{r}"
    );
    let instructions = s
        .rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "t", "version": "1"}}),
        )
        .await["result"]["instructions"]
        .as_str()
        .unwrap()
        .to_string();
    for needle in ["id prefix", "read-only", "token_limit", "cursor"] {
        assert!(instructions.contains(needle), "{needle}: {instructions}");
    }
}

#[tokio::test]
async fn allow_writes_discloses_destructive_tools() {
    let s = Server::new(&["--mcp", "--allow-writes"]);
    let found = s.search("").await;
    assert!(found.iter().any(|f| f == "notes_delete"));
    assert!(!found.iter().any(|f| f == "chat_ask"), "AI stays gated");
    assert!(!found.iter().any(|f| f == "account_login"), "CLI-only");
    let d = s
        .call("get_tool_details", json!({"name": "notes_delete"}))
        .await;
    let d = &d["result"]["structuredContent"];
    assert_eq!(d["annotations"]["destructiveHint"], true, "{d}");
    assert!(d["inputSchema"]["properties"]["confirm"].is_object());

    let init = s
        .rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "t", "version": "1"}}),
        )
        .await;
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("confirm")
    );

    // No confirm: a dry-run preview, no request made.
    let r = s
        .call(
            "call_write_tool",
            json!({"name": "notes_delete", "arguments": {"note": "abcdef"}}),
        )
        .await;
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    let preview: Value = serde_json::from_str(text).unwrap();
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["command"], "notes delete");
}

#[tokio::test]
async fn allow_ai_discloses_ai_tools() {
    let s = Server::new(&["--mcp", "--allow-ai"]);
    let found = s.search("").await;
    assert!(found.iter().any(|f| f == "chat_ask"));
    assert!(!found.iter().any(|f| f == "notes_delete"));
    // AI tools that also write need both switches.
    let both = Server::new(&["--mcp", "--allow-ai", "--allow-writes"])
        .search("")
        .await;
    assert!(both.len() > found.len());
}

#[tokio::test]
async fn flat_and_include() {
    let flat = Server::new(&["--mcp", "--mcp-flat"]).tools().await;
    assert!(flat.len() > 30);
    assert!(!names(&flat).contains(&"search_tools"));
    assert!(!names(&flat).contains(&"notes_delete"));
    let only = Server::new(&["--mcp", "--mcp-flat", "--mcp-tools", "folders_*"])
        .tools()
        .await;
    assert!(!only.is_empty());
    assert!(
        names(&only).iter().all(|n| n.starts_with("folders_")),
        "{only:?}"
    );
}

#[tokio::test]
async fn resources() {
    let s = Server::new(&["--mcp"]);
    let list = s.rpc("resources/list", json!({})).await;
    let uris: Vec<&str> = list["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["notability://folders", "notability://notes/recent"]);
    let t = s.rpc("resources/templates/list", json!({})).await;
    assert_eq!(
        t["result"]["resourceTemplates"][0]["uriTemplate"],
        "notability://note/{note_id}"
    );
    let bad = s
        .rpc("resources/read", json!({"uri": "notability://nope"}))
        .await;
    assert!(bad["error"].is_object(), "{bad}");
}

fn stdio_session(home: &std::path::Path, lines: &[Value]) -> Vec<Value> {
    let mut child = bin(home)
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut out = Vec::new();
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
        if l.get("id").is_some() {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            out.push(serde_json::from_str(&line).unwrap());
        }
    }
    drop(stdin);
    child.wait().unwrap();
    out
}

fn init() -> Value {
    json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}}})
}

#[test]
fn stdio_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let r = stdio_session(
        home.path(),
        &[
            init(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "notes_list", "arguments": {}}}),
        ],
    );
    assert_eq!(r[1]["result"]["tools"].as_array().unwrap().len(), 8);
    assert_eq!(r[2]["result"]["isError"], true);
    let err: Value =
        serde_json::from_str(r[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(err["code"], "AUTH");
}

#[tokio::test]
async fn http_round_trip() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let home = tempfile::tempdir().unwrap();
    let mut child = bin(home.path())
        .args(["--mcp", "--http", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let http = reqwest::Client::new();
    let mut tools = None;
    for _ in 0..50 {
        let req = http
            .post(&url)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2025-06-18")
            .body(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string());
        if let Ok(res) = req.send().await {
            tools = Some(parse_rpc(&res.text().await.unwrap()));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Only /mcp is served: incurs' REST command routes would bypass the gate.
    let rest = http
        .post(format!("http://127.0.0.1:{port}/notes/delete"))
        .send()
        .await
        .unwrap()
        .status();
    child.kill().unwrap();
    child.wait().unwrap();
    let tools = tools.expect("server came up");
    assert_eq!(
        tools["result"]["tools"].as_array().unwrap().len(),
        8,
        "{tools}"
    );
    assert_eq!(rest, 404);
}
