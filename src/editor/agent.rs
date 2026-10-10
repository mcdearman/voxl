//! The agent the app talks to: a program of its own, which the window asks and listens to.
//!
//! The window knows only this much of an agent: it can be asked something, it says things
//! back over a while, and it can be told to stop. What the agent is, and how it reaches the
//! game (through mira's own tools, served for the game that is running in the window), is
//! behind [`Agent`].

use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{mpsc::Sender, Arc, Mutex},
};

use crate::reflect::{json, Value};

/// What an agent says back while it works on what it was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// More of its answer, to go on the end of what it has said so far.
    Text(String),
    /// It has begun to use a tool: a name for this use of it, to tell the uses apart, and
    /// the tool's name.
    Tool { id: String, name: String },
    /// More of what it is giving the tool, as it writes it.
    Given { id: String, more: String },
    /// What came back from the tool: words, whether it failed, and a picture if it sent
    /// one (width, height, and four bytes a pixel).
    Back {
        id: String,
        text: String,
        failed: bool,
        picture: Option<(u32, u32, Vec<u8>)>,
    },
    /// It has finished.
    Done,
    /// It could not go on, and why.
    Failed(String),
}

/// A program that can be asked things.
pub trait Agent {
    /// Starts it on what was asked. What it says is sent to `heard` as it says it, ending
    /// with [`Heard::Done`] or [`Heard::Failed`].
    fn ask(&mut self, asked: &str, heard: Sender<Heard>);

    /// Stops what it is doing, if it is doing anything.
    fn stop(&mut self);
}

/// No agent: says how to have one.
pub struct NoAgent;

impl Agent for NoAgent {
    fn ask(&mut self, _asked: &str, heard: Sender<Heard>) {
        let _ = heard.send(Heard::Failed(
            "There is no agent to ask. Start the app with one (see docs/EDITOR.md).".to_owned(),
        ));
    }

    fn stop(&mut self) {}
}

/// Claude Code, run as a program of its own for each thing asked, with the game's tools.
///
/// Each question starts `claude` in its print mode, reading its answer as it streams; the
/// next question resumes the same conversation. It is given one set of tools besides its
/// own reading of files: mira's (`mira_*`, through `mira-mcp`), attached to the game running
/// in the window, and allowed without asking. Anything else that would need a yes, such as
/// changing a file or running a command, is refused, since the window has nowhere yet to
/// ask for one.
pub struct ClaudeCode {
    /// The program to run: `MIRA_AGENT`, else `~/.local/bin/claude`, else `claude`.
    program: PathBuf,
    /// `mira-mcp`, and where the game it is to attach to is listening.
    tools: PathBuf,
    game: String,
    /// The conversation so far, by the name Claude Code gave it.
    session: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<Option<Child>>>,
}

impl ClaudeCode {
    /// An agent for the game listening at `game`, reaching it through the `mira-mcp` at
    /// `tools`.
    pub fn new(tools: impl Into<PathBuf>, game: impl Into<String>) -> Self {
        let installed = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".local/bin/claude"))
            .filter(|path| path.exists());
        let program = std::env::var_os("MIRA_AGENT")
            .map(PathBuf::from)
            .or(installed)
            .unwrap_or_else(|| PathBuf::from("claude"));
        Self {
            program,
            tools: tools.into(),
            game: game.into(),
            session: Arc::default(),
            running: Arc::default(),
        }
    }

    /// What `claude` is run with for one question.
    fn arguments(&self, asked: &str, session: Option<&str>) -> Vec<String> {
        // Written as Rust writes strings, which is how JSON writes them too.
        let servers = format!(
            r#"{{"mcpServers": {{"mira": {{"type": "stdio", "command": {:?}, "args": ["--at", {:?}]}}}}}}"#,
            self.tools.to_string_lossy(),
            self.game,
        );
        let mut arguments: Vec<String> = [
            "-p",
            asked,
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--mcp-config",
            &servers,
            "--strict-mcp-config",
            "--allowedTools",
            "mcp__mira",
        ]
        .map(str::to_owned)
        .into();
        if let Some(session) = session {
            arguments.extend(["--resume".to_owned(), session.to_owned()]);
        }
        arguments
    }
}

