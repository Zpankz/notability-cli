//! Live transcription: ffmpeg encodes the microphone (or a file) to fragmented MP4 (mono AAC,
//! 48 kHz, one-second fragments, like the web recorder's `MediaRecorder`), the fragments are
//! split into recorder chunks (chunk 0 carries `ftyp`+`moov`), and every PUT carries all chunks
//! the server has not acknowledged yet, as the web app's chunk upload buffer does.

use std::path::PathBuf;
use std::process::Stdio;

use nb_api::live::LiveReply;
use nb_api::{NbError, Result};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;

use crate::client::Client;

/// Incremental ISO-BMFF splitter: feed bytes, get complete `moof`+`mdat` chunks back.
#[derive(Default)]
pub struct Fragmenter {
    buf: Vec<u8>,
    head: Vec<u8>,
    cur: Vec<u8>,
    emitted: usize,
}

impl Fragmenter {
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 8 {
                break;
            }
            let mut size =
                u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as u64;
            if size == 1 {
                if self.buf.len() < 16 {
                    break;
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.buf[8..16]);
                size = u64::from_be_bytes(b);
            }
            if size == 0 {
                break; // runs to end of stream; handled by `finish`
            }
            if size < 8 {
                return Err(NbError::generic("malformed MP4 box from ffmpeg"));
            }
            let size = size as usize;
            if self.buf.len() < size {
                break;
            }
            let bx: Vec<u8> = self.buf.drain(..size).collect();
            self.take(&bx, &mut out);
        }
        Ok(out)
    }

    fn take(&mut self, bx: &[u8], out: &mut Vec<Vec<u8>>) {
        let kind = &bx[4..8];
        if kind == b"moof" {
            self.cur = bx.to_vec();
        } else if kind == b"mdat" && !self.cur.is_empty() {
            let mut chunk = std::mem::take(&mut self.head);
            chunk.append(&mut self.cur);
            chunk.extend_from_slice(bx);
            out.push(chunk);
            self.emitted += 1;
        } else if self.emitted == 0 && self.cur.is_empty() {
            self.head.extend_from_slice(bx);
        } else if !self.cur.is_empty() {
            self.cur.extend_from_slice(bx);
        }
    }

    /// Any final chunk at end of stream (a trailing size-0 box or an unterminated fragment).
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let rest = std::mem::take(&mut self.buf);
        let mut out = Vec::new();
        if rest.len() >= 8 {
            self.take(&rest, &mut out);
        }
        out
    }
}

/// Chunks appended vs acknowledged, as the server reports `lastAudioChunkIndexConsumed`.
#[derive(Default)]
pub struct ChunkBuffer {
    pending: Vec<Vec<u8>>,
    pub appended: u64,
    pub acked: u64,
}

impl ChunkBuffer {
    pub fn append(&mut self, c: Vec<u8>) {
        if !c.is_empty() {
            self.pending.push(c);
            self.appended += 1;
        }
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    pub fn body(&self) -> Vec<u8> {
        self.pending.concat()
    }
    pub fn ack(&mut self, r: &LiveReply) {
        let consumed = match (r.status, r.consumed) {
            (_, Some(c)) => c,
            (204, None) => self.appended,
            _ => return,
        };
        let n = consumed.min(self.appended).saturating_sub(self.acked) as usize;
        if n > 0 && n <= self.pending.len() {
            self.pending.drain(..n);
            self.acked += n as u64;
        }
    }
}

/// Where live audio comes from.
pub enum LiveSource {
    File { path: PathBuf, realtime: bool },
    Device(String),
}

pub fn default_device() -> String {
    if cfg!(target_os = "macos") {
        "avfoundation::0".into()
    } else {
        "pulse:default".into()
    }
}

/// The ffmpeg arguments that turn the source into recorder-style fragmented MP4 on stdout.
pub fn ffmpeg_args(src: &LiveSource, seconds: Option<f64>) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-loglevel", "error"]
        .map(String::from)
        .to_vec();
    match src {
        LiveSource::File { path, realtime } => {
            if *realtime {
                a.push("-re".into());
            }
            a.extend(["-i".into(), path.display().to_string()]);
        }
        LiveSource::Device(dev) => {
            let (fmt, name) = dev.split_once(':').unwrap_or((dev.as_str(), "default"));
            a.extend([
                "-f".into(),
                fmt.into(),
                "-i".into(),
                if name.is_empty() {
                    "default".into()
                } else {
                    name.into()
                },
            ]);
        }
    }
    if let Some(s) = seconds {
        a.extend(["-t".into(), format!("{s}")]);
    }
    a.extend(
        [
            "-vn",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "-frag_duration",
            "1000000",
            "-f",
            "mp4",
            "pipe:1",
        ]
        .map(String::from),
    );
    a
}

pub struct LiveOptions {
    pub language: String,
    pub seconds: Option<f64>,
    pub chunks_per_put: usize,
    pub save: Option<PathBuf>,
    pub session: Option<String>,
    pub finalize: bool,
}

