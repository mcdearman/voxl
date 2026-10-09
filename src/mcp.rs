//! The Model Context Protocol face of the engine: what lets an AI agent drive a running game
//! as a developer would, seeing the scene and reading and changing all of its state.
//!
//! An MCP client (an agent's host) starts `mira-mcp` and speaks JSON-RPC to it over standard
//! input and output, one message per line. Every tool is one command of the
//! [debug connection](crate::remote), which `mira-mcp` reaches over the game's local socket;
//! `screenshot` also reads the saved frame back and returns it as an image.
//!
//! This module is the protocol with the transport left out, so it can be tested without
//! processes or sockets: [`handle_message`] takes a line and a way to ask the game.

use crate::reflect::{json, Value};

/// The protocol revision this speaks.
const PROTOCOL: &str = "2024-11-05";

/// A tool: a debug command, what it is for, and its arguments as
/// `name: type: description`, with `?` after the name of an optional one.
struct Tool {
    command: &'static str,
    about: &'static str,
    arguments: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        command: "status",
        about: "The game at a glance: frame, seconds of game time, time scale, whether it is paused, how many systems have failed, how many entities there are. Start here.",
        arguments: &[],
    },
    Tool {
        command: "screenshot",
        about: "See the scene: returns the game's next rendered frame as an image. Works while paused. Fails for a game with no window.",
        arguments: &["width?: integer: how many pixels wide the image should be (default 1024; the frame is scaled down to it, never up)"],
    },
    Tool {
        command: "entities",
        about: "Lists entities with the names of the components each has. Entities are numbers; pass them to the other tools.",
        arguments: &[
            "with?: string: only entities with this component, e.g. mira.Camera",
            "limit?: integer: how many to list (default 200); `total` in the answer says how many there are",
        ],
    },
    Tool {
        command: "get",
        about: "Reads an entity's components: one by name, or all of them.",
        arguments: &[
            "entity: integer: the entity's number",
            "component?: string: a component name; leave out for every component",
        ],
    },
    Tool {
        command: "set",
        about: "Changes a component on an entity, or adds it. With `path`, sets one field (e.g. translation.1) and leaves the rest; without, `value` is the whole component. Use `schema` to learn a component's shape.",
        arguments: &[
            "entity: integer: the entity's number",
            "component: string: the component's name",
            "value: any: the new value",
            "path?: string: a field inside the component, parts joined by dots",
        ],
    },
    Tool {
        command: "remove",
        about: "Takes a component off an entity.",
        arguments: &["entity: integer: the entity's number", "component: string: the component's name"],
    },
    Tool {
        command: "spawn",
        about: "Makes an entity with the given components. Returns its number.",
        arguments: &["components: object: component values by component name"],
    },
    Tool {
        command: "despawn",
        about: "Removes an entity and everything below it in the hierarchy. Returns how many entities went.",
        arguments: &["entity: integer: the entity's number"],
    },
    Tool {
        command: "resource",
        about: "Reads a registered resource (a game-wide setting such as mira.Fog), or sets it when `value` is given.",
        arguments: &["name: string: the resource's name", "value?: any: a new value for it"],
    },
    Tool {
        command: "types",
        about: "The names of every component and resource that can be read and written by name.",
        arguments: &[],
    },
    Tool {
        command: "schema",
        about: "The shape of a component or resource: its fields and their types, as needed to write a value of it.",
        arguments: &["name: string: a component or resource name"],
    },
    Tool {
        command: "systems",
        about: "The game's systems by stage, in the order they run: what each reads and writes, its ordering constraints, how long it takes, whether it is suspended after a failure, and which systems could run in parallel (`batch`).",
        arguments: &["stage?: string: one stage only, e.g. Update"],
    },
    Tool {
        command: "failures",
        about: "Systems that have panicked or thrown, each with its message, source location and stack. A failure pauses the game; fix the code (plugins reload when saved) or call `resume`.",
        arguments: &[],
    },
    Tool {
        command: "pause",
        about: "Stops the simulation. Rendering, input, hot reload and these tools carry on.",
        arguments: &[],
    },
    Tool {
        command: "resume",
        about: "Carries on after a pause, and lets systems that failed run again.",
        arguments: &[],
    },
    Tool {
        command: "step",
        about: "Runs a number of frames of the simulation and pauses again. The frames run as the game's loop comes round, so read `status` afterwards to see where it has got to.",
        arguments: &["frames?: integer: how many frames (default 1)"],
    },
    Tool {
        command: "time_scale",
        about: "Slows the game down or speeds it up: 0.25 is quarter speed, 1 is normal.",
        arguments: &["scale: number: how fast game time runs against the clock"],
    },
    Tool {
        command: "record",
        about: "Turns the recording of snapshots on or off, which is what makes `rewind` possible. Each snapshot is a whole scene, so leave it off when not needed.",
        arguments: &[
            "on?: boolean: whether to record",
            "every?: integer: frames between snapshots (default 30)",
            "keep?: integer: how many snapshots to keep (default 240)",
        ],
    },
    Tool {
        command: "history",
        about: "The moments (frame and seconds) that `rewind` can go back to.",
        arguments: &[],
    },
    Tool {
        command: "rewind",
        about: "Steps the game back to an earlier snapshot and pauses it there. Later snapshots are forgotten. Needs `record` to have been on.",
        arguments: &[
            "frames?: integer: how many frames back (default 60)",
            "to_frame?: integer: or the frame to go back to",
        ],
    },
    Tool {
        command: "signals",
        about: "The signal graph: the game's rules as values derived from the world and from each other. Returns a drawing of it (what is true, what feeds what) and every node with its kind, inputs and value.",
        arguments: &[],
    },
    Tool {
        command: "signal_set",
        about: "Sets a constant signal, defining it if need be: a setting the rules read.",
        arguments: &["name: string: the signal's name", "value: any: true, false or a number"],
    },
    Tool {
        command: "signal_force",
        about: "Holds a signal's output at a value whatever it would work out, to try something; leave `value` out to let it go.",
        arguments: &["name: string: the signal's name", "value?: any: true, false or a number"],
    },
    Tool {
        command: "signal_connect",
        about: "Connects one input of a signal to a different signal.",
        arguments: &[
            "name: string: the signal whose input changes",
            "input: integer: which input, from 0",
            "to: string: the signal it should read",
        ],
    },
    Tool {
        command: "signal_define",
        about: "Defines a rule: a signal worked out from others, or replaces one. Operations: and, or, not, count, sum, select, timer, held_for (needs `seconds`), less, less_or_equal, equal, greater_or_equal, greater, constant (needs `value`).",
        arguments: &[
            "name: string: the signal's name",
            "op: string: the operation",
            "inputs?: array: the names of the signals it reads, in order",
            "seconds?: number: for held_for",
            "value?: any: for constant",
        ],
    },
    Tool {
        command: "signal_remove",
        about: "Removes a signal.",
        arguments: &["name: string: the signal's name"],
    },
    Tool {
        command: "reload_plugins",
        about: "Reloads every plugin whose file has changed, now. Returns how many reloaded.",
        arguments: &[],
    },
    Tool {
        command: "save_scene",
        about: "Saves every entity's registered components, and the registered resources, to a JSON scene file. Returns how many entities were saved.",
        arguments: &["path: string: where to write the file"],
    },
];

