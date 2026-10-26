//! Learn chat: thread reads and one streamed (SSE) user turn with the web client's retry rule.

use std::time::Duration;

use futures_util::future::BoxFuture;
use reqwest::Method;
use serde_json::{Value, json};

use crate::config::{AUTH_COOKIE, ENDPOINTS};
use crate::error::{Kind, NbError, Result};
use crate::graphql::QUOTA_KEYS;
use crate::http::{Http, Req, Resp};

/// Parse a `text/event-stream` body into `(event, data)`; data is JSON when it parses.
pub fn parse_sse(body: &str) -> Vec<(String, Value)> {
    body.replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(parse_frame)
        .collect()
}

fn parse_frame(frame: &str) -> Option<(String, Value)> {
    let mut event = "message".to_string();
    let mut data = Vec::new();
    for line in frame.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        match k {
            "event" => event = v.to_string(),
            "data" => data.push(v),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    let raw = data.join("\n");
    Some((
        event,
        serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
    ))
}

fn raise_chat(r: &Resp) -> Result<()> {
    if r.status < 400 {
        return Ok(());
    }
    let key = serde_json::from_slice::<Value>(&r.body)
        .ok()
        .and_then(|v| v["key"].as_str().map(str::to_string));
    if let Some(k) = key.as_deref().filter(|k| QUOTA_KEYS.contains(k)) {
        return Err(NbError::quota(format!("AI chat unavailable: {k}"))
            .with_key(Some(k))
            .with_status(r.status));
    }
    let shown = key.clone().unwrap_or_else(|| "no key".into());
    Err(NbError::server(format!("chat HTTP {} ({shown})", r.status))
        .with_key(key)
        .with_status(r.status))
}

/// One streamed turn, folded.
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    /// `exchange_complete` arrived (or an error after some text).
    Done {
        text: String,
        errors: Vec<Value>,
        complete: Value,
    },
    /// The assistant asked the client to run a tool (`tool_call` ends the stream; web
    /// ChatSSEStream's `suspended` outcome). Answer with a `tool_result` turn at index + 1.
    Tool { text: String, call: Value },
    /// The stream ended without a terminal event.
    Broken,
}

/// Fold one turn's SSE events.
pub fn fold_turn(events: &[(String, Value)]) -> Result<Turn> {
    let (mut reply, mut call, mut errors, mut complete) = (String::new(), None, vec![], None);
    for (ev, data) in events {
        match ev.as_str() {
            "text" => reply.push_str(data.as_str().unwrap_or_default()),
            "tool_call" => call = Some(data.clone()),
            "error" => errors.push(data.clone()),
            "exchange_complete" => complete = Some(data.clone()),
            _ => {}
        }
    }
    if !errors.is_empty() && reply.is_empty() {
        let e0 = if errors[0].is_object() {
            errors[0].clone()
        } else {
            json!({"message": errors[0].to_string()})
        };
        let code = e0["code"].as_str().map(str::to_string);
        let msg = e0["message"].as_str().map(str::to_string);
        if let Some(c) = code.as_deref().filter(|c| QUOTA_KEYS.contains(c)) {
            return Err(NbError::quota(msg.unwrap_or_else(|| c.to_string())).with_key(Some(c)));
        }
        return Err(NbError::server(format!(
            "chat error: {}",
            msg.unwrap_or_else(|| e0.to_string())
        ))
        .with_key(code));
    }
    // Web ChatSSEStream: `tool_call` always suspends the turn, even if `exchange_complete` follows.
    if let Some(call) = call {
        return Ok(Turn::Tool { text: reply, call });
    }
    if complete.is_none() && errors.is_empty() {
        return Ok(Turn::Broken);
    }
    Ok(Turn::Done {
        text: reply,
        errors,
        complete: complete.unwrap_or_else(|| json!({})),
    })
}

/// Answers a client tool call: `(tool, args) -> (status, data)`, status being one of the web
/// client's `SUCCESS`, `ERROR`, `TOOL_NOT_AVAILABLE`, `TOOL_DENIED`.
pub type ToolResolver<'a> =
    &'a (dyn Fn(String, Value) -> BoxFuture<'a, (String, Value)> + Send + Sync);

/// Client tool calls answered per `chat ask` (the web client has no cap; this guards loops).
pub const MAX_TOOL_ROUNDS: usize = 4;

/// An assistant `tool_call` the thread still waits on (no `tool_result` after it).
fn pending_tool_call(read: &Value) -> Option<Value> {
    let msgs = read["messages"].as_array()?;
    let last = msgs.last()?;
    if last["role"] != "assistant" {
        return None;
    }
    last["content"]
        .as_array()?
        .iter()
        .rev()
        .find(|c| c["type"] == "tool_call")
        .cloned()
}