impl Client {
    /// Stream audio through a live-transcription session (spends transcription quota).
    pub async fn live_transcribe(
        &self,
        note_id: &str,
        src: LiveSource,
        o: LiveOptions,
        mut on_text: impl FnMut(&str),
    ) -> Result<Value> {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return Err(NbError::usage("live transcription needs ffmpeg on PATH"));
        }
        if let LiveSource::File { path, .. } = &src
            && !path.is_file()
        {
            return Err(NbError::usage(format!("no such file: {}", path.display())));
        }
        if matches!(src, LiveSource::Device(_)) && o.seconds.is_none() {
            return Err(NbError::usage(
                "pass --seconds when recording from a device",
            ));
        }
        let session = o
            .session
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut child = tokio::process::Command::new("ffmpeg")
            .args(ffmpeg_args(&src, o.seconds))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| NbError::usage(format!("cannot start ffmpeg: {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| NbError::generic("ffmpeg has no stdout"))?;
        let mut frag = Fragmenter::default();
        let mut buf = ChunkBuffer::default();
        let mut saved: Vec<u8> = Vec::new();
        let (mut text, mut smart) = (String::new(), String::new());
        let (mut puts, mut since) = (0usize, 0usize);
        let per = o.chunks_per_put.max(1);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut eof = false;
        while !eof {
            let n = stdout
                .read(&mut chunk)
                .await
                .map_err(|e| NbError::generic(format!("reading ffmpeg output: {e}")))?;
            let mut ready = if n == 0 {
                eof = true;
                frag.finish()
            } else {
                frag.push(&chunk[..n])?
            };
            if o.save.is_some() {
                saved.extend_from_slice(&chunk[..n]);
            }
            since += ready.len();
            for c in ready.drain(..) {
                buf.append(c);
            }
            if !buf.is_empty() && (since >= per || eof) {
                since = 0;
                let r = self
                    .http
                    .live_put(&session, note_id, &o.language, buf.body(), buf.appended)
                    .await?;
                puts += 1;
                buf.ack(&r);
                if let Some(t) = r.transcription_text {
                    on_text(&t);
                    text = t;
                }
                if let Some(s) = r.smart_notes_text {
                    smart = s;
                }
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|e| NbError::generic(format!("ffmpeg: {e}")))?;
        if !status.success() && buf.appended == 0 {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err).await;
            }
            return Err(NbError::usage(format!(
                "ffmpeg failed: {}",
                err.trim().chars().take(200).collect::<String>()
            )));
        }
        if buf.appended == 0 {
            return Err(NbError::usage("ffmpeg produced no audio"));
        }
        let mut out = json!({"note_id": note_id, "session_id": session, "chunks": buf.appended,
                             "acked": buf.acked, "puts": puts,
                             "transcription_text": text, "smart_notes_text": smart});
        if let Some(p) = &o.save {
            std::fs::write(p, &saved)
                .map_err(|e| NbError::usage(format!("cannot write {}: {e}", p.display())))?;
            out["saved"] = json!(p.display().to_string());
        }
        if o.finalize {
            out["finalized"] = self.http.live_finalize(&session, &o.language).await?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn splits_fragments_incrementally() {
        let stream = [
            bx(b"ftyp", b"iso6"),
            bx(b"moov", b"m"),
            bx(b"moof", b"1"),
            bx(b"mdat", b"aa"),
            bx(b"moof", b"2"),
            bx(b"mdat", b"bb"),
        ]
        .concat();
        let mut f = Fragmenter::default();
        let mut chunks = Vec::new();
        for piece in stream.chunks(5) {
            chunks.extend(f.push(piece).unwrap());
        }
        chunks.extend(f.finish());
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with(&bx(b"ftyp", b"iso6")));
        assert_eq!(chunks[1], [bx(b"moof", b"2"), bx(b"mdat", b"bb")].concat());
        assert!(
            Fragmenter::default()
                .push(&[0, 0, 0, 4, b'b', b'a', b'd', b'x'])
                .is_err()
        );
    }

    #[test]
    fn buffer_resends_until_acked() {
        let mut b = ChunkBuffer::default();
        b.append(vec![1]);
        b.append(vec![2]);
        b.ack(&LiveReply {
            status: 200,
            consumed: Some(1),
            ..Default::default()
        });
        assert_eq!((b.acked, b.body()), (1, vec![2]));
        b.append(vec![3]);
        assert_eq!(b.body(), vec![2, 3]);
        b.ack(&LiveReply {
            status: 204,
            ..Default::default()
        });
        assert!(b.is_empty());
        assert_eq!(b.acked, 3);
    }

    #[test]
    fn ffmpeg_args_shape() {
        let a = ffmpeg_args(&LiveSource::Device("pulse:".into()), Some(5.0));
        assert!(a.windows(2).any(|w| w == ["-i", "default"]));
        assert!(a.windows(2).any(|w| w == ["-t", "5"]));
        assert_eq!(a.last().map(String::as_str), Some("pipe:1"));
        let f = ffmpeg_args(
            &LiveSource::File {
                path: "x.wav".into(),
                realtime: true,
            },
            None,
        );
        assert!(f.contains(&"-re".to_string()));
    }
}
