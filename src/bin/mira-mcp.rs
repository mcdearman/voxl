//! An MCP server for a running mira game: lets an AI agent see the scene and read and change
//! the game's state. An MCP client starts this and talks to it over standard input and
//! output; this talks to the game over its debug connection.
//!
//! ```sh
//! MIRA_DEBUG=127.0.0.1:7878 cargo run --example host -- chase     # the game
//! claude mcp add mira -- cargo run --quiet --bin mira-mcp          # tell an agent about it
//! ```
//!
//! The game is found at `--at address`, else at `MIRA_DEBUG`, else at 127.0.0.1:7878. It
//! doesn't have to be running when this starts: each tool call connects afresh, and says so
//! if nothing answers. See `docs/MCP.md`.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use mira::{
    mcp::{handle_message, Game},
    reflect::{json, Value},
};

struct Remote {
    address: String,
    shots: u32,
    /// A game this server started, and where what it prints is kept.
    child: Option<(std::process::Child, PathBuf)>,
}

impl Remote {
    /// Stops the game this server launched, if it is still going.
    fn stop_child(&mut self) {
        if let Some((mut child, _)) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // A game launched for an agent doesn't outlive the agent's connection.
        self.stop_child();
    }
}

impl Game for Remote {
    fn ask(&mut self, request: &Value) -> Result<Value, String> {
        let exchange = || -> std::io::Result<String> {
            let mut stream = TcpStream::connect(&self.address)?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            stream.write_all(json::to_line(request).as_bytes())?;
            stream.write_all(b"\n")?;
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line)?;
            Ok(line)
        };
        let line = exchange().map_err(|err| {
            format!(
                "no game answered at {}: {err}. Start one with MIRA_DEBUG={} set.",
                self.address, self.address
            )
        })?;
        let answer =
            json::parse(&line).map_err(|err| format!("the game's answer was not JSON: {err}"))?;
        match (answer.field("ok"), answer.field("error")) {
            (Some(value), _) => Ok(value.clone()),
            (None, Some(Value::Text(why))) => Err(why.clone()),
            _ => Err(format!("the game answered {line:?}")),
        }
    }

    fn launch(&mut self, command: &str, folder: Option<&str>) -> Result<String, String> {
        self.stop_child();
        // A port nothing else has: ask the system for one, then give it up to the game.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|err| format!("can't find a free port: {err}"))?
            .port();
        let address = format!("127.0.0.1:{port}");
        let log = std::env::temp_dir().join(format!("mira-mcp-{}-game.log", std::process::id()));
        let out = std::fs::File::create(&log)
            .map_err(|err| format!("can't write {}: {err}", log.display()))?;
        let err = out.try_clone().map_err(|err| err.to_string())?;
        let mut shell = std::process::Command::new("sh");
        shell
            .arg("-c")
            // `exec`, so that stopping the child stops the game and not only its shell.
            .arg(format!("exec {command}"))
            .env("MIRA_DEBUG", &address)
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err);
        if let Some(folder) = folder {
            shell.current_dir(folder);
        }
        let child = shell
            .spawn()
            .map_err(|err| format!("can't run `{command}`: {err}"))?;
        self.child = Some((child, log.clone()));
        self.address = address.clone();
        // Building and loading can take a while; the game is ready when it answers.
        let started = Instant::now();
        let status = Value::Map(vec![("cmd".to_owned(), Value::Text("status".to_owned()))]);
        loop {
            if self.ask(&status).is_ok() {
                return Ok(format!(
                    "the game is running and listening on {address}; what it prints is in {}",
                    log.display()
                ));
            }
            let (child, _) = self.child.as_mut().expect("set just above");
            if let Ok(Some(code)) = child.try_wait() {
                let tail = self.log(30).unwrap_or_default();
                self.child = None;
                return Err(format!(
                    "the game stopped before it answered ({code}). It printed:\n{tail}"
                ));
            }
            if started.elapsed() > Duration::from_secs(900) {
                self.stop_child();
                return Err(
                    "the game didn't answer within fifteen minutes; it has been stopped".to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn log(&mut self, lines: usize) -> Result<String, String> {
        let (_, log) = self
            .child
            .as_ref()
            .ok_or("no game was launched from here, so there is no log")?;
        let text = std::fs::read_to_string(log)
            .map_err(|err| format!("can't read {}: {err}", log.display()))?;
        let all: Vec<&str> = text.lines().collect();
        Ok(all[all.len().saturating_sub(lines)..].join("\n"))
    }

    fn stopped(&mut self) {
        // Give a game this server launched a moment to go by itself, then make sure.
        let Some((child, _)) = self.child.as_mut() else {
            return;
        };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.child = None;
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.stop_child();
    }

    fn screenshot(&mut self, width: u32) -> Result<Vec<u8>, String> {
        self.shots += 1;
        let path: PathBuf = std::env::temp_dir().join(format!(
            "mira-mcp-{}-{}.png",
            std::process::id(),
            self.shots
        ));
        let _ = std::fs::remove_file(&path);
        let request = Value::Map(vec![
            ("cmd".to_owned(), Value::Text("screenshot".to_owned())),
            ("path".to_owned(), Value::Text(path.display().to_string())),
        ]);
        self.ask(&request)?;
        // The game saves the frame after the one it is on; wait for the whole file.
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
            let size = std::fs::metadata(&path).ok().map(|meta| meta.len());
            if size.is_some_and(|size| size > 0) && size == last {
                let frame =
                    image::open(&path).map_err(|err| format!("can't read the frame: {err}"));
                let _ = std::fs::remove_file(&path);
                return shrink(frame?, width);
            }
            last = size;
        }
        Err(
            "the game didn't draw a frame in ten seconds (is its window hidden or minimised?)"
                .to_owned(),
        )
    }
}

/// A full-size frame is megabytes, most of it more detail than whoever asked can use.
fn shrink(frame: image::DynamicImage, width: u32) -> Result<Vec<u8>, String> {
    let frame = if frame.width() > width {
        let height = (frame.height() as u64 * width as u64 / frame.width() as u64).max(1) as u32;
        frame.resize_exact(width, height, image::imageops::FilterType::Triangle)
    } else {
        frame
    };
    let mut png = std::io::Cursor::new(Vec::new());
    frame
        .to_rgb8()
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|err| format!("can't encode the frame: {err}"))?;
    Ok(png.into_inner())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut address = std::env::var("MIRA_DEBUG").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
    while let Some(arg) = args.next() {
        if arg == "--at" {
            address = args.next().unwrap_or(address);
        }
    }
    let mut game = Remote {
        address,
        shots: 0,
        child: None,
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = handle_message(&line, &mut game) {
            // Standard output carries the protocol and nothing else.
            if writeln!(out, "{answer}")
                .and_then(|()| out.flush())
                .is_err()
            {
                break;
            }
        }
    }
}