/// What has been made of one answer's lines so far.
#[derive(Default)]
struct Reading {
    /// Whether any of the answer has come as it was written. If none has by the end, the
    /// whole answer is taken from the last line instead.
    streamed: bool,
    ended: bool,
    /// The tool it is giving something to just now.
    tool: Option<String>,
}

/// The bytes that base64 text stands for, or nothing if it isn't base64.
fn from_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    let (mut held, mut bits) = (0u32, 0);
    for letter in text.bytes().filter(|letter| !letter.is_ascii_whitespace()) {
        let six = match letter {
            b'A'..=b'Z' => letter - b'A',
            b'a'..=b'z' => letter - b'a' + 26,
            b'0'..=b'9' => letter - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        held = held << 6 | six as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((held >> bits) as u8);
        }
    }
    Some(bytes)
}

/// What a tool sent back, out of the line that carries it: every `tool_result` in it.
fn results(line: &Value) -> Vec<Heard> {
    let Some(Value::List(blocks)) = line.get_path("message.content") else {
        return Vec::new();
    };
    let text_of = |value: &Value, path: &str| match value.get_path(path) {
        Some(Value::Text(text)) => Some(text.clone()),
        _ => None,
    };
    blocks
        .iter()
        .filter(|block| text_of(block, "type").as_deref() == Some("tool_result"))
        .filter_map(|block| {
            let (mut text, mut picture) = (String::new(), None);
            match block.get_path("content") {
                Some(Value::Text(whole)) => text = whole.clone(),
                Some(Value::List(parts)) => {
                    for part in parts {
                        match text_of(part, "type").as_deref() {
                            Some("text") => text += &text_of(part, "text").unwrap_or_default(),
                            Some("image") => {
                                picture = text_of(part, "source.data")
                                    .and_then(|data| from_base64(&data))
                                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                                    .map(|picture| {
                                        let picture = picture.to_rgba8();
                                        (picture.width(), picture.height(), picture.into_raw())
                                    });
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            Some(Heard::Back {
                id: text_of(block, "tool_use_id")?,
                text,
                failed: block.get_path("is_error") == Some(&Value::Bool(true)),
                picture,
            })
        })
        .collect()
}

/// What one line of Claude Code's streamed output says, and the conversation's name if the
/// line carries it.
fn read_line(line: &str, reading: &mut Reading) -> (Vec<Heard>, Option<String>) {
    let Ok(line) = json::parse(line) else {
        return (Vec::new(), None);
    };
    let text = |path: &str| match line.get_path(path) {
        Some(Value::Text(text)) => Some(text.as_str()),
        _ => None,
    };
    let session = text("session_id").map(str::to_owned);
    let mut heard = Vec::new();
    match (text("type"), text("event.type")) {
        (Some("stream_event"), Some("content_block_delta")) => {
            match (text("event.delta.type"), &reading.tool) {
                (Some("text_delta"), _) => {
                    if let Some(more) = text("event.delta.text") {
                        reading.streamed = true;
                        heard.push(Heard::Text(more.to_owned()));
                    }
                }
                (Some("input_json_delta"), Some(tool)) => {
                    if let Some(more) = text("event.delta.partial_json") {
                        heard.push(Heard::Given {
                            id: tool.clone(),
                            more: more.to_owned(),
                        });
                    }
                }
                _ => {}
            }
        }
        (Some("stream_event"), Some("content_block_start")) => {
            reading.tool = None;
            if let (Some("tool_use"), Some(id), Some(name)) = (
                text("event.content_block.type"),
                text("event.content_block.id"),
                text("event.content_block.name"),
            ) {
                // mira's own tools by their own names.
                let name = name.strip_prefix("mcp__mira__").unwrap_or(name);
                reading.tool = Some(id.to_owned());
                heard.push(Heard::Tool {
                    id: id.to_owned(),
                    name: name.to_owned(),
                });
            }
        }
        (Some("user"), _) => heard.extend(results(&line)),
        (Some("result"), _) => {
            reading.ended = true;
            let failed = line.get_path("is_error") == Some(&Value::Bool(true))
                || text("subtype").is_some_and(|subtype| subtype != "success");
            if failed {
                let why = match line.get_path("errors.0") {
                    Some(Value::Text(why)) => why.clone(),
                    _ => text("result")
                        .or(text("subtype"))
                        .unwrap_or("it did not say why")
                        .to_owned(),
                };
                heard.push(Heard::Failed(format!("The agent could not finish: {why}")));
            } else {
                if let (false, Some(whole)) = (reading.streamed, text("result")) {
                    heard.push(Heard::Text(whole.to_owned()));
                }
                heard.push(Heard::Done);
            }
        }
        _ => {}
    }
    (heard, session)
}

impl Agent for ClaudeCode {
    fn ask(&mut self, asked: &str, heard: Sender<Heard>) {
        self.stop();
        let session = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let child = Command::new(&self.program)
            .args(self.arguments(asked, session.as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                let _ = heard.send(Heard::Failed(format!(
                    "Can't run the agent ({}): {err}. Install Claude Code, or set MIRA_AGENT to where it is.",
                    self.program.display()
                )));
                return;
            }
        };
        let (Some(output), Some(errors)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = heard.send(Heard::Failed("Can't hear the agent.".to_owned()));
            return;
        };
        *self.running.lock().unwrap_or_else(|p| p.into_inner()) = Some(child);
        let (session, running) = (self.session.clone(), self.running.clone());
        std::thread::spawn(move || {
            let mut reading = Reading::default();
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                let (said, named) = read_line(&line, &mut reading);
                if let Some(named) = named {
                    *session.lock().unwrap_or_else(|p| p.into_inner()) = Some(named);
                }
                for said in said {
                    if heard.send(said).is_err() {
                        return;
                    }
                }
            }
            // It went away without a last word: stopped from here, or fell over.
            let finished = running.lock().unwrap_or_else(|p| p.into_inner()).take();
            let stopped = finished.is_none();
            if let Some(mut finished) = finished {
                let _ = finished.wait();
            }
            if !reading.ended && !stopped {
                let mut why = String::new();
                let _ = BufReader::new(errors).read_to_string(&mut why);
                let why = why.lines().last().unwrap_or("it gave no reason").to_owned();
                let _ = heard.send(Heard::Failed(format!("The agent stopped: {why}")));
            }
        });
    }

    fn stop(&mut self) {
        if let Some(mut child) = self
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ClaudeCode {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(lines: &[&str]) -> (Vec<Heard>, Option<String>) {
        let mut reading = Reading::default();
        let (mut heard, mut session) = (Vec::new(), None);
        for line in lines {
            let (said, named) = read_line(line, &mut reading);
            heard.extend(said);
            session = named.or(session);
        }
        (heard, session)
    }

    #[test]
    fn an_answer_is_read_from_the_lines_as_they_come() {
        let (heard, session) = all(&[
            r#"{"type":"system","subtype":"init","session_id":"abc","tools":[]}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Blue is "}},"session_id":"abc"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"on a site."}},"session_id":"abc"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"t1","name":"mcp__mira__mira_signals","input":{}}},"session_id":"abc"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{}"}},"session_id":"abc"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"door.open = true"}]}}"#,
            // The same words again, whole: already heard.
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Blue is on a site."}]},"session_id":"abc"}"#,
            "not a line of it at all",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Blue is on a site.","session_id":"abc","total_cost_usd":0.01}"#,
        ]);
        assert_eq!(
            heard,
            [
                Heard::Text("Blue is ".into()),
                Heard::Text("on a site.".into()),
                Heard::Tool {
                    id: "t1".into(),
                    name: "mira_signals".into()
                },
                Heard::Given {
                    id: "t1".into(),
                    more: "{}".into()
                },
                Heard::Back {
                    id: "t1".into(),
                    text: "door.open = true".into(),
                    failed: false,
                    picture: None
                },
                Heard::Done,
            ]
        );
        assert_eq!(session.as_deref(), Some("abc"));

        // What a tool sends back may be parts: words, and a picture as base64.
        let png = {
            let mut bytes = Vec::new();
            let dot = image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255]));
            image::DynamicImage::ImageRgba8(dot)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        let encoded = {
            const LETTERS: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut text = String::new();
            for chunk in png.chunks(3) {
                let held = chunk
                    .iter()
                    .fold(0u32, |held, byte| held << 8 | *byte as u32)
                    << (8 * (3 - chunk.len()));
                for place in 0..4 {
                    if place <= chunk.len() {
                        text.push(LETTERS[(held >> (18 - 6 * place) & 63) as usize] as char);
                    } else {
                        text.push('=');
                    }
                }
            }
            text
        };
        assert_eq!(from_base64(&encoded), Some(png));
        assert_eq!(from_base64("not base64!"), None);
        let line = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":[{{"type":"text","text":"the frame"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{encoded}"}}}}]}}]}}}}"#
        );
        let (heard, _) = all(&[&line]);
        assert_eq!(
            heard,
            [Heard::Back {
                id: "t2".into(),
                text: "the frame".into(),
                failed: true,
                picture: Some((2, 1, vec![255, 0, 0, 255, 255, 0, 0, 255]))
            }]
        );

        // Nothing streamed: the answer is taken whole from the end.
        let (heard, _) =
            all(&[r#"{"type":"result","subtype":"success","is_error":false,"result":"Done."}"#]);
        assert_eq!(heard, [Heard::Text("Done.".into()), Heard::Done]);

        // It could not finish, and says why.
        let (heard, _) = all(&[
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"errors":["too many turns"]}"#,
        ]);
        assert_eq!(
            heard,
            [Heard::Failed(
                "The agent could not finish: too many turns".into()
            )]
        );
    }

    #[test]
    fn it_is_run_with_the_games_tools_and_goes_on_with_the_same_conversation() {
        let agent = ClaudeCode::new("/opt/mira/mira-mcp", "127.0.0.1:7878");
        let first = agent.arguments("What is on screen?", None);
        assert_eq!(first[..2], ["-p", "What is on screen?"]);
        let servers = &first[first.iter().position(|arg| arg == "--mcp-config").unwrap() + 1];
        let servers = json::parse(servers).expect("the servers are JSON");
        assert_eq!(
            servers.get_path("mcpServers.mira.command"),
            Some(&Value::Text("/opt/mira/mira-mcp".into()))
        );
        assert_eq!(
            servers.get_path("mcpServers.mira.args.1"),
            Some(&Value::Text("127.0.0.1:7878".into()))
        );
        assert!(first.contains(&"mcp__mira".to_owned()) && !first.contains(&"--resume".to_owned()));
        let next = agent.arguments("And now?", Some("abc"));
        assert_eq!(next[next.len() - 2..], ["--resume", "abc"]);
    }

    /// A stand-in for the agent's program: a script that says what the real one would.
    #[cfg(unix)]
    #[test]
    fn a_program_is_run_heard_and_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let folder = std::env::temp_dir().join(format!("mira-agent-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("agent.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in *--resume*) said="again" ;; *) said="first" ;; esac
