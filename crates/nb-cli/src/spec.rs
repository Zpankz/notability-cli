//! Table-driven command registry: one [`Spec`] per command, turned into incurs groups.

use std::future::Future;
use std::io::IsTerminal;
use std::pin::Pin;

use async_trait::async_trait;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions, McpResultContent,
};
use incurs::output::{CommandResult, CtaBlock, CtaEntry, Format};
use incurs::schema::{FieldMeta, FieldType};
use nb_api::{NbError, Result};
use nb_domain::Client;
use serde_json::{Value, json};

/// Effect class: drives MCP annotations, gating and the destructive confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Reads remote state.
    R,
    /// Changes remote state.
    W,
    /// Changes remote state irreversibly or widens exposure.
    D,
    /// Spends AI quota, reads only.
    Ai,
    /// Spends AI quota and writes.
    Aiw,
    /// Touches only local files (credentials).
    Local,
}

impl Effect {
    pub fn writes(self) -> bool {
        matches!(self, Effect::W | Effect::D | Effect::Aiw)
    }
    pub fn ai(self) -> bool {
        matches!(self, Effect::Ai | Effect::Aiw)
    }
    pub fn label(self) -> &'static str {
        match self {
            Effect::R => "R",
            Effect::W => "W",
            Effect::D => "D",
            Effect::Ai => "AI",
            Effect::Aiw => "AIW",
            Effect::Local => "LOCAL",
        }
    }
}

pub type BoxFut = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
pub type Run = fn(Ctx) -> BoxFut;

/// One command.
pub struct Spec {
    pub path: &'static str,
    pub help: &'static str,
    pub effect: Effect,
    /// Listed directly in progressive MCP discovery.
    pub direct: bool,
    /// Never exposed over MCP.
    pub cli_only: bool,
    /// Mapped from the web client but not yet confirmed against the live server.
    pub unverified: bool,
    /// Uses this machine's devices (microphone); hidden over `--http`.
    pub local_device: bool,
    /// Success data carries `image: {data, mimeType}` for an MCP image block.
    pub image_result: bool,
    pub args: Vec<FieldMeta>,
    pub opts: Vec<FieldMeta>,
    pub run: Run,
}

impl Spec {
    pub fn new(path: &'static str, effect: Effect, help: &'static str, run: Run) -> Self {
        Self {
            path,
            help,
            effect,
            direct: false,
            cli_only: effect == Effect::Local,
            unverified: false,
            local_device: false,
            image_result: false,
            args: Vec::new(),
            opts: Vec::new(),
            run,
        }
    }
    pub fn direct(mut self) -> Self {
        self.direct = true;
        self
    }
    pub fn cli_only(mut self) -> Self {
        self.cli_only = true;
        self
    }
    pub fn local_device(mut self) -> Self {
        self.local_device = true;
        self
    }
    pub fn image_result(mut self) -> Self {
        self.image_result = true;
        self
    }
    pub fn unverified(mut self) -> Self {
        self.unverified = true;
        self
    }
    pub fn arg(mut self, f: FieldMeta) -> Self {
        self.args.push(f);
        self
    }
    pub fn opt(mut self, f: FieldMeta) -> Self {
        self.opts.push(f);
        self
    }
    /// MCP tool name: the command path joined with `_`.
    pub fn tool(&self) -> String {
        self.path.replace(' ', "_")
    }
    pub fn description(&self) -> String {
        if self.unverified {
            format!("{} (unverified)", self.help)
        } else {
            self.help.to_string()
        }
    }