fn last_assistant_text(read: &Value) -> String {
    for m in read["messages"].as_array().into_iter().flatten().rev() {
        if m["role"] == "assistant" {
            return match &m["content"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter(|p| p["type"] == "text")
                    .filter_map(|p| p["text"].as_str())
                    .collect(),
                _ => String::new(),
            };
        }
    }
    String::new()
}

impl Http {
    pub async fn chat_thread(&self, thread_id: &str) -> Result<Value> {
        let r = self
            .send(
                Req::new(
                    Method::POST,
                    format!("{}/learn/chatbotThread/{thread_id}", ENDPOINTS.api),
                )
                .header("content-type", "application/json"),
            )
            .await?;
        raise_chat(&r)?;
        r.json()
    }

    /// `POST /collab-api/rag/recent-notes-backfill` (no AI quota; the web app fires it on chat open).
    pub async fn chat_backfill(&self) -> Result<Value> {
        let r = self
            .send(Req::new(
                Method::POST,
                format!("{}/rag/recent-notes-backfill", ENDPOINTS.collab),
            ))
            .await?;
        if r.status >= 400 {
            return Err(
                NbError::server(format!("recent-notes backfill HTTP {}", r.status))
                    .with_status(r.status),
            );
        }
        Ok(json!({"status": r.status, "ok": true}))
    }

    async fn chat_turn(&self, thread_id: &str, body: Value) -> Result<Turn> {
        let req = Req::new(
            Method::POST,
            format!("{}/learn/chatbotThread/{thread_id}", ENDPOINTS.api),
        )
        .header(AUTH_COOKIE, self.token())
        .header("accept", "text/event-stream")
        .json(body)
        .timeout(Duration::from_secs(180))
        .once();
        let r = match self.send(req).await {
            Ok(r) => r,
            Err(e) if e.kind == Kind::Server && e.status.is_none() => return Ok(Turn::Broken),
            Err(e) => return Err(e),
        };
        raise_chat(&r)?;
        let raw = r.text();
        let turn = fold_turn(&parse_sse(&raw))?;
        // An empty completed turn may still have parked a client tool call in the thread.
        if let Turn::Done { text, errors, .. } = &turn
            && text.trim().is_empty()
            && errors.is_empty()
            && let Ok(read) = self.chat_thread(thread_id).await
            && let Some(call) = pending_tool_call(&read)
        {
            return Ok(Turn::Tool {
                text: String::new(),
                call,
            });
        }
        Ok(turn)
    }

    /// Send one turn body at `idx`. A broken stream re-reads the thread: a committed turn is
    /// adopted, otherwise it is replayed once at the corrected index (web ChatManager
    /// `_classifyAndReplay`).
    async fn turn_with_replay(&self, tid: &str, idx: &mut u64, body: &Value) -> Result<Turn> {
        for attempt in 0..2 {
            let mut b = body.clone();
            b["exchangeIndex"] = json!(*idx);
            let t = self.chat_turn(tid, b).await?;
            if t != Turn::Broken || attempt == 1 {
                return Ok(t);
            }
            if *idx > 0 {
                let read = self.chat_thread(tid).await?;
                let next = read["nextExchangeIndex"].as_u64().unwrap_or(0);
                if next > *idx {
                    return Ok(Turn::Done {
                        text: last_assistant_text(&read),
                        errors: vec![],
                        complete: json!({"nextExchangeIndex": next, "recovered": "adopted-committed-turn"}),
                    });
                }
                *idx = next;
            }
        }
        Ok(Turn::Broken)
    }

