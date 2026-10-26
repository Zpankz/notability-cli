//! Local recorder (`audio record start|stop|status`). Notability has no server-side recording
//! session: the web client records with `MediaRecorder` and commits the file like an imported
//! one. Here ffmpeg captures (device, or `--input-file` streamed in real time) to AAC m4a in a
//! detached process; `stop` sends SIGINT so ffmpeg finalises the `moov` atom, then uploads.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use nb_api::{NbError, Result};
use serde_json::{Value, json};

use crate::client::{Client, now_ms};
use crate::media::{PreparedAudio, add_recording, mp4_duration_ms};

/// The child when this process started it (an MCP server), so `stop` can reap it.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

pub fn state_dir() -> PathBuf {
    nb_api::auth::config_dir().join("recordings")
}

fn state_file() -> PathBuf {
    state_dir().join("active.json")
}

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

fn reap(pid: u32) -> bool {
    let mut g = CHILD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(c) = g.as_mut().filter(|c| c.id() == pid) {
        return match c.try_wait() {
            Ok(Some(_)) | Err(_) => {
                *g = None;
                true
            }
            Ok(None) => false,
        };
    }
    false
}

pub fn alive(pid: u32) -> bool {
    if pid == 0 || reap(pid) {
        return false;
    }
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // state is the field after the parenthesised command name
        return stat
            .rsplit_once(')')
            .and_then(|(_, r)| r.split_whitespace().next())
            .is_some_and(|s| s != "Z" && s != "X");
    }
    if Path::new("/proc/self").exists() {
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn signal(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &pid.to_string()])
        .stderr(Stdio::null())
        .status();
}

fn read_state() -> Option<Value> {
    serde_json::from_slice(&std::fs::read(state_file()).ok()?).ok()
}

/// `device` is ffmpeg's `FORMAT:INPUT` (default `pulse:default` on Linux, `avfoundation::0` on
/// macOS); `input_file` is streamed with `-re` instead (tests, machines without a microphone).
pub fn start(note_id: &str, input_file: Option<&str>, device: Option<&str>) -> Result<Value> {
    if !has("ffmpeg") {
        return Err(NbError::usage("audio record needs ffmpeg on PATH"));
    }
    if let Some(cur) = read_state()
        && alive(cur["pid"].as_u64().unwrap_or(0) as u32)
    {
        return Err(NbError::usage(format!(
            "a recording is already running for note {} (run `audio record stop`)",
            cur["note_id"].as_str().unwrap_or("?")
        )));
    }
    let dir = state_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| NbError::generic(format!("{}: {e}", dir.display())))?;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let out = dir.join(format!("rec-{}-{tag}.m4a", now_ms() / 1000));
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y"]);
    let source = if let Some(f) = input_file {
        let p =
            std::fs::canonicalize(f).map_err(|_| NbError::usage(format!("no such file: {f}")))?;
        cmd.arg("-re").arg("-i").arg(&p);
        p.display().to_string()
    } else {
        let dev = device.map_or_else(
            || {
                if cfg!(target_os = "macos") {
                    "avfoundation::0".to_string()
                } else {
                    "pulse:default".to_string()
                }
            },
            str::to_string,
        );
        let (fmt, name) = dev.split_once(':').unwrap_or((dev.as_str(), "default"));
        cmd.args([
            "-f",
            fmt,
            "-i",
            if name.is_empty() { "default" } else { name },
        ]);
        dev.clone()
    };
    cmd.args([
        "-vn", "-ac", "1", "-c:a", "aac", "-b:a", "96k", "-f", "ipod",
    ])
    .arg(&out);
    let log_path = out.with_extension("log");
    let log = std::fs::File::create(&log_path).map_err(|e| NbError::generic(format!("{e}")))?;
    let log2 = log
        .try_clone()
        .map_err(|e| NbError::generic(format!("{e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2)
        .spawn()
        .map_err(|e| NbError::generic(format!("ffmpeg: {e}")))?;
    let pid = child.id();
    let state = json!({"note_id": note_id, "pid": pid, "file": out.display().to_string(),
                       "start_ms": now_ms(), "source": source});
    std::fs::write(state_file(), state.to_string())
        .map_err(|e| NbError::generic(format!("{e}")))?;
    std::thread::sleep(Duration::from_millis(500));
    if let Ok(Some(st)) = child.try_wait()
        && !st.success()
    {
        let _ = std::fs::remove_file(state_file());
        let msg: String = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        return Err(NbError::generic(format!("ffmpeg exited: {}", msg.trim())));
    }
    *CHILD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child);
    let mut s = state;
    s["status"] = json!("recording");
    Ok(s)
}

pub fn status() -> Value {
    let Some(mut s) = read_state() else {
        return json!({"active": false});
    };
    let pid = s["pid"].as_u64().unwrap_or(0) as u32;
    s["active"] = json!(true);
    s["running"] = json!(alive(pid));
    s["elapsed_ms"] = json!(now_ms().saturating_sub(s["start_ms"].as_u64().unwrap_or(0)));
    let f = s["file"].as_str().unwrap_or_default().to_string();
    s["bytes"] = json!(std::fs::metadata(&f).map_or(0, |m| m.len()));
    s
}

pub async fn stop(
    client: &Client,
    transcribe: bool,
    keep_file: bool,
    discard: bool,
) -> Result<Value> {
    let s = read_state()
        .ok_or_else(|| NbError::usage("no active recording (run `audio record start`)"))?;
    let pid = s["pid"].as_u64().unwrap_or(0) as u32;
    if alive(pid) {
        signal(pid, "-INT");
        let mut gone = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !alive(pid) {
                gone = true;
                break;
            }
        }
        if !gone {
            signal(pid, "-KILL");
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = alive(pid);
        }
    }
    let _ = std::fs::remove_file(state_file());
    let f = PathBuf::from(s["file"].as_str().unwrap_or_default());
    let _ = std::fs::remove_file(f.with_extension("log"));
    let note = s["note_id"].as_str().unwrap_or_default().to_string();
    if discard {
        let _ = std::fs::remove_file(&f);
        return Ok(json!({"note_id": note, "discarded": true}));
    }
    let data = std::fs::read(&f).unwrap_or_default();
    if data.is_empty() {
        return Err(NbError::generic("the recording produced no audio"));
    }
    let start_ms = s["start_ms"].as_u64();
    let duration_ms =
        mp4_duration_ms(&data).unwrap_or_else(|| now_ms().saturating_sub(start_ms.unwrap_or(0)));
    let audio = PreparedAudio {
        data,
        file_name: format!("{}.m4a", uuid::Uuid::new_v4()),
        mime: "audio/x-m4a",
        duration_ms,
        converted: false,
    };
    let res = add_recording(client, &note, audio, start_ms, None, transcribe, true).await;
    if !keep_file {
        let _ = std::fs::remove_file(&f);
    }
    let mut r = res?;
    if keep_file {
        r["local_file"] = json!(f.display().to_string());
    }
    Ok(r)
}