    pub fn into_def(self) -> CommandDef {
        let name = self
            .path
            .rsplit(' ')
            .next()
            .unwrap_or(self.path)
            .to_string();
        let ro = matches!(self.effect, Effect::R | Effect::Ai);
        let destructive = self.effect == Effect::D;
        let description = self.description();
        let mut opts = self.opts;
        if destructive {
            opts.push(confirm_field());
        }
        let mut def = CommandDef::build(
            name,
            DynHandler {
                run: self.run,
                destructive,
                path: self.path,
            },
        )
        .description(description)
        .mcp(McpCommandOptions {
            enabled: !self.cli_only,
            direct: self.direct,
            destructive,
            annotations: Some(McpAnnotations {
                read_only_hint: Some(ro),
                destructive_hint: Some(destructive),
                idempotent_hint: Some(self.effect == Effect::R),
                open_world_hint: Some(true),
                ..Default::default()
            }),
            result_content: if self.image_result {
                vec![McpResultContent::Image {
                    data_pointer: "/image/data".into(),
                    mime_type_pointer: "/image/mimeType".into(),
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        })
        .destructive(destructive)
        .done();
        def.args_fields = self.args;
        def.options_fields = opts;
        def
    }
}

/// The destructive confirm: `--yes/-y` on the CLI, `confirm: true` on MCP.
fn confirm_field() -> FieldMeta {
    FieldMeta {
        cli_name: "yes".into(),
        alias: Some('y'),
        ..flag(
            "confirm",
            "Confirm the destructive action (MCP: confirm=true; otherwise a dry-run preview).",
        )
    }
}

// ---- field builders ----

fn field(name: &'static str, desc: &'static str, ty: FieldType) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.replace('_', "-"),
        description: Some(desc),
        field_type: ty,
        required: false,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

/// Required positional / option string.
pub fn req(name: &'static str, desc: &'static str) -> FieldMeta {
    FieldMeta {
        required: true,
        ..field(name, desc, FieldType::String)
    }
}
pub fn text(name: &'static str, desc: &'static str) -> FieldMeta {
    field(name, desc, FieldType::String)
}
pub fn text_def(name: &'static str, def: &'static str, desc: &'static str) -> FieldMeta {
    FieldMeta {
        default: Some(json!(def)),
        ..field(name, desc, FieldType::String)
    }
}
pub fn flag(name: &'static str, desc: &'static str) -> FieldMeta {
    field(name, desc, FieldType::Boolean)
}
/// Boolean that defaults on; `--no-NAME` turns it off.
pub fn flag_on(name: &'static str, desc: &'static str) -> FieldMeta {
    FieldMeta {
        default: Some(json!(true)),
        ..field(name, desc, FieldType::Boolean)
    }
}
pub fn num(name: &'static str, desc: &'static str) -> FieldMeta {
    field(name, desc, FieldType::Number)
}
pub fn num_def(name: &'static str, def: f64, desc: &'static str) -> FieldMeta {
    FieldMeta {
        default: Some(if def.fract() == 0.0 {
            json!(def as i64)
        } else {
            json!(def)
        }),
        ..field(name, desc, FieldType::Number)
    }
}
pub fn req_num(name: &'static str, desc: &'static str) -> FieldMeta {
    FieldMeta {
        required: true,
        ..field(name, desc, FieldType::Number)
    }
}
pub fn choice(
    name: &'static str,
    choices: &[&str],
    def: Option<&str>,
    desc: &'static str,
) -> FieldMeta {
    FieldMeta {
        default: def.map(|d| json!(d)),
        ..field(
            name,
            desc,
            FieldType::Enum(choices.iter().map(|s| s.to_string()).collect()),
        )
    }
}
pub fn req_choice(name: &'static str, choices: &[&str], desc: &'static str) -> FieldMeta {
    FieldMeta {
        required: true,
        ..choice(name, choices, None, desc)
    }
}
pub fn multi(name: &'static str, desc: &'static str) -> FieldMeta {
    field(name, desc, FieldType::Array(Box::new(FieldType::String)))
}
pub fn req_multi(name: &'static str, desc: &'static str) -> FieldMeta {
    FieldMeta {
        required: true,
        ..multi(name, desc)
    }
}
pub fn alias(mut f: FieldMeta, c: char) -> FieldMeta {
    f.alias = Some(c);
    f
}
pub fn cli_name(mut f: FieldMeta, name: &str) -> FieldMeta {
    f.cli_name = name.into();
    f
}

/// The shared note-reference argument.
pub fn note_arg() -> FieldMeta {
    req(
        "note",
        "Note id, id prefix (>=6), exact title, or folder/path/title.",
    )
}

/// The list paging options: `--limit/-n`, `--cursor`, hidden-ish `--offset`.
pub fn paging(spec: Spec, limit: f64) -> Spec {
    spec.opt(alias(num_def("limit", limit, "Max rows (0 = all)."), 'n'))
        .opt(text(
            "cursor",
            "Opaque cursor from a previous page (meta.next_cursor).",
        ))
        .opt(num("offset", "Row offset (prefer --cursor)."))
}

// ---- runtime ----

/// What a handler sees.
pub struct Ctx {
    pub args: Value,
    pub opts: Value,
    /// Called over MCP (no TTY, no prompts).
    pub mcp: bool,
}

impl Ctx {
    fn get(&self, k: &str) -> Option<&Value> {
        self.args
            .get(k)
            .or_else(|| self.opts.get(k))
            .filter(|v| !v.is_null())
    }
    pub fn s(&self, k: &str) -> Result<String> {
        self.os(k)
            .ok_or_else(|| NbError::usage(format!("missing {}", k.replace('_', "-"))))
    }
    pub fn os(&self, k: &str) -> Option<String> {
        self.get(k).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    }
    pub fn b(&self, k: &str) -> bool {
        self.ob(k).unwrap_or(false)
    }
    pub fn ob(&self, k: &str) -> Option<bool> {
        self.get(k).and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            Value::String(s) => Some(matches!(s.as_str(), "true" | "1" | "yes")),
            _ => None,
        })
    }
    pub fn of(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
    }
    pub fn f(&self, k: &str) -> Result<f64> {
        self.of(k)
            .ok_or_else(|| NbError::usage(format!("missing {}", k.replace('_', "-"))))
    }
    pub fn ou(&self, k: &str) -> Result<Option<usize>> {
        match self.of(k) {
            None => Ok(None),
            Some(f) if f >= 0.0 && f.fract() == 0.0 => Ok(Some(f as usize)),
            Some(f) => Err(NbError::usage(format!(
                "{} must be a non-negative integer, got {f}",
                k.replace('_', "-")
            ))),
        }
    }
    pub fn u(&self, k: &str) -> Result<usize> {
        self.ou(k)?
            .ok_or_else(|| NbError::usage(format!("missing {}", k.replace('_', "-"))))
    }
    pub fn list(&self, k: &str) -> Vec<String> {
        match self.get(k) {
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                .collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }
    /// Entity ids, space- or comma-separated.
    pub fn ids(&self, k: &str) -> Vec<String> {
        self.list(k)
            .iter()
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
    /// A signed-in client from the stored session.
    pub fn client(&self) -> Result<Client> {
        let creds = nb_api::auth::require_credentials()?;
        Ok(Client::new(nb_api::Http::new(creds.token)?))
    }
}

/// Paging state from `--limit/--cursor/--offset`.
pub struct Paging {
    pub offset: usize,
    pub limit: usize,
}

impl Paging {
    pub fn from(ctx: &Ctx, snapshot: &str) -> Result<Self> {
        let limit = ctx.ou("limit")?.unwrap_or(50);
        let offset = match ctx.os("cursor") {
            Some(c) => crate::cursor::decode(&c, snapshot)?,
            None => ctx.ou("offset")?.unwrap_or(0),
        };
        Ok(Self { offset, limit })
    }

    /// Paginated result: `{items, page}` plus a `next_cursor` lifted into meta.
    pub fn apply(&self, rows: Vec<Value>, snapshot: &str) -> Value {
        let total = rows.len();
        let take = if self.limit == 0 { total } else { self.limit };
        let items: Vec<Value> = rows.into_iter().skip(self.offset).take(take).collect();
        let end = self.offset + items.len();
        let next = (end < total).then_some(end);
        json!({
            "items": items,
            "page": {"total": total, "offset": self.offset, "limit": self.limit,
                     "returned": end - self.offset.min(end), "next_offset": next},
            "next_cursor": next.map(|n| crate::cursor::encode(n, snapshot)),
        })
    }

    /// A page fetched server-side at `offset`; a full page implies there may be more.
    pub fn server_page(&self, rows: Vec<Value>, snapshot: &str) -> Value {
        let n = rows.len();
        let next = (self.limit > 0 && n == self.limit).then_some(self.offset + n);
        json!({
            "items": rows,
            "page": {"offset": self.offset, "limit": self.limit, "returned": n, "next_offset": next},
            "next_cursor": next.map(|o| crate::cursor::encode(o, snapshot)),
        })
    }
}

struct DynHandler {
    run: Run,
    destructive: bool,
    path: &'static str,
}

fn err_result(e: &NbError) -> CommandResult {
    CommandResult::Error {
        code: match e.key.as_deref() {
            Some(k @ crate::cursor::STALE_CURSOR) => k.to_string(),
            _ => e.kind.code().to_uppercase(),
        },
        message: e.message.clone(),
        retryable: matches!(e.kind, nb_api::Kind::RateLimited),
        exit_code: Some(e.exit_code()),
        cta: auth_cta(e),
    }
}

fn auth_cta(e: &NbError) -> Option<CtaBlock> {
    (e.kind == nb_api::Kind::Auth).then(|| cta("Sign in first:", "account login"))
}

fn cta(description: &str, command: &str) -> CtaBlock {
    CtaBlock {
        commands: vec![CtaEntry::Simple(command.to_string())],
        description: Some(description.into()),
    }
}

/// Asks on the terminal; `Ok(false)` when declined.
fn prompt(path: &str) -> bool {
    use std::io::Write;
    eprint!("{path}: this is destructive. Proceed? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

#[async_trait]
impl CommandHandler for DynHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let mcp = ctx.mcp.is_some();
        let c = Ctx {
            args: ctx.args,
            opts: ctx.options,
            mcp,
        };
        if let Some(cur) = c.os("cursor")
            && let Err(e) = crate::cursor::parse(&cur)
        {
            return err_result(&e);
        }
        if self.destructive && !c.b("confirm") && !c.b("dry_run") {
            if mcp {
                let preview = json!({"dry_run": true, "command": self.path, "args": c.args,
                    "options": c.opts, "hint": "destructive: call again with confirm=true to apply"});
                return CommandResult::Ok {
                    data: preview,
                    cta: None,
                    exit_code: None,
                };
            }
            let tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
            let machine = ctx.format_explicit && ctx.format == Format::Json;
            if !tty || machine {
                let path = self.path;
                return CommandResult::Error {
                    code: "CONFIRM_REQUIRED".into(),
                    message: format!("{path} is destructive; re-run with --yes to confirm"),
                    retryable: false,
                    exit_code: Some(2),
                    cta: Some(cta(
                        "Confirm with:",
                        &format!("{path} <same arguments> --yes"),
                    )),
                };
            }
            if !prompt(self.path) {
                return err_result(&NbError::usage("aborted"));
            }
        }
        match (self.run)(c).await {
            Ok(data) => CommandResult::Ok {
                data,
                cta: None,
                exit_code: None,
            },
            Err(e) => err_result(&e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Ctx;
    use serde_json::json;

    #[test]
    fn ids_accept_spaces_and_commas() {
        let c = Ctx {
            args: json!({"ids": ["0:1,0:2", " 0:3 ", ""]}),
            opts: json!({}),
            mcp: false,
        };
        assert_eq!(c.ids("ids"), ["0:1", "0:2", "0:3"]);
    }
}