    /// One user turn, answering client tool calls through `resolver` (tools in `tools` are
    /// advertised in `clientInfo.availableTools`; anything else gets `TOOL_NOT_AVAILABLE`).
    pub async fn chat_ask(
        &self,
        text: &str,
        thread_id: Option<&str>,
        note_id: Option<&str>,
        tools: &[&str],
        resolver: Option<ToolResolver<'_>>,
    ) -> Result<Value> {
        let (tid, mut idx) = match thread_id {
            Some(t) => (
                t.to_string(),
                self.chat_thread(t).await?["nextExchangeIndex"]
                    .as_u64()
                    .unwrap_or(0),
            ),
            None => (uuid::Uuid::new_v4().to_string(), 0),
        };
        let entry = match note_id {
            Some(n) => json!({"location": "note", "noteId": n}),
            None => json!({"location": "home"}),
        };
        let first = idx;
        let mut body = json!({"role": "user", "text": text,
            "clientContext": {"entryPoint": entry, "clientInfo": {"platform": "web", "availableTools": tools}}});
        let (mut reply, mut calls) = (String::new(), Vec::new());
        loop {
            match self.turn_with_replay(&tid, &mut idx, &body).await? {
                Turn::Broken => {
                    return Err(NbError::server(
                        "chat stream interrupted: no exchange_complete event",
                    ));
                }
                Turn::Done {
                    text,
                    errors,
                    complete,
                } => {
                    reply.push_str(&text);
                    let mut out = json!({
                        "thread_id": tid, "exchange_index": first, "text": reply, "tool_calls": calls,
                        "errors": errors,
                        "next_exchange_index": complete.get("nextExchangeIndex").cloned().unwrap_or(Value::Null),
                        "title": complete.get("title").cloned().unwrap_or(Value::Null),
                    });
                    if let Some(r) = complete.get("recovered") {
                        out["recovered"] = r.clone();
                    }
                    return Ok(out);
                }
                Turn::Tool { text, call } => {
                    reply.push_str(&text);
                    let tool = call["tool"].as_str().unwrap_or_default().to_string();
                    let call_id = call["toolCallId"].clone();
                    let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                    let (status, data) = match resolver {
                        _ if calls.len() >= MAX_TOOL_ROUNDS => (
                            "TOOL_NOT_AVAILABLE".to_string(),
                            json!({"message": "Tool call limit reached for this question. Answer from what you have."}),
                        ),
                        Some(r) if tools.contains(&tool.as_str()) => {
                            r(tool.clone(), args.clone()).await
                        }
                        _ => (
                            "TOOL_NOT_AVAILABLE".to_string(),
                            json!({"message": format!("{tool} is not available in this client. Answer from search results instead.")}),
                        ),
                    };
                    calls.push(json!({"tool": tool, "tool_call_id": call_id, "args": args, "status": status}));
                    idx += 1;
                    body = json!({"role": "tool_result", "toolCallId": call_id, "status": status, "data": data});
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_stream_folds_to_reply() {
        let cap = include_str!("../tests/fixtures/chat-sse.txt");
        let body: String = cap.lines().skip(2).map(|l| format!("{l}\n")).collect();
        let evs = parse_sse(&body);
        let Turn::Done { text, complete, .. } = fold_turn(&evs).unwrap() else {
            panic!("not done")
        };
        assert_eq!(text, "pong");
        assert_eq!(complete["nextExchangeIndex"], 1);
        assert_eq!(complete["title"], "Pong Response");
    }

    #[test]
    fn missing_completion_is_interrupted_and_quota_errors_map() {
        assert_eq!(
            fold_turn(&parse_sse("event: text\ndata: \"a\"\n\n")).unwrap(),
            Turn::Broken
        );
        let tool = fold_turn(&parse_sse(
            "event: text\ndata: \"Let me look.\"\n\nevent: tool_call\ndata: {\"toolCallId\":\"c1\",\"tool\":\"note_read\",\"args\":{}}\n\n",
        ))
        .unwrap();
        assert_eq!(
            tool,
            Turn::Tool {
                text: "Let me look.".into(),
                call: json!({"toolCallId": "c1", "tool": "note_read", "args": {}})
            }
        );
        let e = fold_turn(&parse_sse(
            "event: error\ndata: {\"code\":\"aiDisabled\",\"message\":\"no\"}\n\n",
        ))
        .unwrap_err();
        assert_eq!(e.kind, Kind::Quota);
        assert_eq!(
            parse_sse(": c\nevent: x\ndata: raw\ndata: two\n\n"),
            vec![("x".into(), json!("raw\ntwo"))]
        );
    }

    #[test]
    fn tool_call_suspends_even_with_completion() {
        let t = fold_turn(&parse_sse(
            "event: tool_call\ndata: {\"toolCallId\":\"c2\",\"tool\":\"note_read\",\"args\":{}}\n\nevent: exchange_complete\ndata: {\"nextExchangeIndex\":1}\n\n",
        ))
        .unwrap();
        assert!(matches!(t, Turn::Tool { ref call, .. } if call["toolCallId"] == "c2"));
        let read = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "q"}]},
            {"role": "assistant", "content": [{"type": "tool_call", "toolCallId": "c3", "tool": "note_read", "args": {}}]}
        ]});
        assert_eq!(pending_tool_call(&read).unwrap()["toolCallId"], "c3");
        let done = json!({"messages": [{"role": "assistant", "content": [{"type": "text", "text": "a"}]}]});
        assert!(pending_tool_call(&done).is_none());
    }
}