echo '{"type":"system","subtype":"init","session_id":"s-1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"'"$said"'"}}}'
case "$2" in *wait*) sleep 30 ;; *fail*) echo "it broke" >&2; exit 3 ;; esac
echo '{"type":"result","subtype":"success","is_error":false,"result":"x","session_id":"s-1"}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut agent = ClaudeCode::new("mira-mcp", "127.0.0.1:1");
        agent.program = script;
        let ask = |agent: &mut ClaudeCode, asked: &str, wait_for: usize| {
            let (send, heard) = std::sync::mpsc::channel();
            agent.ask(asked, send);
            let patience = std::time::Duration::from_secs(10);
            let heard: Vec<Heard> = (0..wait_for)
                .map_while(|_| heard.recv_timeout(patience).ok())
                .collect();
            heard
        };
        assert_eq!(
            ask(&mut agent, "hello", 2),
            [Heard::Text("first".into()), Heard::Done]
        );
        // The next question goes on with the conversation the first one named.
        assert_eq!(
            ask(&mut agent, "more", 2),
            [Heard::Text("again".into()), Heard::Done]
        );
        // One that falls over says what it said last.
        assert_eq!(
            ask(&mut agent, "fail", 2),
            [
                Heard::Text("again".into()),
                Heard::Failed("The agent stopped: it broke".into())
            ]
        );
        // One that is taking its time is stopped, and says nothing more.
        assert_eq!(ask(&mut agent, "wait", 1), [Heard::Text("again".into())]);
        let started = std::time::Instant::now();
        agent.stop();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(agent.running.lock().unwrap().is_none());
        let _ = std::fs::remove_dir_all(folder);
    }
}
