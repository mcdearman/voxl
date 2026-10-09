//! An MCP server for a running voxl game: lets an AI agent see the scene and read and change
//! the game's state. An MCP client starts this and talks to it over standard input and
//! output; this talks to the game over its debug connection.
//!
//! ```sh
//! VOXL_DEBUG=127.0.0.1:7878 cargo run --example host -- chase     # the game
//! claude mcp add voxl -- cargo run --quiet --bin voxl-mcp          # tell an agent about it
//! ```
//!
//! The game is found at `--at address`, else at `VOXL_DEBUG`, else at 127.0.0.1:7878. It
//! doesn't have to be running when this starts: each tool call connects afresh, and says so
//! if nothing answers. See `docs/MCP.md`.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use voxl::{
    mcp::{handle_message, Game},
    reflect::{json, Value},
};

struct Remote {
    address: String,
    shots: u32,
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
                "no game answered at {}: {err}. Start one with VOXL_DEBUG={} set.",
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

    fn screenshot(&mut self, width: u32) -> Result<Vec<u8>, String> {
        self.shots += 1;
        let path: PathBuf = std::env::temp_dir().join(format!(
            "voxl-mcp-{}-{}.png",
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
    let mut address = std::env::var("VOXL_DEBUG").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
    while let Some(arg) = args.next() {
        if arg == "--at" {
            address = args.next().unwrap_or(address);
        }
    }
    let mut game = Remote { address, shots: 0 };
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