fn map(fields: Vec<(&str, Value)>) -> Value {
    Value::Map(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn text(text: impl Into<String>) -> Value {
    Value::Text(text.into())
}

/// A tool's arguments as a JSON Schema object.
fn input_schema(tool: &Tool) -> Value {
    let mut properties = Vec::new();
    let mut required = Vec::new();
    for argument in tool.arguments {
        let mut parts = argument.splitn(3, ": ");
        let (name, kind, about) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or("any"),
            parts.next().unwrap_or_default(),
        );
        let (name, optional) = match name.strip_suffix('?') {
            Some(name) => (name, true),
            None => (name, false),
        };
        let mut property = vec![("description", text(about))];
        // "any" is said by giving no type at all.
        if kind != "any" {
            property.insert(0, ("type", text(kind)));
        }
        properties.push((name.to_owned(), map(property)));
        if !optional {
            required.push(text(name));
        }
    }
    map(vec![
        ("type", text("object")),
        ("properties", Value::Map(properties)),
        ("required", Value::List(required)),
    ])
}

fn tool_list() -> Value {
    Value::List(
        TOOLS
            .iter()
            .map(|tool| {
                map(vec![
                    ("name", text(format!("mira_{}", tool.command))),
                    ("description", text(tool.about)),
                    ("inputSchema", input_schema(tool)),
                ])
            })
            .collect(),
    )
}

/// What a tool call gives back: text, and for a screenshot an image.
fn content(parts: Vec<Value>, failed: bool) -> Value {
    map(vec![
        ("content", Value::List(parts)),
        ("isError", Value::Bool(failed)),
    ])
}

fn words(words: impl Into<String>) -> Value {
    map(vec![("type", text("text")), ("text", text(words))])
}

/// Standard base64, for sending an image as text.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let bits = u32::from_be_bytes([0, group[0], group[1], group[2]]);
        for place in 0..4 {
            if place <= chunk.len() {
                out.push(ALPHABET[(bits >> (18 - 6 * place)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How `mira-mcp` reaches the game and the files it writes.
pub trait Game {
    /// Sends one debug command (an object with `cmd` and its arguments) and returns what the
    /// game answered under `ok`, or why it refused.
    fn ask(&mut self, request: &Value) -> Result<Value, String>;

    /// Asks the game to save its next frame, waits for the file, and returns it as a PNG no
    /// more than `width` pixels wide.
    fn screenshot(&mut self, width: u32) -> Result<Vec<u8>, String>;
}

fn call(game: &mut dyn Game, name: &str, arguments: Option<&Value>) -> Value {
    let Some(tool) = name
        .strip_prefix("mira_")
        .and_then(|command| TOOLS.iter().find(|tool| tool.command == command))
    else {
        return content(vec![words(format!("there is no tool `{name}`"))], true);
    };
    if tool.command == "screenshot" {
        let width = arguments
            .and_then(|arguments| arguments.field("width"))
            .and_then(Value::as_f64)
            .map_or(1024, |width| width.clamp(16.0, 8192.0) as u32);
        return match game.screenshot(width) {
            Ok(png) => content(
                vec![map(vec![
                    ("type", text("image")),
                    ("data", text(base64(&png))),
                    ("mimeType", text("image/png")),
                ])],
                false,
            ),
            Err(why) => content(vec![words(why)], true),
        };
    }
    let mut request = vec![("cmd".to_owned(), text(tool.command))];
    if let Some(Value::Map(arguments)) = arguments {
        request.extend(arguments.iter().filter(|(key, _)| key != "cmd").cloned());
    }
    match game.ask(&Value::Map(request)) {
        Ok(answer) => {
            let mut parts = Vec::new();
            // A graph is easier to take in drawn than listed.
            if tool.command == "signals" {
                parts.push(words(crate::remote::signals_text(&answer)));
            }
            parts.push(words(json::to_string(&answer)));
            content(parts, false)
        }
        Err(why) => content(vec![words(why)], true),
    }
}

/// Answers one JSON-RPC message from an MCP client. Returns the line to send back, or `None`
/// for a notification, which is not answered.
pub fn handle_message(line: &str, game: &mut dyn Game) -> Option<String> {
    let error = |id: Value, code: i64, message: String| {
        json::to_line(&map(vec![
            ("jsonrpc", text("2.0")),
            ("id", id),
            (
                "error",
                map(vec![("code", Value::Int(code)), ("message", text(message))]),
            ),
        ]))
    };
    let message = match json::parse(line) {
        Ok(message) => message,
        Err(err) => return Some(error(Value::Null, -32700, format!("not JSON: {err}"))),
    };
    let method = match message.field("method") {
        Some(Value::Text(method)) => method.as_str(),
        _ => {
            return Some(error(
                Value::Null,
                -32600,
                "a message needs a `method`".to_owned(),
            ))
        }
    };
    // No id: a notification. Nothing is sent back, whatever it was.
    let id = message.field("id")?.clone();
    let params = message.field("params");
    let result = match method {
        "initialize" => map(vec![
            ("protocolVersion", text(PROTOCOL)),
            ("capabilities", map(vec![("tools", Value::Map(Vec::new()))])),
            (
                "serverInfo",
                map(vec![("name", text("mira")), ("version", text(env!("CARGO_PKG_VERSION")))]),
            ),
            (
                "instructions",
                text("Tools to inspect and change a running mira game. Start with mira_status; mira_screenshot shows the scene; mira_types and mira_schema say what can be read and written; mira_signals shows the game's rules."),
            ),
        ]),
        "ping" => Value::Map(Vec::new()),
        "tools/list" => map(vec![("tools", tool_list())]),
        "tools/call" => {
            let name = match params.and_then(|params| params.field("name")) {
                Some(Value::Text(name)) => name.as_str(),
                _ => return Some(error(id, -32602, "tools/call needs a tool `name`".to_owned())),
            };
            call(game, name, params.and_then(|params| params.field("arguments")))
        }
        other => return Some(error(id, -32601, format!("there is no method `{other}`"))),
    };
    Some(json::to_line(&map(vec![
        ("jsonrpc", text("2.0")),
        ("id", id),
        ("result", result),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::App,
        remote::respond,
        signal::{Op, Signals},
        time::TimePlugin,
        transform::{Transform, TransformPlugin},
    };

    /// A game in this process, asked through the same function the socket uses.
    struct Local(App);

    impl Game for Local {
        fn ask(&mut self, request: &Value) -> Result<Value, String> {
            let answer = json::parse(&respond(&mut self.0, &json::to_line(request))).unwrap();
            match (answer.field("ok"), answer.field("error")) {
                (Some(value), _) => Ok(value.clone()),
                (None, Some(Value::Text(why))) => Err(why.clone()),
                _ => Err("no answer".to_owned()),
            }
        }

        fn screenshot(&mut self, width: u32) -> Result<Vec<u8>, String> {
            // No renderer here; stand in for the file the game would have written.
            self.ask(&map(vec![("cmd", text("status"))]))?;
            Ok(format!("\u{89}PNG fake {width}").into_bytes())
        }
    }

    fn game() -> Local {
        let mut app = App::new();
        app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
        let signals = app.world.resource_mut::<Signals>();
        signals.set("door.open", true);
        signals.define("door.shut", Op::Not, ["door.open"]);
        app.world.spawn(Transform::from_xyz(1.0, 2.0, 3.0));
        app.update();
        Local(app)
    }

    /// Sends a request and returns its `result`.
    fn rpc(game: &mut Local, method: &str, params: &str) -> Value {
        let line =
            format!(r#"{{"jsonrpc": "2.0", "id": 7, "method": "{method}", "params": {params}}}"#);
        let answer = json::parse(&handle_message(&line, game).expect("an answer")).unwrap();
        assert_eq!(answer.field("id"), Some(&Value::Int(7)));
        answer
            .field("result")
            .unwrap_or_else(|| panic!("{method} failed: {answer:?}"))
            .clone()
    }

    /// Calls a tool and returns its text parts and whether it failed.
    fn tool(game: &mut Local, name: &str, arguments: &str) -> (Vec<String>, bool) {
        let result = rpc(
            game,
            "tools/call",
            &format!(r#"{{"name": "{name}", "arguments": {arguments}}}"#),
        );
        let Some(Value::List(parts)) = result.field("content") else {
            panic!("content: {result:?}");
        };
        let texts = parts
            .iter()
            .filter_map(|part| match part.field("text") {
                Some(Value::Text(text)) => Some(text.clone()),
                _ => None,
            })
            .collect();
        (texts, result.field("isError") == Some(&Value::Bool(true)))
    }

    #[test]
    fn a_client_is_greeted_and_told_the_tools() {
        let mut game = game();
        let hello = rpc(
            &mut game,
            "initialize",
            r#"{"protocolVersion": "2024-11-05", "capabilities": {}}"#,
        );
        assert_eq!(hello.field("protocolVersion"), Some(&text(PROTOCOL)));
        assert!(hello.get_path("capabilities.tools").is_some());
        assert_eq!(hello.get_path("serverInfo.name"), Some(&text("mira")));
        // A notification gets no answer; nor does one this doesn't know.
        assert!(handle_message(
            r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#,
            &mut game
        )
        .is_none());
        assert!(handle_message(
            r#"{"jsonrpc": "2.0", "method": "notifications/whatever"}"#,
            &mut game
        )
        .is_none());
        assert_eq!(rpc(&mut game, "ping", "{}"), Value::Map(Vec::new()));

        let Some(Value::List(tools)) = rpc(&mut game, "tools/list", "{}").field("tools").cloned()
        else {
            panic!("a list of tools");
        };
        assert_eq!(tools.len(), TOOLS.len());
        let set = tools
            .iter()
            .find(|tool| tool.field("name") == Some(&text("mira_set")))
            .unwrap();
        assert_eq!(set.get_path("inputSchema.type"), Some(&text("object")));
        assert_eq!(
            set.get_path("inputSchema.properties.entity.type"),
            Some(&text("integer"))
        );
        assert!(
            set.get_path("inputSchema.properties.value.type").is_none(),
            "any value"
        );
        assert_eq!(
            set.get_path("inputSchema.required"),
            Some(&Value::List(vec![
                text("entity"),
                text("component"),
                text("value")
            ]))
        );
        // Every tool is a command the game knows: none is refused as unknown.
        for tool in TOOLS {
            let (texts, _) = self::tool(&mut game, &format!("mira_{}", tool.command), "{}");
            assert!(
                !texts.concat().contains("there is no command"),
                "{}",
                tool.command
            );
        }
    }

    #[test]
    fn tools_read_and_change_the_game() {
        let mut game = game();
        let (status, failed) = tool(&mut game, "mira_status", "{}");
        assert!(
            !failed && status[0].contains("\"entities\": 1"),
            "{status:?}"
        );

        let (listed, _) = tool(&mut game, "mira_entities", r#"{"with": "mira.Transform"}"#);
        let listed = json::parse(&listed[0]).unwrap();
        let Some(Value::Int(entity)) = listed.get_path("entities.0.entity").cloned() else {
            panic!("an entity: {listed:?}");
        };
        let arguments = format!(
            r#"{{"entity": {entity}, "component": "mira.Transform", "path": "translation.0", "value": 8.5, "cmd": "despawn"}}"#
        );
        let (_, failed) = tool(&mut game, "mira_set", &arguments);
        assert!(
            !failed,
            "and an argument called `cmd` can't turn one tool into another"
        );
        let moved = game
            .0
            .world
            .query::<&Transform>()
            .iter()
            .next()
            .unwrap()
            .translation
            .x;
        assert_eq!(moved, 8.5);

        // Signals come drawn as well as listed, and can be changed.
        let (signals, _) = tool(&mut game, "mira_signals", "{}");
        assert!(signals[0].contains("● door.open = true"), "{}", signals[0]);
        assert!(signals[1].contains("\"name\": \"door.shut\""));
        tool(
            &mut game,
            "mira_signal_set",
            r#"{"name": "door.open", "value": false}"#,
        );
        game.0.update();
        assert!(game.0.world.resource::<Signals>().is_true("door.shut"));

        // Refusals are results marked as errors, not protocol errors, so the agent reads why.
        let (why, failed) = tool(&mut game, "mira_get", r#"{"entity": 999}"#);
        assert!(failed && why[0].contains("there is no entity 999"));
        let (why, failed) = tool(&mut game, "mira_explode", "{}");
        assert!(failed && why[0].contains("no tool"));

        // The scene, as an image.
        let shot = rpc(
            &mut game,
            "tools/call",
            r#"{"name": "mira_screenshot", "arguments": {"width": 640}}"#,
        );
        assert_eq!(shot.get_path("content.0.type"), Some(&text("image")));
        assert_eq!(
            shot.get_path("content.0.mimeType"),
            Some(&text("image/png"))
        );
        assert_eq!(
            shot.get_path("content.0.data"),
            Some(&text(base64("\u{89}PNG fake 640".as_bytes())))
        );
    }

    #[test]
    fn bad_messages_are_answered_as_errors() {
        let mut game = game();
        let code = |line: &str, game: &mut Local| {
            let answer = json::parse(&handle_message(line, game).unwrap()).unwrap();
            answer.get_path("error.code").cloned()
        };
        assert_eq!(code("not json", &mut game), Some(Value::Int(-32700)));
        assert_eq!(code(r#"{"id": 1}"#, &mut game), Some(Value::Int(-32600)));
        assert_eq!(
            code(r#"{"id": 1, "method": "resources/list"}"#, &mut game),
            Some(Value::Int(-32601))
        );
        assert_eq!(
            code(
                r#"{"id": 1, "method": "tools/call", "params": {}}"#,
                &mut game
            ),
            Some(Value::Int(-32602))
        );
    }

    #[test]
    fn base64_matches_the_standard() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }
}
