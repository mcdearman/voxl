//! Talks to a running game over its debug connection.
//!
//! ```text
//! voxl-debug status
//! voxl-debug entities with=voxl.Camera
//! voxl-debug get entity=4294967297 component=voxl.Transform
//! voxl-debug set entity=4294967297 component=voxl.Transform path=translation.1 value=3.5
//! voxl-debug signals
//! voxl-debug signal_force name=blue.contesting value=false
//! voxl-debug --at 127.0.0.1:7878 pause
//! voxl-debug watch                     # the signal graph, redrawn as the game runs
//! voxl-debug watch every=0.5
//! ```
//!
//! The first word is the command and the rest are its arguments. A value that reads as JSON
//! (a number, `true`, `[1, 2, 3]`, `{"a": 1}`) is sent as that, and anything else as text.
//! The game is found at `--at`, else at `VOXL_DEBUG`, else at 127.0.0.1:7878.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    process::ExitCode,
    time::Duration,
};

use voxl::reflect::{json, Value};

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut address = std::env::var("VOXL_DEBUG").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
    if args.first().is_some_and(|arg| arg == "--at") && args.len() >= 2 {
        address = args[1].clone();
        args.drain(..2);
    }
    let Some(command) = args.first() else {
        eprintln!("usage: voxl-debug [--at address] <command> [name=value ...]");
        return ExitCode::from(2);
    };
    if command == "watch" {
        return watch(&address, &args[1..]);
    }
    let mut request = vec![("cmd".to_owned(), Value::Text(command.clone()))];
    for arg in &args[1..] {
        let Some((name, value)) = arg.split_once('=') else {
            eprintln!("`{arg}` is not name=value");
            return ExitCode::from(2);
        };
        let value = json::parse(value).unwrap_or_else(|_| Value::Text(value.to_owned()));
        request.push((name.to_owned(), value));
    }

    let answer = TcpStream::connect(&address).and_then(|mut stream| {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(json::to_line(&Value::Map(request)).as_bytes())?;
        stream.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    });
    let line = match answer {
        Ok(line) => line,
        Err(err) => {
            eprintln!("no answer from a game at {address}: {err}");
            return ExitCode::FAILURE;
        }
    };
    match json::parse(&line) {
        Ok(answer) => match (answer.field("ok"), answer.field("error")) {
            (Some(value), _) => {
                print!("{}", json::to_string(value));
                ExitCode::SUCCESS
            }
            (None, Some(Value::Text(why))) => {
                eprintln!("{why}");
                ExitCode::FAILURE
            }
            _ => {
                print!("{line}");
                ExitCode::FAILURE
            }
        },
        Err(_) => {
            print!("{line}");
            ExitCode::FAILURE
        }
    }
}

fn ask(address: &str, request: &str) -> std::io::Result<Value> {
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let answer = json::parse(&line).map_err(|err| std::io::Error::other(err.to_string()))?;
    answer
        .field("ok")
        .cloned()
        .ok_or_else(|| std::io::Error::other(format!("refused: {line}")))
}

/// Shows the signal graph and redraws it until interrupted: which signals are true, which
/// just changed, what feeds what.
fn watch(address: &str, args: &[String]) -> ExitCode {
    let every = args
        .iter()
        .find_map(|arg| arg.strip_prefix("every=")?.parse::<f64>().ok())
        .unwrap_or(0.2);
    loop {
        let drawn = ask(address, r#"{"cmd": "status"}"#).and_then(|status| {
            let graph = ask(address, r#"{"cmd": "signals"}"#)?;
            let number = |key: &str| status.field(key).and_then(Value::as_f64).unwrap_or(0.0);
            let paused = matches!(status.field("paused"), Some(Value::Bool(true)));
            Ok(format!(
                "frame {}  {:.1}s  {}  failures: {}\n\n{}",
                number("frame"),
                number("seconds"),
                if paused { "PAUSED" } else { "running" },
                number("failures"),
                voxl::remote::signals_text(&graph),
            ))
        });
        match drawn {
            // Home the cursor, draw, and clear what is left of the last drawing.
            Ok(drawn) => print!("\x1b[H{}\x1b[J", drawn.replace('\n', "\x1b[K\n")),
            Err(err) => {
                eprintln!("no answer from a game at {address}: {err}");
                return ExitCode::FAILURE;
            }
        }
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs_f64(every.max(0.02)));
    }
}
