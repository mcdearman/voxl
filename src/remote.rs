//! The debug connection: a running game answers questions and takes orders over a local
//! socket, so it can be inspected and changed from outside while it runs.
//!
//! The protocol is one JSON object per line each way. A request has a `cmd` and its
//! arguments, and optionally an `id` that the answer repeats:
//!
//! ```text
//! {"cmd": "get", "entity": 4294967297, "component": "mira.Transform"}
//! {"ok": {"translation": [0.0, 2.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}}
//! ```
//!
//! A request that can't be carried out is answered `{"error": "why"}`. Requests are handled
//! at the start of a frame, between frames of the game, and whether or not it is paused.
//! See `docs/LIVE.md` for the commands.

use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

use crate::{
    app::{App, Stage},
    ecs::{ComponentKey, Entity},
    input::{key_named, InjectedInput, Played},
    live::{FrameStats, History, Live},
    reflect::{json, Scene, Schema, TypeRegistry, Value},
    signal::{Op, Signal, Signals},
    time::Time,
    transform::despawn_recursive,
};

struct Client {
    stream: TcpStream,
    unread: Vec<u8>,
    /// What this client last knew, if it asked to be told of changes.
    watching: Option<Known>,
}

/// The state a watching client has been told of, to tell it only what is new.
struct Known {
    failures: usize,
    paused: bool,
    signals: Vec<(String, Signal)>,
}

impl Known {
    fn of(app: &App) -> Self {
        let live = app.world.resource::<Live>();
        Self {
            failures: live.failures().len(),
            paused: live.is_paused(),
            signals: app
                .world
                .get_resource::<Signals>()
                .map_or(Vec::new(), |signals| {
                    signals
                        .graph()
                        .into_iter()
                        .map(|node| (node.name, node.value))
                        .collect()
                }),
        }
    }

    /// What has happened since this was last brought up to date, oldest first.
    fn news(&mut self, app: &App) -> Vec<Value> {
        let now = Self::of(app);
        let mut news = Vec::new();
        let event = |kind: &str, mut fields: Vec<(&'static str, Value)>| {
            fields.insert(0, ("event", Value::Text(kind.to_owned())));
            map(fields)
        };
        let live = app.world.resource::<Live>();
        for failure in live.failures().iter().skip(self.failures) {
            news.push(event("failure", vec![("failure", failure_value(failure))]));
        }
        for (name, value) in &now.signals {
            let before = self
                .signals
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, v)| *v);
            if before != Some(*value) {
                let value = match value {
                    Signal::Bool(b) => Value::Bool(*b),
                    Signal::Number(n) => Value::Float(*n),
                };
                news.push(event(
                    "signal",
                    vec![("name", Value::Text(name.clone())), ("value", value)],
                ));
            }
        }
        if now.paused != self.paused {
            let frame = app
                .world
                .get_resource::<Time>()
                .map_or(0, Time::frame_count);
            news.push(event(
                if now.paused { "paused" } else { "resumed" },
                vec![("frame", Value::Int(frame as i64))],
            ));
        }
        *self = now;
        news
    }
}

fn failure_value(failure: &crate::live::Failure) -> Value {
    map([
        ("system", Value::Text(failure.system.clone())),
        ("stage", Value::Text(format!("{:?}", failure.stage))),
        ("frame", Value::Int(failure.frame as i64)),
        ("message", Value::Text(failure.message.clone())),
        ("location", Value::Text(failure.location.clone())),
        (
            "stack",
            texts(failure.stack.lines().map(|line| line.trim().to_owned())),
        ),
    ])
}

/// Listens for debuggers. Held by the app; made by [`App::listen_for_debugger`].
pub struct DebugServer {
    listener: TcpListener,
    clients: Vec<Client>,
}

