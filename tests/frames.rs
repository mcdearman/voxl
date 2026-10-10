//! Pictures of fixed scenes, compared with the frames stored in `tests/frames`.
//!
//! Each scene is run as its own program with the window hidden, started paused and stepped
//! a set number of frames, so it is at the same moment every time; the frame drawn there is
//! compared with the stored one (`mira::render::frame_diff`).
//!
//! These need a graphics card, so they only run when asked:
//!
//! ```sh
//! MIRA_FRAME_TESTS=1 cargo test --test frames
//! MIRA_FRAME_TESTS=1 MIRA_UPDATE_FRAMES=1 cargo test --test frames   # store new frames
//! ```

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use mira::{
    reflect::{json, Value},
    render::frame_diff::{check_frame, Allowed},
};

/// A scene running as its own program, held still and told what to do over the debug
/// connection.
struct Scene {
    program: Child,
    address: String,
}

impl Scene {
    /// Starts one of the examples, hidden and paused at its first moment.
    fn start(example: &str) -> Scene {
        // Examples are built beside the tests' own programs, but not by asking for this
        // test alone: build the one wanted, so that what is drawn is the code as it is now.
        // This test's own program is in `deps`, beside the folder the examples are built into.
        let here = std::env::current_exe().expect("this program has a path");
        let beside = here.parent().expect("it is in a folder");
        let mut build = Command::new(env!("CARGO"));
        build
            .args(["build", "--quiet", "--example", example])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if beside
            .components()
            .any(|part| part.as_os_str() == "release")
        {
            build.arg("--release");
        }
        assert!(
            build.status().is_ok_and(|status| status.success()),
            "{example} did not build"
        );
        let program = beside.with_file_name("examples").join(example);
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let address = format!("127.0.0.1:{port}");
        let child = Command::new(&program)
            .env("MIRA_DEBUG", &address)
            .env("MIRA_HIDDEN", "1")
            .env("MIRA_PAUSED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|err| panic!("can't run {}: {err}", program.display()));
        let scene = Scene {
            program: child,
            address,
        };
        let started = Instant::now();
        while scene.ask(r#"{"cmd": "status"}"#).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "{example} did not start"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        scene
    }

    fn ask(&self, request: &str) -> Result<Value, String> {
        let mut stream = TcpStream::connect(&self.address).map_err(|err| err.to_string())?;
        writeln!(stream, "{request}").map_err(|err| err.to_string())?;
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|err| err.to_string())?;
        let answer = json::parse(&line).map_err(|err| err.to_string())?;
        match (answer.field("ok"), answer.field("error")) {
            (Some(ok), _) => Ok(ok.clone()),
            (_, Some(Value::Text(error))) => Err(error.clone()),
            _ => Err(format!("an answer that is neither: {line}")),
        }
    }

    /// Steps the scene on by this many frames and returns the frame drawn there.
    fn frame_after(&self, frames: u32, into: &Path) -> image::RgbaImage {
        self.ask(&format!(r#"{{"cmd": "step", "frames": {frames}}}"#))
            .expect("stepping");
        let started = Instant::now();
        loop {
            let status = self.ask(r#"{"cmd": "status"}"#).expect("status");
            if status.field("stepping") == Some(&Value::Int(0)) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "still stepping"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        // A few more frames drawn of the same moment, for what is built up over frames
        // (anti-aliasing, shadows fading in) to settle.
        std::thread::sleep(Duration::from_millis(500));
        let _ = std::fs::remove_file(into);
        // A request is one line; a path written as Rust writes a string is one JSON reads.
        let request = format!(
            r#"{{"cmd": "screenshot", "path": {:?}}}"#,
            into.to_string_lossy()
        );
        self.ask(&request).expect("a screenshot");
        let started = Instant::now();
        loop {
            // The file appears a frame later, and is whole a moment after that.
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(frame) = image::open(into) {
                return frame.to_rgba8();
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no frame was saved to {}",
                into.display()
            );
        }
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = self.ask(r#"{"cmd": "quit"}"#);
        let _ = self.program.kill();
        let _ = self.program.wait();
    }
}

/// Whether these tests were asked for. Without a graphics card there is nothing to draw
/// with, so they don't run by themselves.
fn asked_for() -> bool {
    let asked = std::env::var("MIRA_FRAME_TESTS").is_ok_and(|asked| asked != "0");
    if !asked {
        eprintln!("skipped: set MIRA_FRAME_TESTS=1 on a machine with a graphics card");
    }
    asked
}

fn stored(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/frames")
        .join(format!("{name}.png"))
}

#[test]
fn the_sacred_sites_look_as_they_did() {
    if !asked_for() {
        return;
    }
    let scene = Scene::start("sacred_sites");
    let scratch = std::env::temp_dir().join(format!("mira-frame-{}.png", std::process::id()));
    // The opening, and five seconds on, when the scouts have walked.
    for (name, frames) in [("sacred_sites_start", 30), ("sacred_sites_later", 300)] {
        let frame = scene.frame_after(frames, &scratch);
        // The scene is a few small things on a wide field, so little may differ: a unit
        // gone, or a site the wrong colour, is a few dozen pixels of the small frame.
        let allowed = Allowed {
            differing: 0.0005,
            ..Allowed::default()
        };
        if let Err(why) = check_frame(&frame, &stored(name), allowed) {
            panic!("{name}: {why}");
        }
    }
    let _ = std::fs::remove_file(scratch);
}