impl DebugServer {
    fn bind(address: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: Vec::new(),
        })
    }

    /// Takes in new connections and returns the requests that have arrived, each with the
    /// client it came from.
    fn requests(&mut self) -> Vec<(usize, String)> {
        while let Ok((stream, _)) = self.listener.accept() {
            if stream.set_nonblocking(true).is_ok() {
                let _ = stream.set_nodelay(true);
                self.clients.push(Client {
                    stream,
                    unread: Vec::new(),
                    watching: None,
                });
            }
        }
        let mut requests = Vec::new();
        let mut gone = Vec::new();
        for (index, client) in self.clients.iter_mut().enumerate() {
            let mut chunk = [0u8; 4096];
            loop {
                match client.stream.read(&mut chunk) {
                    Ok(0) => {
                        gone.push(index);
                        break;
                    }
                    Ok(count) => client.unread.extend_from_slice(&chunk[..count]),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    Err(err) if err.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        gone.push(index);
                        break;
                    }
                }
            }
            while let Some(end) = client.unread.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = client.unread.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line).trim().to_owned();
                if !line.is_empty() {
                    requests.push((index, line));
                }
            }
        }
        // A client that has hung up is dropped once its last requests are answered.
        for index in gone {
            self.clients[index].unread.clear();
            let _ = self.clients[index]
                .stream
                .shutdown(std::net::Shutdown::Read);
        }
        requests
    }

    fn answer(&mut self, client: usize, line: &str) {
        let Some(client) = self.clients.get_mut(client) else {
            return;
        };
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        let mut sent = 0;
        let mut waited = 0;
        while sent < bytes.len() {
            match client.stream.write(&bytes[sent..]) {
                Ok(count) => sent += count,
                // The other end is slow to read: give it a moment, not the whole game.
                Err(err) if err.kind() == ErrorKind::WouldBlock && waited < 200 => {
                    waited += 1;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }

    fn drop_closed(&mut self) {
        self.clients.retain(|client| {
            let mut probe = [0u8; 1];
            !matches!(client.stream.peek(&mut probe), Ok(0))
        });
    }
}

type Answer = Result<Value, String>;

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str, String> {
    match request.field(key) {
        Some(Value::Text(text)) => Ok(text),
        Some(_) => Err(format!("`{key}` must be text")),
        None => Err(format!("this command needs `{key}`")),
    }
}

fn number(request: &Value, key: &str) -> Result<f64, String> {
    request
        .field(key)
        .ok_or_else(|| format!("this command needs `{key}`"))?
        .as_f64()
        .ok_or_else(|| format!("`{key}` must be a number"))
}

fn entity(app: &App, request: &Value) -> Result<Entity, String> {
    let bits = match request.field("entity") {
        Some(Value::Int(bits)) => *bits as u64,
        Some(Value::Entity(bits)) => *bits,
        Some(_) => return Err("`entity` must be an entity's number".to_owned()),
        None => return Err("this command needs `entity`".to_owned()),
    };
    let entity = Entity::from_bits(bits);
    if app.world.contains_entity(entity) {
        Ok(entity)
    } else {
        Err(format!("there is no entity {bits}"))
    }
}

fn map(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn texts(items: impl IntoIterator<Item = String>) -> Value {
    Value::List(items.into_iter().map(Value::Text).collect())
}

fn signal_of(value: &Value) -> Result<Signal, String> {
    match value {
        Value::Bool(b) => Ok(Signal::Bool(*b)),
        Value::Int(_) | Value::Float(_) => Ok(Signal::Number(value.as_f64().unwrap_or(0.0))),
        _ => Err("a signal's value is true, false or a number".to_owned()),
    }
}

fn op_of(request: &Value) -> Result<Op, String> {
    let value = request.field("value").map(signal_of).transpose()?;
    let seconds = request.field("seconds").and_then(Value::as_f64);
    Op::named(text(request, "op")?, value, seconds)
}

const STAGES: [Stage; 13] = [
    Stage::PreStartup,
    Stage::Startup,
    Stage::First,
    Stage::PreUpdate,
    Stage::FixedFirst,
    Stage::FixedUpdate,
    Stage::FixedLast,
    Stage::Update,
    Stage::PostUpdate,
    Stage::Last,
    Stage::Extract,
    Stage::Prepare,
    Stage::Render,
];

fn with_registry<R>(
    app: &mut App,
    body: impl FnOnce(&mut crate::ecs::World, &TypeRegistry) -> R,
) -> R {
    app.world
        .resource_scope(|world, registry: &mut TypeRegistry| body(world, registry))
}

/// A type's shape as plain data: what an agent or an inspector needs to know to write a
/// value of it.
pub fn schema_value(schema: &Schema) -> Value {
    let fields = |fields: &[(&'static str, Schema)]| {
        Value::Map(
            fields
                .iter()
                .map(|(name, schema)| ((*name).to_owned(), schema_value(schema)))
                .collect(),
        )
    };
    let of = |kind: &str, rest: Vec<(&'static str, Value)>| {
        let mut entries = vec![("type".to_owned(), Value::Text(kind.to_owned()))];
        entries.extend(rest.into_iter().map(|(key, value)| (key.to_owned(), value)));
        Value::Map(entries)
    };
    match schema {
        Schema::Any => Value::Text("any".to_owned()),
        Schema::Unit => Value::Text("nothing".to_owned()),
        Schema::Bool => Value::Text("bool".to_owned()),
        Schema::Int => Value::Text("integer".to_owned()),
        Schema::Float => Value::Text("number".to_owned()),
        Schema::Text => Value::Text("text".to_owned()),
        Schema::Entity => Value::Text("entity".to_owned()),
        Schema::List(item) => of("list", vec![("of", schema_value(item))]),
        Schema::Array(item, count) => of(
            "array",
            vec![
                ("of", schema_value(item)),
                ("length", Value::Int(*count as i64)),
            ],
        ),
        Schema::Optional(item) => of("optional", vec![("of", schema_value(item))]),
        Schema::Tuple(items) => of(
            "tuple",
            vec![("of", Value::List(items.iter().map(schema_value).collect()))],
        ),
        Schema::Fields(named) => fields(named),
        Schema::Struct { name, fields } => of(
            "struct",
            vec![
                ("name", Value::Text((*name).to_owned())),
                ("fields", schema_value(fields)),
            ],
        ),
        Schema::Enum { name, variants } => of(
            "enum",
            vec![
                ("name", Value::Text((*name).to_owned())),
                ("variants", fields(variants)),
            ],
        ),
    }
}

/// The scene in words: where the camera is, and what there is, nearest first, with where on
/// the screen each thing appears. For when a picture is more than is needed, or there is no
/// window to take one of.
fn describe(app: &mut App, limit: usize) -> String {
    use crate::{
        render::{Camera, Mesh, Mesh3d},
        transform::GlobalTransform,
        window::Window,
    };
    use std::fmt::Write;

    let mut out = String::new();
    let time = app.world.get_resource::<Time>();
    let frame = time.map_or(0, Time::frame_count);
    let paused = app.world.resource::<Live>().is_paused();
    let _ = writeln!(
        out,
        "frame {frame}, {}, {} entities",
        if paused { "paused" } else { "running" },
        app.world.entity_count()
    );

    let aspect = app
        .world
        .get_resource::<Window>()
        .map(|window| window.size())
        .filter(|size| size.y > 0)
        .map_or(16.0 / 9.0, |size| size.x as f32 / size.y as f32);
    let camera = app
        .world
        .query::<(Entity, &Camera, &GlobalTransform)>()
        .iter()
        .find(|(_, camera, _)| camera.active)
        .map(|(entity, camera, at)| (entity, camera.projection(aspect) * at.0.inverse(), *at));
    let eye = match &camera {
        Some((entity, _, at)) => {
            let (p, f) = (at.translation(), at.forward());
            let _ = writeln!(
                out,
                "camera #{} at ({:.1}, {:.1}, {:.1}) looking along ({:.2}, {:.2}, {:.2})",
                entity.to_bits(),
                p.x,
                p.y,
                p.z,
                f.x,
                f.y,
                f.z
            );
            Some(p)
        }
        None => {
            let _ = writeln!(out, "no active camera; distances are from the origin");
            None
        }
    };

    // Each placed entity: where it is, where it shows, and what it is made of.
    let placed: Vec<(Entity, glam::Vec3)> = app
        .world
        .query::<(Entity, &GlobalTransform)>()
        .iter()
        .map(|(entity, at)| (entity, at.translation()))
        .collect();
    let mut lines: Vec<(bool, f32, String)> = Vec::new();
    let registry = app.world.resource::<TypeRegistry>();
    let server = app.world.get_resource::<crate::asset_server::AssetServer>();
    for (entity, position) in placed {
        if camera
            .as_ref()
            .is_some_and(|(camera, ..)| *camera == entity)
        {
            continue;
        }
        let distance = position.distance(eye.unwrap_or(glam::Vec3::ZERO));
        let (seen, place) = match &camera {
            Some((_, view_projection, _)) => {
                let clip = *view_projection * position.extend(1.0);
                let (x, y) = (clip.x / clip.w, clip.y / clip.w);
                if clip.w <= 0.0 {
                    (false, "behind the camera".to_owned())
                } else if x.abs() > 1.0 || y.abs() > 1.0 {
                    let side = if x < -1.0 {
                        "to the left"
                    } else if x > 1.0 {
                        "to the right"
                    } else if y > 1.0 {
                        "above"
                    } else {
                        "below"
                    };
                    (false, format!("out of view {side}"))
                } else {
                    let across = if x < -0.33 {
                        "left"
                    } else if x > 0.33 {
                        "right"
                    } else {
                        "centre"
                    };
                    let up = if y > 0.33 {
                        "top"
                    } else if y < -0.33 {
                        "bottom"
                    } else {
                        "middle"
                    };
                    (true, format!("on screen, {up} {across}"))
                }
            }
            None => (false, "not drawn".to_owned()),
        };
        let mut made_of: Vec<String> = registry
            .iter()
            .filter(|component| (component.get)(&app.world, entity).is_some())
            .map(|component| component.name.trim_start_matches("mira.").to_owned())
            .filter(|name| name != "Transform")
            .collect();
        let mesh = app
            .world
            .get::<Mesh3d>(entity)
            .and_then(|mesh| server?.name_of::<Mesh>(mesh.0));
        if let (Some(mesh), Some(slot)) = (mesh, made_of.iter_mut().find(|name| *name == "Mesh3d"))
        {
            *slot = format!("Mesh3d {mesh}");
        }
        lines.push((
            seen,
            distance,
            format!(
                "#{} at ({:.1}, {:.1}, {:.1}), {distance:.1} away, {place}: {}",
                entity.to_bits(),
                position.x,
                position.y,
                position.z,
                if made_of.is_empty() {
                    "nothing registered".to_owned()
                } else {
                    made_of.join(", ")
                }
            ),
        ));
    }
    // What can be seen first, then nearest first.
    lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    let (total, on_screen) = (lines.len(), lines.iter().filter(|line| line.0).count());
    let _ = writeln!(out, "{total} placed entities, {on_screen} on screen");
    for (_, _, line) in lines.iter().take(limit) {
        let _ = writeln!(out, "{line}");
    }
    if total > limit {
        let _ = writeln!(
            out,
            "… and {} more (raise `limit` to list them)",
            total - limit
        );
    }
    out
}

/// Plays a key or button going down, coming up, or both a few frames apart.
fn play_button(
    injected: &mut InjectedInput,
    action: &str,
    frames: u32,
    played: impl Fn(bool) -> Played,
) -> Result<(), String> {
    match action {
        "press" => injected.play(played(true)),
        "release" => injected.play(played(false)),
        "tap" => {
            injected.play(played(true));
            injected.play_after(frames, played(false));
        }
        other => {
            return Err(format!(
                "there is no action `{other}`; use press, release or tap"
            ))
        }
    }
    Ok(())
}

/// Carries out one request.
fn handle(app: &mut App, request: &Value) -> Answer {
    let done = Ok(Value::Bool(true));
    match text(request, "cmd")? {
        "status" => {
            let live = app.world.resource::<Live>();
            let (paused, failures) = (live.is_paused(), live.failures().len());
            let (stepping, reached) = (live.steps_left(), live.reached());
            let time = app.world.get_resource::<Time>();
            Ok(map([
                (
                    "frame",
                    Value::Int(time.map_or(0, Time::frame_count) as i64),
                ),
                (
                    "seconds",
                    Value::Float(time.map_or(0.0, |t| t.elapsed().as_secs_f64())),
                ),
                (
                    "time_scale",
                    Value::Float(time.map_or(1.0, |t| t.scale() as f64)),
                ),
                ("paused", Value::Bool(paused)),
                ("stepping", Value::Int(stepping as i64)),
                ("reached", Value::Bool(reached)),
                ("failures", Value::Int(failures as i64)),
                ("entities", Value::Int(app.world.entity_count() as i64)),
            ]))
        }
        "quit" => {
            // The same event a game sends itself to stop: the loop ends after this frame.
            match app
                .world
                .get_resource_mut::<crate::ecs::Events<crate::app::AppExit>>()
            {
                Some(events) => {
                    events.send(crate::app::AppExit);
                    done
                }
                None => Err("this game has no way to be told to stop".to_owned()),
            }
        }
        "pause" => {
            app.world.resource_mut::<Live>().pause();
            done
        }
        "resume" => {
            app.world.resource_mut::<Live>().resume();
            done
        }
        "step" => {
            let frames = request
                .field("frames")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            app.world
                .resource_mut::<Live>()
                .step_frames(frames.max(1.0) as u32);
            done
        }
        "run_until" => {
            let signal = text(request, "signal")?;
            if app.world.resource::<Signals>().get(signal).is_none() {
                return Err(format!("there is no signal `{signal}`"));
            }
            let frames = request
                .field("max_frames")
                .and_then(Value::as_f64)
                .unwrap_or(600.0);
            app.world
                .resource_mut::<Live>()
                .step_until(signal, frames.max(1.0) as u32);
            done
        }
        "input" => {
            let injected = app
                .world
                .get_resource_mut::<InjectedInput>()
                .ok_or("this game takes no input")?;
            let frames = request
                .field("frames")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .max(1.0) as u32;
            let action = match request.field("action") {
                Some(Value::Text(action)) => action.as_str(),
                None => "tap",
                Some(_) => return Err("`action` is press, release or tap".to_owned()),
            };
            let mut played = 0;
            let pair = |key: &str| match request.field(key) {
                Some(Value::List(pair)) if pair.len() == 2 => {
                    match (pair[0].as_f64(), pair[1].as_f64()) {
                        (Some(x), Some(y)) => Ok(Some(glam::Vec2::new(x as f32, y as f32))),
                        _ => Err(format!("`{key}` is two numbers")),
                    }
                }
                Some(_) => Err(format!("`{key}` is two numbers")),
                None => Ok(None),
            };
            // Where the pointer is comes before what its buttons do there.
            if let Some(position) = pair("mouse_position")? {
                injected.play(Played::Cursor(position));
                played += 1;
            }
            if let Some(Value::Text(name)) = request.field("key") {
                let key = key_named(name).ok_or_else(|| {
                    format!("there is no key `{name}`; keys are named like KeyW, Space, ArrowLeft, Digit1, ShiftLeft")
                })?;
                play_button(injected, action, frames, |down| Played::Key(key, down))?;
                played += 1;
            }
            if let Some(Value::Text(name)) = request.field("mouse_button") {
                let button = match name.to_ascii_lowercase().as_str() {
                    "left" => crate::input::MouseButton::Left,
                    "right" => crate::input::MouseButton::Right,
                    "middle" => crate::input::MouseButton::Middle,
                    _ => {
                        return Err(format!(
                            "there is no mouse button `{name}`; use left, right or middle"
                        ))
                    }
                };
                play_button(injected, action, frames, |down| {
                    Played::Button(button, down)
                })?;
                played += 1;
            }
            if let Some(motion) = pair("mouse_motion")? {
                injected.play(Played::Motion(motion));
                played += 1;
            }
            if let Some(lines) = pair("mouse_scroll")? {
                injected.play(Played::Scroll(lines));
                played += 1;
            }
            if let Some(Value::Text(text)) = request.field("text") {
                injected.type_text(text.clone());
                played += 1;
            }
            if played == 0 {
                return Err(
                    "give a `key`, a `mouse_button`, `mouse_motion`, `mouse_position`, `mouse_scroll` or `text`"
                        .to_owned(),
                );
            }
            Ok(Value::Int(played))
        }
        "time_scale" => {
            let scale = number(request, "scale")? as f32;
            app.world
                .get_resource_mut::<Time>()
                .ok_or("this app keeps no time")?
                .set_scale(scale);
            done
        }
        "record" => {
            let history = app.world.resource_mut::<History>();
            if let Some(Value::Bool(on)) = request.field("on") {
                history.recording = *on;
            }
            if let Some(every) = request.field("every").and_then(Value::as_f64) {
                history.every = every.max(1.0) as u64;
            }
            if let Some(keep) = request.field("keep").and_then(Value::as_f64) {
                history.keep = keep.max(1.0) as usize;
            }
            Ok(map([
                ("recording", Value::Bool(history.recording)),
                ("every", Value::Int(history.every as i64)),
                ("keep", Value::Int(history.keep as i64)),
                ("snapshots", Value::Int(history.moments().len() as i64)),
            ]))
        }
        "history" => Ok(Value::List(
            app.world
                .resource::<History>()
                .moments()
                .into_iter()
                .map(|moment| {
                    map([
                        ("frame", Value::Int(moment.frame as i64)),
                        ("seconds", Value::Float(moment.seconds)),
                    ])
                })
                .collect(),
        )),
        "rewind" => {
            let moment = match request.field("to_frame").and_then(Value::as_f64) {
                Some(frame) => History::rewind_to(&mut app.world, frame.max(0.0) as u64),
                None => {
                    let frames = request
                        .field("frames")
                        .and_then(Value::as_f64)
                        .unwrap_or(60.0);
                    History::rewind(&mut app.world, frames.max(0.0) as u64)
                }
            };
            let moment = moment.ok_or("there is no snapshot that far back (is recording on?)")?;
            Ok(map([
                ("frame", Value::Int(moment.frame as i64)),
                ("seconds", Value::Float(moment.seconds)),
            ]))
        }
        "failures" => Ok(Value::List(
            app.world
                .resource::<Live>()
                .failures()
                .iter()
                .map(failure_value)
                .collect(),
        )),
        "systems" => {
            let only = request.field("stage").and_then(|stage| match stage {
                Value::Text(name) => Some(name.as_str()),
                _ => None,
            });
            let stages = STAGES
                .into_iter()
                .filter(|stage| {
                    only.is_none_or(|name| name.eq_ignore_ascii_case(&format!("{stage:?}")))
                })
                .map(|stage| {
                    let systems = app.systems(stage).into_iter().map(|system| {
                        map([
                            ("name", Value::Text(system.name)),
                            ("sets", texts(system.sets)),
                            ("before", texts(system.before)),
                            ("after", texts(system.after)),
                            ("conditions", Value::Int(system.conditions as i64)),
                            ("suspended", Value::Bool(system.suspended)),
                            (
                                "main_thread",
                                system.main_thread.map_or(Value::Null, Value::Bool),
                            ),
                            (
                                "batch",
                                system
                                    .batch
                                    .map_or(Value::Null, |batch| Value::Int(batch as i64)),
                            ),
                            (
                                "access",
                                system.access.map_or(
                                    Value::Text("the whole world".into()),
                                    |access| {
                                        map([
                                            ("reads", texts(access.reads)),
                                            ("writes", texts(access.writes)),
                                            ("resource_reads", texts(access.resource_reads)),
                                            ("resource_writes", texts(access.resource_writes)),
                                        ])
                                    },
                                ),
                            ),
                            ("runs", Value::Int(system.stats.runs as i64)),
                            (
                                "last_micros",
                                Value::Float(system.stats.last.as_secs_f64() * 1e6),
                            ),
                            (
                                "total_millis",
                                Value::Float(system.stats.total.as_secs_f64() * 1e3),
                            ),
                        ])
                    });
                    (format!("{stage:?}"), Value::List(systems.collect()))
                })
                .filter(|(_, systems)| !matches!(systems, Value::List(list) if list.is_empty()))
                .collect();
            Ok(Value::Map(stages))
        }
        "types" => {
            let registry = app.world.resource::<TypeRegistry>();
            Ok(map([
                (
                    "components",
                    texts(registry.iter().map(|t| t.name.to_owned())),
                ),
                (
                    "resources",
                    texts(registry.resources().map(|t| t.name.to_owned())),
                ),
            ]))
        }
        "schema" => {
            let name = text(request, "name")?;
            let registry = app.world.resource::<TypeRegistry>();
            let schema = match (registry.get(name), registry.resource(name)) {
                (Some(component), _) => (component.schema)(),
                (None, Some(resource)) => (resource.schema)(),
                (None, None) => return Err(format!("there is no type `{name}`")),
            };
            Ok(schema_value(&schema))
        }
        "profile" => {
            // Where the time goes: frames, then stages, then the systems that cost most.
            let count = request
                .field("systems")
                .and_then(Value::as_f64)
                .unwrap_or(10.0) as usize;
            let millis = |time: Duration| Value::Float(time.as_secs_f64() * 1e3);
            let stats = app.world.resource::<FrameStats>();
            let mean = stats.mean();
            let frames = map([
                ("measured", Value::Int(stats.frames().count() as i64)),
                ("mean_millis", millis(mean)),
                ("worst_millis", millis(stats.worst())),
                (
                    "could_reach_fps",
                    Value::Float(if mean.is_zero() {
                        0.0
                    } else {
                        1.0 / mean.as_secs_f64()
                    }),
                ),
            ]);
            let stages = Value::Map(
                stats
                    .stages()
                    .iter()
                    .map(|(stage, took)| (format!("{stage:?}"), millis(*took)))
                    .collect(),
            );
            let mut systems: Vec<(String, crate::ecs::SystemInfo)> = STAGES
                .into_iter()
                .flat_map(|stage| {
                    app.systems(stage)
                        .into_iter()
                        .map(move |system| (format!("{stage:?}"), system))
                })
                .filter(|(_, system)| system.stats.runs > 0)
                .collect();
            systems.sort_by_key(|(_, system)| std::cmp::Reverse(system.stats.last));
            let systems = systems.into_iter().take(count).map(|(stage, system)| {
                let mean = system.stats.total / system.stats.runs.max(1) as u32;
                map([
                    ("system", Value::Text(system.name)),
                    ("stage", Value::Text(stage)),
                    ("last_millis", millis(system.stats.last)),
                    ("mean_millis", millis(mean)),
                    ("runs", Value::Int(system.stats.runs as i64)),
                ])
            });
            Ok(map([
                ("frames", frames),
                ("stages", stages),
                ("systems", Value::List(systems.collect())),
            ]))
        }
        "unregistered" => {
            // What the world holds that can't be reached by name: invisible to scenes,
            // to stepping back, and to whoever is on the other end of this connection.
            let registry = app.world.resource::<TypeRegistry>();
            let components = app
                .world
                .component_kinds()
                .into_iter()
                .filter(|(key, name, _)| match key {
                    ComponentKey::Type(rust_type) => !registry.knows_type(*rust_type),
                    ComponentKey::Dynamic(_) => registry.get(name).is_none(),
                })
                .map(|(_, name, entities)| {
                    map([
                        ("type", Value::Text(name)),
                        ("entities", Value::Int(entities as i64)),
                    ])
                });
            let resources = app
                .world
                .resource_kinds()
                .into_iter()
                .filter(|(rust_type, _)| !registry.knows_type(*rust_type))
                .map(|(_, name)| name.to_owned());
            Ok(map([
                ("components", Value::List(components.collect())),
                ("resources", texts(resources)),
            ]))
        }
        "describe" => {
            let limit = request
                .field("limit")
                .and_then(Value::as_f64)
                .unwrap_or(40.0) as usize;
            Ok(Value::Text(describe(app, limit)))
        }
        "screenshot" => {
            let path = text(request, "path")?;
            let screenshot = app
                .world
                .get_resource_mut::<crate::render::Screenshot>()
                .ok_or(
                    "this game draws nothing (it has no renderer), so there is nothing to see",
                )?;
            screenshot.request(path);
            // The frame after this request is the one saved.
            Ok(map([("path", Value::Text(path.to_owned()))]))
        }
        "entities" => {
            // Entities by what they have, optionally only those with one component.
            let with = match request.field("with") {
                Some(Value::Text(name)) => Some(name.clone()),
                _ => None,
            };
            let limit = request
                .field("limit")
                .and_then(Value::as_f64)
                .unwrap_or(200.0) as usize;
            let registry = app.world.resource::<TypeRegistry>();
            let mut found: std::collections::BTreeMap<Entity, Vec<String>> = Default::default();
            for component in registry.iter() {
                for entity in (component.entities)(&app.world) {
                    found
                        .entry(entity)
                        .or_default()
                        .push(component.name.to_owned());
                }
            }
            if let Some(with) = &with {
                if registry.get(with).is_none() {
                    return Err(format!("there is no component `{with}`"));
                }
                found.retain(|_, components| components.contains(with));
            }
            let total = found.len();
            let listed = found.into_iter().take(limit).map(|(entity, components)| {
                map([
                    ("entity", Value::Int(entity.to_bits() as i64)),
                    ("components", texts(components)),
                ])
            });
            Ok(map([
                ("entities", Value::List(listed.collect())),
                ("total", Value::Int(total as i64)),
            ]))
        }
        "get" => {
            let entity = entity(app, request)?;
            let registry = app.world.resource::<TypeRegistry>();
            match request.field("component") {
                Some(Value::Text(name)) => {
                    let component = registry
                        .get(name)
                        .ok_or_else(|| format!("there is no component `{name}`"))?;
                    (component.get)(&app.world, entity)
                        .ok_or_else(|| format!("the entity has no `{name}`"))
                }
                _ => Ok(Value::Map(
                    registry
                        .iter()
                        .filter_map(|c| Some((c.name.to_owned(), (c.get)(&app.world, entity)?)))
                        .collect(),
                )),
            }
        }
        "set" => {
            let entity = entity(app, request)?;
            let name = text(request, "component")?;
            let new = request.field("value").ok_or("this command needs `value`")?;
            let path = request.field("path").and_then(|path| match path {
                Value::Text(path) => Some(path.as_str()),
                _ => None,
            });
            with_registry(app, |world, registry| {
                let component = registry
                    .get(name)
                    .ok_or_else(|| format!("there is no component `{name}`"))?;
                let value = match path {
                    None | Some("") => new.clone(),
                    Some(path) => {
                        let mut value = (component.get)(world, entity)
                            .ok_or_else(|| format!("the entity has no `{name}`"))?;
                        if !value.set_path(path, new.clone()) {
                            return Err(format!("`{name}` has no `{path}`"));
                        }
                        value
                    }
                };
                (component.insert)(world, entity, &value).map_err(|err| err.to_string())?;
                Ok(Value::Bool(true))
            })
        }
        "remove" => {
            let entity = entity(app, request)?;
            let name = text(request, "component")?;
            with_registry(app, |world, registry| {
                let component = registry
                    .get(name)
                    .ok_or_else(|| format!("there is no component `{name}`"))?;
                (component.remove)(world, entity);
                Ok(Value::Bool(true))
            })
        }
        "spawn" => {
            let Some(Value::Map(components)) = request.field("components") else {
                return Err("this command needs `components`, a map of component values".to_owned());
            };
            with_registry(app, |world, registry| {
                for (name, _) in components {
                    if registry.get(name).is_none() {
                        return Err(format!("there is no component `{name}`"));
                    }
                }
                let entity = world.spawn_empty();
                for (name, value) in components {
                    let component = registry.get(name).expect("checked above");
                    if let Err(err) = (component.insert)(world, entity, value) {
                        world.despawn(entity);
                        return Err(format!("`{name}`: {err}"));
                    }
                }
                Ok(Value::Int(entity.to_bits() as i64))
            })
        }
        "despawn" => {
            let entity = entity(app, request)?;
            Ok(Value::Int(despawn_recursive(&mut app.world, entity) as i64))
        }
        "resource" => {
            let name = text(request, "name")?;
            match request.field("value") {
                None => {
                    let registry = app.world.resource::<TypeRegistry>();
                    let resource = registry
                        .resource(name)
                        .ok_or_else(|| format!("there is no resource `{name}`"))?;
                    (resource.get)(&app.world).ok_or_else(|| format!("the world has no `{name}`"))
                }
                Some(value) => with_registry(app, |world, registry| {
                    let resource = registry
                        .resource(name)
                        .ok_or_else(|| format!("there is no resource `{name}`"))?;
                    (resource.insert)(world, value).map_err(|err| err.to_string())?;
                    Ok(Value::Bool(true))
                }),
            }
        }
        "save_scene" => {
            let path = text(request, "path")?;
            with_registry(app, |world, registry| {
                let scene = Scene::capture(world, registry);
                scene.save(path).map_err(|err| format!("{err:#}"))?;
                Ok(Value::Int(scene.entities.len() as i64))
            })
        }
        "reload_plugins" => Ok(Value::Int(app.reload_native_plugins() as i64)),
        "signals" => Ok(app.world.resource::<Signals>().to_value()),
        "signal_set" => {
            let value = signal_of(request.field("value").ok_or("this command needs `value`")?)?;
            app.world
                .resource_mut::<Signals>()
                .set(text(request, "name")?, value);
            done
        }
        "signal_force" => {
            let value = match request.field("value") {
                None | Some(Value::Null) => None,
                Some(value) => Some(signal_of(value)?),
            };
            let name = text(request, "name")?;
            if app.world.resource_mut::<Signals>().force(name, value) {
                done
            } else {
                Err(format!("there is no signal `{name}`"))
            }
        }
        "signal_connect" => {
            let (name, to) = (text(request, "name")?, text(request, "to")?);
            let input = number(request, "input")? as usize;
            if app.world.resource_mut::<Signals>().connect(name, input, to) {
                done
            } else {
                Err(format!("`{name}` has no input {input}"))
            }
        }
        "signal_define" => {
            let op = op_of(request)?;
            let inputs: Vec<String> = match request.field("inputs") {
                Some(Value::List(inputs)) => inputs
                    .iter()
                    .map(|input| match input {
                        Value::Text(name) => Ok(name.clone()),
                        _ => Err("`inputs` must be a list of signal names".to_owned()),
                    })
                    .collect::<Result<_, _>>()?,
                None => Vec::new(),
                Some(_) => return Err("`inputs` must be a list of signal names".to_owned()),
            };
            app.world
                .resource_mut::<Signals>()
                .define(text(request, "name")?, op, inputs);
            done
        }
        "signals_save" => {
            let path = text(request, "path")?;
            app.world.resource::<Signals>().save_rules(path)?;
            done
        }
        "signals_load" => {
            let path = text(request, "path")?;
            let defined = app.world.resource_mut::<Signals>().load_rules(path)?;
            Ok(Value::Int(defined as i64))
        }
        "signal_rename" => {
            let (name, to) = (text(request, "name")?, text(request, "to")?);
            app.world.resource_mut::<Signals>().rename(name, to)?;
            done
        }
        "signal_remove" => {
            let name = text(request, "name")?;
            if app.world.resource_mut::<Signals>().remove(name) {
                done
            } else {
                Err(format!("there is no signal `{name}`"))
            }
        }
        // Answered by the server itself when it comes over a socket.
        "watch" => Err("watching needs a connection that stays open".to_owned()),
        other => Err(format!("there is no command `{other}`")),
    }
}

/// Draws the signal graph (what the `signals` command answers) as text: each signal that
/// nothing else reads, with what it reads below it, and so on down to the sources.
///
/// ```text
/// ○ red.wins = false  (compare GreaterOrEqual)
/// ├─ ● red.clock = 7  (timer)
/// │  ├─ ● red.clock.running = true  (and) *
/// │  │  ├─ ● red.holds_all = true  (source)
/// ```
///
/// `●` is true or not zero, `*` changed on the last update, `!` is forced.
pub fn signals_text(graph: &Value) -> String {
    let Value::List(nodes) = graph else {
        return String::new();
    };
    let name_of = |node: &Value| match node.field("name") {
        Some(Value::Text(name)) => name.clone(),
        _ => String::new(),
    };
    let inputs_of = |node: &Value| match node.field("inputs") {
        Some(Value::List(inputs)) => inputs
            .iter()
            .filter_map(|input| match input {
                Value::Text(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let find = |name: &str| nodes.iter().find(|node| name_of(node) == name);
    let read: std::collections::HashSet<String> = nodes.iter().flat_map(inputs_of).collect();

    fn line(node: &Value, name: &str) -> String {
        let value = match node.field("value") {
            Some(Value::Bool(b)) => b.to_string(),
            Some(Value::Float(n)) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{n:.0}"),
            Some(Value::Float(n)) => format!("{n:.3}"),
            _ => "?".to_owned(),
        };
        let on = !matches!(value.as_str(), "false" | "0" | "?");
        let flag = |key: &str, mark: &str| match node.field(key) {
            Some(Value::Bool(true)) => mark.to_owned(),
            _ => String::new(),
        };
        let kind = match node.field("kind") {
            Some(Value::Text(kind)) => kind.as_str(),
            _ => "?",
        };
        let problem = match node.field("problem") {
            Some(Value::Text(problem)) => format!("  ({problem})"),
            _ => String::new(),
        };
        format!(
            "{} {name} = {value}  ({kind}){}{}{problem}",
            if on { "●" } else { "○" },
            flag("changed", " *"),
            flag("forced", " !"),
        )
    }

    let mut out = String::new();
    // (name, the prefix its line gets, the prefix its children's lines get, its ancestors)
    let mut pending: Vec<(String, String, String, Vec<String>)> = nodes
        .iter()
        .map(name_of)
        .filter(|name| !read.contains(name))
        .rev()
        .map(|name| (name, String::new(), String::new(), Vec::new()))
        .collect();
    let mut drawn = std::collections::HashSet::new();
    loop {
        let Some((name, prefix, below, above)) = pending.pop() else {
            // Signals on a circle have nothing above them: start again from the first of
            // them that hasn't been drawn.
            match nodes.iter().map(name_of).find(|name| !drawn.contains(name)) {
                Some(name) => pending.push((name, String::new(), String::new(), Vec::new())),
                None => break,
            }
            continue;
        };
        drawn.insert(name.clone());
        let Some(node) = find(&name) else {
            out += &format!("{prefix}? {name}  (no such signal)\n");
            continue;
        };
        if above.contains(&name) {
            out += &format!("{prefix}↺ {name}\n");
            continue;
        }
        out += &format!("{prefix}{}\n", line(node, &name));
        let inputs = inputs_of(node);
        let mut path = above.clone();
        path.push(name);
        for (index, input) in inputs.iter().enumerate().rev() {
            let last = index + 1 == inputs.len();
            pending.push((
                input.clone(),
                format!("{below}{}", if last { "└─ " } else { "├─ " }),
                format!("{below}{}", if last { "   " } else { "│  " }),
                path.clone(),
            ));
        }
    }
    out
}

/// Answers one line of the protocol with one line.
pub fn respond(app: &mut App, line: &str) -> String {
    let (id, answer) = match json::parse(line) {
        Ok(request) => (request.field("id").cloned(), handle(app, &request)),
        Err(err) => (None, Err(format!("not JSON: {err}"))),
    };
    let mut fields = Vec::new();
    if let Some(id) = id {
        fields.push(("id".to_owned(), id));
    }
    fields.push(match answer {
        Ok(value) => ("ok".to_owned(), value),
        Err(why) => ("error".to_owned(), Value::Text(why)),
    });
    json::to_line(&Value::Map(fields))
}

impl App {
    /// Starts answering debuggers on a local address (`"127.0.0.1:7878"`; port 0 picks a
    /// free one) and returns the address in use. The game then answers requests at the start
    /// of every frame. Anyone who can connect can change the game, so listen only on the
    /// machine itself.
    pub fn listen_for_debugger(&mut self, address: &str) -> std::io::Result<SocketAddr> {
        let server = DebugServer::bind(address)?;
        let address = server.listener.local_addr()?;
        log::info!("listening for debuggers on {address}");
        self.debug = Some(server);
        Ok(address)
    }

    /// Where the game is listening for debuggers, if it is.
    pub fn debugger_address(&self) -> Option<SocketAddr> {
        self.debug.as_ref()?.listener.local_addr().ok()
    }

    /// Listens where `MIRA_DEBUG` says (`MIRA_DEBUG=127.0.0.1:7878`), if it is set.
    pub fn listen_for_debugger_from_env(&mut self) -> &mut Self {
        if let Ok(address) = std::env::var("MIRA_DEBUG") {
            if let Err(err) = self.listen_for_debugger(&address) {
                log::error!("can't listen for debuggers on {address}: {err}");
            }
        }
        self
    }

    pub(crate) fn serve_debuggers(&mut self) {
        let Some(mut server) = self.debug.take() else {
            return;
        };
        for (client, line) in server.requests() {
            // `watch` is about the connection, which only the server knows.
            let watch = json::parse(&line).ok().and_then(|request| {
                let on = match request.field("cmd") {
                    Some(Value::Text(cmd)) if cmd == "watch" => {
                        !matches!(request.field("on"), Some(Value::Bool(false)))
                    }
                    _ => return None,
                };
                Some((on, request.field("id").cloned()))
            });
            let answer = match watch {
                Some((on, id)) => {
                    server.clients[client].watching = on.then(|| Known::of(self));
                    let mut fields = Vec::new();
                    fields.extend(id.map(|id| ("id".to_owned(), id)));
                    fields.push(("ok".to_owned(), Value::Bool(on)));
                    json::to_line(&Value::Map(fields))
                }
                None => respond(self, &line),
            };
            server.answer(client, &answer);
        }
        // Tell the clients that asked what has changed since the last frame.
        for client in 0..server.clients.len() {
            let news = match &mut server.clients[client].watching {
                Some(known) => known.news(self),
                None => continue,
            };
            for event in news {
                server.answer(client, &json::to_line(&event));
            }
        }
        server.drop_closed();
        // A request may have replaced the server; the one that was serving stays.
        self.debug = Some(server);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};

    use super::*;
    use crate::{
        ecs::{Res, ResMut},
        physics::{Collider, RigidBody},
        time::TimePlugin,
        transform::{Parent, Transform, TransformPlugin},
    };

    #[derive(Default)]
    struct Frames(u32);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin).add_plugins(TransformPlugin);
        app.register_type::<RigidBody>()
            .register_type::<Collider>()
            .register_resource_type::<crate::render::Fog>()
            .init_resource::<Frames>();
        app.world
            .resource_mut::<Time>()
            .set_fixed_step(Some(Duration::from_millis(10)));
        app.add_signal("warm", |frames: Res<Frames>| frames.0 >= 2)
            .add_systems(Stage::Update, |mut frames: ResMut<Frames>| frames.0 += 1);
        app
    }

    /// Sends a request written with single quotes for double, and returns what `ok` holds.
    fn ask(app: &mut App, request: &str) -> Value {
        let answer = json::parse(&respond(app, &request.replace('\'', "\""))).unwrap();
        match answer.field("ok") {
            Some(value) => value.clone(),
            None => panic!("{request} was refused: {answer:?}"),
        }
    }

    fn refused(app: &mut App, request: &str) -> String {
        let answer = json::parse(&respond(app, &request.replace('\'', "\""))).unwrap();
        match answer.field("error") {
            Some(Value::Text(why)) => why.clone(),
            _ => panic!("{request} was not refused: {answer:?}"),
        }
    }

    #[test]
    fn the_world_can_be_read_and_changed_by_name() {
        let mut app = app();
        let parent = app.world.spawn(Transform::from_xyz(1.0, 2.0, 3.0));
        let child = app.world.spawn((Transform::IDENTITY, Parent(parent)));
        app.update();

        let listed = ask(&mut app, "{'cmd': 'entities', 'with': 'mira.Parent'}");
        assert_eq!(listed.field("total"), Some(&Value::Int(1)));
        assert_eq!(
            listed.get_path("entities.0.entity"),
            Some(&Value::Int(child.to_bits() as i64))
        );
        assert_eq!(
            ask(&mut app, "{'cmd': 'entities', 'limit': 1}").field("total"),
            Some(&Value::Int(2))
        );

        let bits = parent.to_bits();
        let all = ask(&mut app, &format!("{{'cmd': 'get', 'entity': {bits}}}"));
        let transform = all.field("mira.Transform").expect("its transform");
        assert_eq!(
            transform.get_path("translation.2"),
            Some(&Value::Float(3.0))
        );
        ask(&mut app, &format!("{{'cmd': 'set', 'entity': {bits}, 'component': 'mira.Transform', 'path': 'translation.1', 'value': 9.5}}"));
        assert_eq!(
            app.world.get::<Transform>(parent).unwrap().translation.y,
            9.5
        );
        // A whole component, on an entity that hadn't one; and taken away again.
        ask(&mut app, &format!("{{'cmd': 'set', 'entity': {bits}, 'component': 'mira.RigidBody', 'value': {{'mass': 4.0}}}}"));
        assert_eq!(app.world.get::<RigidBody>(parent).unwrap().mass, 4.0);
        ask(
            &mut app,
            &format!("{{'cmd': 'remove', 'entity': {bits}, 'component': 'mira.RigidBody'}}"),
        );
        assert!(!app.world.has::<RigidBody>(parent));

        let spawned = ask(&mut app, "{'cmd': 'spawn', 'components': {'mira.Transform': {'translation': [0, 5, 0], 'rotation': [0, 0, 0, 1], 'scale': [1, 1, 1]}}}");
        let Value::Int(spawned) = spawned else {
            panic!("the new entity's number");
        };
        assert_eq!(
            app.world
                .get::<Transform>(Entity::from_bits(spawned as u64))
                .unwrap()
                .translation
                .y,
            5.0
        );
        assert_eq!(
            ask(&mut app, &format!("{{'cmd': 'despawn', 'entity': {bits}}}")),
            Value::Int(2),
            "children go too"
        );
        assert_eq!(app.world.entity_count(), 1);

        ask(
            &mut app,
            "{'cmd': 'resource', 'name': 'mira.Fog', 'value': {'density': 0.5}}",
        );
        assert_eq!(app.world.resource::<crate::render::Fog>().density, 0.5);
        assert_eq!(
            ask(&mut app, "{'cmd': 'resource', 'name': 'mira.Fog'}").field("density"),
            Some(&Value::Float(0.5))
        );
        let types = ask(&mut app, "{'cmd': 'types'}");
        assert!(
            matches!(types.field("components"), Some(Value::List(names)) if names.contains(&Value::Text("mira.Collider".into())))
        );

        // What can't be done says why, and changes nothing.
        // The shape of a type, for whoever has to write one.
        let shape = ask(&mut app, "{'cmd': 'schema', 'name': 'mira.Transform'}");
        assert_eq!(shape.field("type"), Some(&Value::Text("struct".into())));
        assert_eq!(
            shape.get_path("fields.translation.length"),
            Some(&Value::Int(3))
        );
        assert_eq!(
            ask(&mut app, "{'cmd': 'schema', 'name': 'mira.Fog'}").get_path("fields.density"),
            Some(&Value::Text("number".into()))
        );
        assert!(refused(&mut app, "{'cmd': 'schema', 'name': 'mira.Nothing'}").contains("no type"));
        assert!(refused(&mut app, "{'cmd': 'screenshot', 'path': 'x.png'}").contains("no renderer"));
        assert_eq!(
            refused(&mut app, "{'cmd': 'get', 'entity': 77}"),
            "there is no entity 77"
        );
        assert_eq!(
            refused(&mut app, "{'cmd': 'fly'}"),
            "there is no command `fly`"
        );
        assert!(refused(&mut app, "not json").starts_with("not JSON"));
        assert!(refused(
            &mut app,
            "{'cmd': 'spawn', 'components': {'mira.Transform': 3}}"
        )
        .contains("mira.Transform"));
        assert_eq!(
            app.world.entity_count(),
            1,
            "a spawn that fails leaves nothing behind"
        );
        let answer = json::parse(&respond(&mut app, r#"{"id": 12, "cmd": "status"}"#)).unwrap();
        assert_eq!(answer.field("id"), Some(&Value::Int(12)));
    }

    #[test]
    fn time_systems_and_signals_are_under_control() {
        let mut app = app();
        app.update();
        ask(&mut app, "{'cmd': 'pause'}");
        app.update();
        app.update();
        assert_eq!(app.world.resource::<Frames>().0, 1);
        let status = ask(&mut app, "{'cmd': 'status'}");
        assert_eq!(status.field("paused"), Some(&Value::Bool(true)));
        assert_eq!(status.field("frame"), Some(&Value::Int(1)));
        ask(&mut app, "{'cmd': 'step', 'frames': 2}");
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(app.world.resource::<Frames>().0, 3);
        ask(&mut app, "{'cmd': 'resume'}");
        ask(&mut app, "{'cmd': 'time_scale', 'scale': 0.5}");
        app.update();
        assert_eq!(app.world.resource::<Frames>().0, 4);
        assert_eq!(app.world.resource::<Time>().scale(), 0.5);

        let systems = ask(&mut app, "{'cmd': 'systems', 'stage': 'update'}");
        let Some(Value::List(update)) = systems.field("Update") else {
            panic!("the Update stage: {systems:?}");
        };
        assert_eq!(update.len(), 1);
        assert_eq!(update[0].field("runs"), Some(&Value::Int(4)));
        let writes = update[0].get_path("access.resource_writes.0");
        assert!(
            matches!(writes, Some(Value::Text(name)) if name.ends_with("Frames")),
            "{writes:?}"
        );
        assert!(
            matches!(ask(&mut app, "{'cmd': 'systems'}"), Value::Map(stages) if stages.len() > 2)
        );
        assert_eq!(
            ask(&mut app, "{'cmd': 'failures'}"),
            Value::List(Vec::new())
        );

        // Stepping back: record, look at what there is, go back.
        assert!(refused(&mut app, "{'cmd': 'rewind', 'frames': 1}").contains("recording"));
        let recording = ask(
            &mut app,
            "{'cmd': 'record', 'on': true, 'every': 1, 'keep': 3}",
        );
        assert_eq!(recording.field("recording"), Some(&Value::Bool(true)));
        for _ in 0..5 {
            app.update();
        }
        let Value::List(moments) = ask(&mut app, "{'cmd': 'history'}") else {
            panic!("a list of moments");
        };
        assert_eq!(moments.len(), 3, "only the last three are kept");
        let frame = app.world.resource::<Time>().frame_count();
        let back = ask(&mut app, "{'cmd': 'rewind', 'frames': 2}");
        assert_eq!(back.field("frame"), Some(&Value::Int(frame as i64 - 2)));
        assert_eq!(
            ask(&mut app, "{'cmd': 'status'}").field("paused"),
            Some(&Value::Bool(true))
        );
        ask(&mut app, "{'cmd': 'record', 'on': false}");
        ask(&mut app, "{'cmd': 'resume'}");
        app.update();

        // The signal graph: read it, add to it, rewire it, force it.
        let graph = ask(&mut app, "{'cmd': 'signals'}");
        assert_eq!(graph.get_path("0.name"), Some(&Value::Text("warm".into())));
        assert_eq!(graph.get_path("0.value"), Some(&Value::Bool(true)));
        ask(
            &mut app,
            "{'cmd': 'signal_set', 'name': 'limit', 'value': 3}",
        );
        ask(
            &mut app,
            "{'cmd': 'signal_define', 'name': 'cold', 'op': 'not', 'inputs': ['warm']}",
        );
        ask(&mut app, "{'cmd': 'signal_define', 'name': 'long', 'op': 'held_for', 'seconds': 60, 'inputs': ['warm']}");
        app.update();
        let signals = |app: &App| {
            let s = app.world.resource::<Signals>();
            (s.is_true("cold"), s.number("limit"), s.is_true("long"))
        };
        assert_eq!(signals(&app), (false, 3.0, false));
        ask(
            &mut app,
            "{'cmd': 'signal_force', 'name': 'warm', 'value': false}",
        );
        app.update();
        assert!(signals(&app).0);
        ask(&mut app, "{'cmd': 'signal_force', 'name': 'warm'}");
        ask(
            &mut app,
            "{'cmd': 'signal_connect', 'name': 'cold', 'input': 0, 'to': 'limit'}",
        );
        app.update();
        assert!(!signals(&app).0);
        // A rule can be renamed; what reads it follows, and a source can't be.
        ask(
            &mut app,
            "{'cmd': 'signal_rename', 'name': 'cold', 'to': 'chilly'}",
        );
        assert_eq!(
            refused(
                &mut app,
                "{'cmd': 'signal_rename', 'name': 'warm', 'to': 'hot'}"
            ),
            "`warm` is a source: the game names it, and reads it by that name"
        );
        ask(
            &mut app,
            "{'cmd': 'signal_rename', 'name': 'chilly', 'to': 'cold'}",
        );
        ask(&mut app, "{'cmd': 'signal_remove', 'name': 'long'}");
        assert_eq!(
            refused(
                &mut app,
                "{'cmd': 'signal_force', 'name': 'long', 'value': true}"
            ),
            "there is no signal `long`"
        );
        assert!(refused(
            &mut app,
            "{'cmd': 'signal_define', 'name': 'x', 'op': 'xor'}"
        )
        .contains("xor"));
    }

    #[test]
    fn the_signal_graph_is_drawn_as_a_tree() {
        let mut app = app();
        let signals = app.world.resource_mut::<Signals>();
        signals.set("limit", 3.0);
        signals.set("always", true);
        signals.define("ready", Op::And, ["warm", "always", "gone"]);
        signals.define("tick", Op::Not, ["tock"]);
        signals.define("tock", Op::Or, ["tick", "ready"]);
        for _ in 0..4 {
            app.update();
        }
        app.world
            .resource_mut::<Signals>()
            .force("always", Some(Signal::Bool(false)));
        app.update();
        let graph = ask(&mut app, "{'cmd': 'signals'}");
        let drawn = signals_text(&graph);
        let lines: Vec<&str> = drawn.lines().collect();
        assert_eq!(lines[0], "● limit = 3  (constant)");
        // `tick` and `tock` read each other, so nothing is above them: the first line that
        // isn't `limit` must still reach every node.
        for name in ["warm", "always", "ready", "tick", "tock", "gone"] {
            assert!(drawn.contains(name), "{name} is missing from:\n{drawn}");
        }
        assert!(
            drawn.contains("○ always = false  (constant) * !"),
            "{drawn}"
        );
        assert!(drawn.contains("? gone  (no such signal)"), "{drawn}");
        assert!(
            drawn.contains("↺ "),
            "the circle is closed, not followed for ever:\n{drawn}"
        );
        assert!(drawn.contains("├─ ") && drawn.contains("└─ "), "{drawn}");
        assert_eq!(signals_text(&Value::Null), "");
    }

    #[test]
    fn a_debugger_connects_over_a_socket() {
        let mut app = app();
        let address = app.listen_for_debugger("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // Two requests in one write, and the start of a third.
        stream
            .write_all(b"{\"cmd\": \"pause\"}\n{\"id\": 2, \"cmd\": \"status\"}\n{\"cmd\": \"res")
            .unwrap();
        let mut answers = BufReader::new(stream.try_clone().unwrap());
        let mut read = |app: &mut App| {
            // The game answers between frames; give the bytes a moment to arrive.
            let mut line = String::new();
            for _ in 0..200 {
                app.update();
                answers
                    .get_mut()
                    .set_read_timeout(Some(Duration::from_millis(20)))
                    .unwrap();
                if answers.read_line(&mut line).is_ok() && line.ends_with('\n') {
                    break;
                }
            }
            json::parse(&line).unwrap_or_else(|_| panic!("an answer, not {line:?}"))
        };
        assert_eq!(read(&mut app).field("ok"), Some(&Value::Bool(true)));
        let status = read(&mut app);
        assert_eq!(status.field("id"), Some(&Value::Int(2)));
        assert_eq!(status.get_path("ok.paused"), Some(&Value::Bool(true)));
        stream.write_all(b"ume\"}\n").unwrap();
        assert_eq!(read(&mut app).field("ok"), Some(&Value::Bool(true)));
        assert!(!app.world.resource::<Live>().is_paused());

        // Hanging up is noticed, and the game carries on.
        drop(answers);
        drop(stream);
        for _ in 0..50 {
            app.update();
            if app.debug.as_ref().unwrap().clients.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.debug.as_ref().unwrap().clients.is_empty());
    }
    #[test]
    fn a_watching_client_is_told_what_happens() {
        fn fragile(broken: Option<Res<Broken>>) {
            assert!(broken.is_none(), "it broke");
        }
        struct Broken;

        let mut app = app();
        app.world.resource_mut::<Live>().catch_failures = true;
        app.add_systems(Stage::Update, fragile);
        let address = app.listen_for_debugger("127.0.0.1:0").unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap());
        // Runs frames until a whole line has arrived, or says what did instead.
        let mut next = |app: &mut App| {
            let mut line = String::new();
            for _ in 0..300 {
                app.update();
                if lines.read_line(&mut line).is_ok() && line.ends_with('\n') {
                    return json::parse(&line).unwrap();
                }
            }
            panic!("nothing arrived; so far {line:?}");
        };
        let event = |value: &Value| match value.field("event") {
            Some(Value::Text(kind)) => kind.clone(),
            _ => panic!("not an event: {value:?}"),
        };

        stream
            .write_all(b"{\"id\": 1, \"cmd\": \"watch\"}\n")
            .unwrap();
        assert_eq!(next(&mut app).field("ok"), Some(&Value::Bool(true)));
        // `warm` comes true on its own as the frames go by: the first thing worth telling.
        let told = next(&mut app);
        assert_eq!(event(&told), "signal");
        assert_eq!(told.field("name"), Some(&Value::Text("warm".into())));
        assert_eq!(told.field("value"), Some(&Value::Bool(true)));

        // A system fails: the failure with its stack, then the pause it caused.
        app.world.insert_resource(Broken);
        let told = next(&mut app);
        assert_eq!(event(&told), "failure");
        assert_eq!(
            told.get_path("failure.message"),
            Some(&Value::Text("it broke".into()))
        );
        assert_eq!(event(&next(&mut app)), "paused");

        // Requests are still answered on the same connection, in among the news.
        app.world.remove_resource::<Broken>();
        stream.write_all(b"{\"cmd\": \"resume\"}\n").unwrap();
        assert_eq!(next(&mut app).field("ok"), Some(&Value::Bool(true)));
        assert_eq!(event(&next(&mut app)), "resumed");
        // And no more once it asks not to be told.
        stream
            .write_all(b"{\"cmd\": \"watch\", \"on\": false}\n")
            .unwrap();
        assert_eq!(next(&mut app).field("ok"), Some(&Value::Bool(false)));
        app.world.resource_mut::<Signals>().set("quiet", true);
        stream
            .write_all(b"{\"id\": 9, \"cmd\": \"status\"}\n")
            .unwrap();
        assert_eq!(
            next(&mut app).field("id"),
            Some(&Value::Int(9)),
            "the next line is the answer"
        );
        assert!(refused(&mut app, "{'cmd': 'watch'}").contains("stays open"));
    }
    #[test]
    fn the_scene_is_described_in_words() {
        use crate::render::{Camera, Material};

        let mut app = app();
        app.register_type::<Camera>().register_type::<Material>();
        let camera = app
            .world
            .spawn((Transform::from_xyz(0.0, 1.0, 0.0), Camera::default()));
        let ahead = app
            .world
            .spawn((Transform::from_xyz(0.0, 1.0, -5.0), Material::default()));
        let far = app.world.spawn(Transform::from_xyz(0.2, 1.0, -40.0));
        let aside = app.world.spawn(Transform::from_xyz(-300.0, 1.0, -5.0));
        let behind = app.world.spawn(Transform::from_xyz(0.0, 1.0, 8.0));
        let low = app.world.spawn(Transform::from_xyz(3.0, -1.0, -6.0));
        app.update();

        let Value::Text(words) = ask(&mut app, "{'cmd': 'describe'}") else {
            panic!("text");
        };
        let lines: Vec<&str> = words.lines().collect();
        assert_eq!(lines[0], "frame 1, running, 6 entities");
        assert!(
            lines[1].starts_with(&format!(
                "camera #{} at (0.0, 1.0, 0.0) looking along (0.00, 0.00, -1.00)",
                camera.to_bits()
            )),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "5 placed entities, 3 on screen");
        // On screen first, nearest first; then the rest, nearest first.
        let line = |entity: Entity| {
            let tag = format!("#{} ", entity.to_bits());
            lines
                .iter()
                .position(|line| line.starts_with(&tag))
                .unwrap_or_else(|| panic!("{tag} in {words}"))
        };
        assert_eq!(
            [line(ahead), line(low), line(far), line(behind), line(aside)],
            [3, 4, 5, 6, 7],
            "{words}"
        );
        assert_eq!(
            lines[3],
            format!(
                "#{} at (0.0, 1.0, -5.0), 5.0 away, on screen, middle centre: Material",
                ahead.to_bits()
            )
        );
        assert!(
            lines[4].contains("on screen, bottom right: nothing registered"),
            "{}",
            lines[4]
        );
        assert!(
            lines[6].contains("behind the camera") && lines[7].contains("out of view to the left"),
            "{words}"
        );

        // A long scene is cut short, and says so; with no camera, it still says what is where.
        let Value::Text(short) = ask(&mut app, "{'cmd': 'describe', 'limit': 2}") else {
            panic!("text");
        };
        assert!(
            short.ends_with("… and 3 more (raise `limit` to list them)\n"),
            "{short}"
        );
        app.world.despawn(camera);
        let Value::Text(blind) = ask(&mut app, "{'cmd': 'describe'}") else {
            panic!("text");
        };
        assert!(
            blind.contains("no active camera") && blind.contains("not drawn"),
            "{blind}"
        );
    }
    #[test]
    fn state_that_cannot_be_seen_is_reported() {
        struct Secret(#[allow(dead_code)] u32);
        impl crate::ecs::Component for Secret {}
        struct Hidden;

        let mut app = app();
        app.world.spawn((Transform::IDENTITY, Secret(1)));
        app.world.spawn(Secret(2));
        app.world.insert_resource(Hidden);
        app.update();
        let report = ask(&mut app, "{'cmd': 'unregistered'}");
        let Some(Value::List(components)) = report.field("components") else {
            panic!("components: {report:?}");
        };
        let named = |part: &str| {
            components.iter().find(|component| {
                matches!(component.field("type"), Some(Value::Text(name)) if name.ends_with(part))
            })
        };
        assert_eq!(
            named("::Secret").unwrap().field("entities"),
            Some(&Value::Int(2))
        );
        assert!(
            named("::Transform").is_none(),
            "registered types are not listed"
        );
        assert!(
            named("::GlobalTransform").is_some(),
            "the engine's own gaps show too"
        );
        let Some(Value::List(resources)) = report.field("resources") else {
            panic!("resources");
        };
        let has = |part: &str| {
            resources
                .iter()
                .any(|name| matches!(name, Value::Text(name) if name.ends_with(part)))
        };
        assert!(has("::Hidden") && has("Frames"));
        assert!(!has("Fog"), "a registered resource is not listed");
    }
    #[test]
    fn the_profile_says_where_the_time_goes() {
        let mut app = app();
        app.add_systems(Stage::Update, |_: Res<Frames>| {
            std::thread::sleep(Duration::from_millis(3));
        });
        for _ in 0..5 {
            app.update();
        }
        let profile = ask(&mut app, "{'cmd': 'profile', 'systems': 2}");
        assert_eq!(profile.get_path("frames.measured"), Some(&Value::Int(5)));
        let number = |path: &str| profile.get_path(path).and_then(Value::as_f64).unwrap();
        assert!(
            number("frames.mean_millis") >= 3.0
                && number("frames.worst_millis") >= number("frames.mean_millis")
        );
        assert!(number("frames.could_reach_fps") < 334.0);
        assert!(number("stages.Update") >= 3.0, "{profile:?}");
        assert!(number("stages.Update") <= number("frames.worst_millis"));
        // The costliest system first, and only as many as asked for.
        let Some(Value::List(systems)) = profile.field("systems") else {
            panic!("systems");
        };
        assert_eq!(systems.len(), 2);
        assert_eq!(
            systems[0].field("stage"),
            Some(&Value::Text("Update".into()))
        );
        assert!(
            systems[0]
                .field("last_millis")
                .and_then(Value::as_f64)
                .unwrap()
                >= 3.0
        );
        assert_eq!(systems[0].field("runs"), Some(&Value::Int(5)));
    }
}
