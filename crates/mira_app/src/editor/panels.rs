//! The panels that show what the engine already knows of a running game: its log, its
//! timings, its systems, its settings, what has been changed and what has gone wrong.
//!
//! Each is a function of the app as it stands, read straight from the game; only the panels
//! in front are built, so one that is shut costs nothing.

use std::time::Duration;

use neo::prelude::*;

use super::{fields_of, short, Editor, Message, AGENT, GAME};
use mira::{
    app::Stage,
    live::{FrameStats, History, Live},
    logging,
    plugin::NativePlugins,
    reflect::{json, TypeRegistry, Value},
    time::Time,
};

/// How many of the log's last lines the Log panel shows.
const LOG_SHOWN: usize = 400;

fn milliseconds(time: Duration) -> f32 {
    time.as_secs_f32() * 1000.0
}

/// A small heading over a part of a panel.
fn heading(said: impl Into<String>) -> Element<Message> {
    text(said)
        .size(12.0)
        .weight(Weight::SEMIBOLD)
        .tone(Tone::Accent)
        .into()
}

/// A panel's contents, scrolling, a little in from its edges.
fn page(rows: Column<Message>) -> Element<Message> {
    scrollable(container(rows.width(Length::Fill)).padding(10.0)).into()
}

/// What a panel says when it has nothing to show.
fn nothing(glyph: neo::theme::Icon, said: &str) -> Element<Message> {
    container(
        column()
            .spacing(6.0)
            .align(Align::Center)
            .push(icon(glyph).size(24.0).tone(Tone::Faint))
            .push(text(said).tone(Tone::Muted)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Align::Center)
    .align_y(Align::Center)
    .into()
}

/// A name on the left and what there is to say of it on the right, in small fixed type.
fn pair(name: impl Into<String>, said: impl Into<String>) -> Element<Message> {
    row()
        .spacing(8.0)
        .align(Align::Center)
        .push(text(name).mono().size(12.5).no_wrap())
        .push(Space::fill_x())
        .push(text(said).mono().size(12.5).tone(Tone::Muted))
        .into()
}

/// A console command as it is typed, `pause` or `entities with=mira.Camera`, as the request
/// the debug connection takes. Values are read as JSON where they are JSON (numbers, true,
/// lists) and as words otherwise.
pub(super) fn request(typed: &str) -> Option<Value> {
    let mut words = typed.split_whitespace();
    let mut request = vec![("cmd".to_owned(), Value::Text(words.next()?.to_owned()))];
    for word in words {
        let (name, value) = word.split_once('=')?;
        let value = json::parse(value).unwrap_or_else(|_| Value::Text(value.to_owned()));
        request.push((name.to_owned(), value));
    }
    Some(Value::Map(request))
}

impl Editor {
    /// Opens a panel that is shut, beside the agent's (or the game's); shuts one that is
    /// open. The game's own panel stays.
    pub(super) fn toggle_panel(&mut self, panel: &str) {
        if panel == GAME {
            return;
        }
        let layout = self.layout.clone();
        let next = if layout.contains(panel) {
            layout.without(panel)
        } else {
            let beside = if layout.contains(AGENT) { AGENT } else { GAME };
            Some(layout.with(panel, beside, Side::Middle))
        };
        if let Some(next) = next {
            self.update(Message::Arranged(next));
        }
    }

    /// Runs what is typed in the console against the game, and keeps the answer.
    pub(super) fn run_command(&mut self) {
        let typed = std::mem::take(&mut self.command);
        let typed = typed.trim();
        if typed.is_empty() {
            return;
        }
        let answer = match request(typed) {
            Some(request) => {
                let line = mira::remote::respond(&mut self.game, &json::to_line(&request));
                match json::parse(&line) {
                    Ok(answer) => match (answer.field("ok"), answer.field("error")) {
                        (Some(ok), _) => json::to_string(ok),
                        (_, Some(Value::Text(why))) => format!("refused: {why}"),
                        _ => line,
                    },
                    Err(_) => line,
                }
            }
            None => "A command is a word, then name=value for each thing it takes.".to_owned(),
        };
        self.asked.push((typed.to_owned(), answer));
        // Enough to look back over; not the whole session.
        let over = self.asked.len().saturating_sub(50);
        self.asked.drain(..over);
        self.lists = super::Lists::of(&self.game, self.chosen);
    }

    /// What the game and the engine have logged.
    pub(super) fn log_panel(&self) -> Element<Message> {
        let wanted = self.log_filter.to_lowercase();
        let lines = logging::lines(None);
        let from = lines.len().saturating_sub(LOG_SHOWN);
        let mut rows = column().spacing(1.0).width(Length::Fill);
        let mut shown = 0;
        for line in &lines[from..] {
            if !wanted.is_empty()
                && !line.text.to_lowercase().contains(&wanted)
                && !line.from.to_lowercase().contains(&wanted)
            {
                continue;
            }
            shown += 1;
            let tone = match line.level {
                log::Level::Error => Tone::Bad,
                log::Level::Warn => Tone::Warn,
                log::Level::Info => Tone::Inherit,
                _ => Tone::Muted,
            };
            rows = rows.push(
                row()
                    .spacing(8.0)
                    .push(
                        text(format!("{:<5}", line.level.as_str().to_lowercase()))
                            .mono()
                            .size(12.0)
                            .tone(tone),
                    )
                    .push(text(line.text.clone()).mono().size(12.0).tone(tone))
                    .push(Space::fill_x())
                    .push(
                        text(short_module(&line.from))
                            .mono()
                            .size(11.0)
                            .tone(Tone::Faint),
                    ),
            );
        }
        let said: Element<Message> = if shown == 0 {
            nothing(
                icons::LIST_TREE,
                "Nothing logged, or nothing with that in it.",
            )
        } else {
            scrollable(container(rows).padding(8.0))
                .follow_end(true)
                .into()
        };
        column()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(container(said).height(Length::Fill))
            .push(
                container(
                    text_input("Show only lines with…", self.log_filter.clone())
                        .on_input(Message::LogFilter)
                        .width(Length::Fill),
                )
                .padding([8.0, 6.0]),
            )
            .into()
    }

    /// A line to tell the game anything the debug connection understands.
    pub(super) fn console_panel(&self) -> Element<Message> {
        let mut rows = column().spacing(8.0).width(Length::Fill);
        for (asked, answer) in &self.asked {
            rows = rows
                .push(
                    text(format!("› {asked}"))
                        .mono()
                        .size(12.5)
                        .tone(Tone::Accent),
                )
                .push(text(answer.clone()).mono().size(12.0).tone(Tone::Muted));
        }
        let said: Element<Message> = if self.asked.is_empty() {
            nothing(
                icons::TYPE,
                "pause  ·  entities with=mira.Camera  ·  signal_set name=open value=true  ·  describe",
            )
        } else {
            scrollable(container(rows).padding(8.0))
                .follow_end(true)
                .into()
        };
        column()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(container(said).height(Length::Fill))
            .push(
                container(
                    text_input("A command, then name=value…", self.command.clone())
                        .on_input(Message::Command)
                        .on_submit(Message::Run)
                        .width(Length::Fill),
                )
                .padding([8.0, 6.0]),
            )
            .into()
    }

    /// Where the time of a frame goes.
    pub(super) fn profiler_panel(&self) -> Element<Message> {
        let Some(stats) = self.game.world.get_resource::<FrameStats>() else {
            return nothing(icons::ACTIVITY, "This game keeps no timings.");
        };
        let frames: Vec<f32> = stats.frames().map(milliseconds).collect();
        let (mean, worst) = (milliseconds(stats.mean()), milliseconds(stats.worst()));
        let mut rows = column().spacing(6.0);
        rows = rows
            .push(heading("A frame"))
            .push(
                container(sparkline(frames, 0.0, (worst * 1.1).max(1.0)))
                    .width(Length::Fill)
                    .height(54.0),
            )
            .push(pair("usual", format!("{mean:.2} ms")))
            .push(pair("worst of late", format!("{worst:.2} ms")))
            .push(pair(
                "could run at",
                format!("{:.0} frames a second", 1000.0 / mean.max(0.001)),
            ))
            .push(heading("By stage"));
        for (stage, took) in stats.stages() {
            rows = rows.push(pair(
                format!("{stage:?}"),
                format!("{:.3} ms", milliseconds(*took)),
            ));
        }
        // Every system's last run, the slowest first.
        let mut systems: Vec<(String, Duration)> = Stage::of_a_frame()
            .into_iter()
            .flat_map(|stage| self.game.systems(stage))
            .map(|system| (system.name, system.stats.last))
            .collect();
        systems.sort_by_key(|system| std::cmp::Reverse(system.1));
        rows = rows.push(heading("Slowest systems"));
        for (name, took) in systems.into_iter().take(16) {
            rows = rows.push(pair(
                short_path(&name),
                format!("{:.3} ms", milliseconds(took)),
            ));
        }
        page(rows)
    }

    /// Every system, in the order and the batches it runs in.
    pub(super) fn systems_panel(&self) -> Element<Message> {
        let mut rows = column().spacing(4.0);
        for stage in Stage::of_a_frame() {
            let systems = self.game.systems(stage);
            if systems.is_empty() {
                continue;
            }
            rows = rows.push(heading(format!("{stage:?}  ·  {}", systems.len())));
            for system in systems {
                // Which batch it runs in, and how much it touches.
                let batch = system
                    .batch
                    .map_or("·".to_owned(), |batch| batch.to_string());
                let touches = match &system.access {
                    Some(access) => format!(
                        "reads {}  writes {}",
                        access.reads.len() + access.resource_reads.len(),
                        access.writes.len() + access.resource_writes.len()
                    ),
                    None => "the whole world".to_owned(),
                };
                let name = text(format!("{batch:>2}  {}", short_path(&system.name)))
                    .mono()
                    .size(12.5)
                    .no_wrap();
                rows = rows.push(
                    row()
                        .spacing(8.0)
                        .push(if system.suspended {
                            name.tone(Tone::Bad)
                        } else {
                            name
                        })
                        .push(Space::fill_x())
                        .push(text(touches).mono().size(11.5).tone(Tone::Faint)),
                );
            }
        }
        page(rows)
    }

    /// The world's settings: every registered resource, each field a control.
    pub(super) fn world_panel(&self) -> Element<Message> {
        let world = &self.game.world;
        let Some(registry) = world.get_resource::<TypeRegistry>() else {
            return nothing(
                icons::SLIDERS_HORIZONTAL,
                "This game has no settings registered.",
            );
        };
        let mut rows = column().spacing(6.0);
        let mut any = false;
        for setting in registry.resources() {
            let Some(value) = (setting.get)(world) else {
                continue;
            };
            if any {
                rows = rows.push(container(Divider::horizontal()).padding([0.0, 4.0]));
            }
            any = true;
            rows = rows.push(heading(short(setting.name)));
            rows = fields_of(rows, setting.name, true, &mut Vec::new(), &value);
        }
        if !any {
            return nothing(
                icons::SLIDERS_HORIZONTAL,
                "This game has no settings registered.",
            );
        }
        page(rows)
    }

    /// What has been changed from the app, to step back and forward through.
    pub(super) fn history_panel(&self) -> Element<Message> {
        if self.done.is_empty() && self.undone.is_empty() {
            return nothing(icons::UNDO_2, "Nothing has been changed yet.");
        }
        let said = |change: &super::Change| {
            let whose = match change.entity {
                Some(entity) => self
                    .lists
                    .names
                    .iter()
                    .find(|(named, _)| *named == entity)
                    .map_or(format!("entity {}", entity.index()), |(_, name)| {
                        name.clone()
                    }),
                None => "the world".to_owned(),
            };
            let what = match change.path.last() {
                Some(field) => format!("{} {field}", short(&change.component)),
                None => short(&change.component).to_owned(),
            };
            format!("{what} of {whose}")
        };
        let mut rows = column().spacing(2.0);
        rows = rows.push(
            button("As it was")
                .kind(ButtonKind::Ghost)
                .on_press(Message::Jump(0)),
        );
        // What stands, oldest first; then what was taken back, nearest first.
        for (at, change) in self.done.iter().enumerate() {
            let standing = at + 1;
            rows = rows.push(
                button(said(change))
                    .kind(ButtonKind::Ghost)
                    .selected(standing == self.done.len())
                    .on_press(Message::Jump(standing)),
            );
        }
        for (back, change) in self.undone.iter().rev().enumerate() {
            let standing = self.done.len() + back + 1;
            rows = rows.push(
                Button::new(text(said(change)).tone(Tone::Faint))
                    .kind(ButtonKind::Ghost)
                    .on_press(Message::Jump(standing)),
            );
        }
        page(rows)
    }

    /// The game's time: how fast it runs, and going back.
    pub(super) fn time_panel(&self) -> Element<Message> {
        let world = &self.game.world;
        let (Some(time), Some(history)) = (
            world.get_resource::<Time>(),
            world.get_resource::<History>(),
        ) else {
            return nothing(icons::SKIP_FORWARD, "This game keeps no time.");
        };
        let moments = history.moments();
        let mut rows = column().spacing(8.0);
        rows = rows
            .push(heading("Speed"))
            .push(pair(
                "game time against the clock",
                format!("{:.2}×", time.scale()),
            ))
            .push(slider(0.0..=2.0, time.scale(), Message::Speed))
            .push(heading("Going back"))
            .push(
                row()
                    .spacing(10.0)
                    .align(Align::Center)
                    .push(toggle(history.recording, Message::Record))
                    .push(text("Keep moments to go back to").size(13.0)),
            )
            .push(pair("kept", format!("{} moments", moments.len())));
        if let (Some(first), Some(last)) = (moments.first(), moments.last()) {
            rows = rows.push(pair(
                "from",
                format!("{:.1} s to {:.1} s", first.seconds, last.seconds),
            ));
        }
        let back = |label: &str, frames: u64| {
            button(label).on_press_maybe((!moments.is_empty()).then_some(Message::Rewind(frames)))
        };
        rows = rows.push(
            row()
                .spacing(6.0)
                .push(back("Back 1 s", 60))
                .push(back("Back 5 s", 300))
                .push(back("To the first", u64::MAX / 2)),
        );
        rows = rows.push(
            text("Going back pauses the game there. Only what is registered is put back.")
                .size(12.0)
                .tone(Tone::Faint),
        );
        page(rows)
    }

    /// The systems that failed, and what they said.
    pub(super) fn failures_panel(&self) -> Element<Message> {
        let failures = self
            .game
            .world
            .get_resource::<Live>()
            .map_or(&[][..], Live::failures);
        if failures.is_empty() {
            return nothing(icons::ZAP, "No system has failed.");
        }
        let mut rows = column().spacing(8.0);
        rows = rows.push(
            button("Forget these and go on")
                .kind(ButtonKind::Accent)
                .on_press(Message::Forgive),
        );
        for failure in failures.iter().rev() {
            rows = rows
                .push(container(Divider::horizontal()).padding([0.0, 2.0]))
                .push(
                    text(short_path(&failure.system))
                        .weight(Weight::SEMIBOLD)
                        .tone(Tone::Bad),
                )
                .push(text(failure.message.clone()))
                .push(
                    text(format!(
                        "{}  ·  {:?}, frame {}",
                        failure.location, failure.stage, failure.frame
                    ))
                    .mono()
                    .size(12.0)
                    .tone(Tone::Muted),
                )
                .push(
                    text(failure.stack.clone())
                        .mono()
                        .size(11.5)
                        .tone(Tone::Faint),
                );
        }
        page(rows)
    }

    /// The plugins the game is made of.
    pub(super) fn plugins_panel(&self) -> Element<Message> {
        let loaded: Vec<(String, u32)> =
            self.game
                .world
                .get_resource::<NativePlugins>()
                .map_or(Vec::new(), |plugins| {
                    plugins
                        .loaded()
                        .map(|(name, times)| (name.to_owned(), times))
                        .collect()
                });
        if loaded.is_empty() {
            return nothing(
                icons::LAYERS,
                "No plugins: this game is built into the program.",
            );
        }
        let mut rows = column().spacing(6.0);
        rows = rows.push(button("Reload what has changed").on_press(Message::Reload));
        for (name, times) in loaded {
            rows = rows.push(pair(name, format!("loaded {times} times")));
        }
        page(rows)
    }

    /// How much of everything there is.
    pub(super) fn statistics_panel(&self) -> Element<Message> {
        let world = &self.game.world;
        let mut rows = column().spacing(4.0);
        rows = rows
            .push(heading("The world"))
            .push(pair("entities", world.entity_count().to_string()))
            .push(pair("signals", self.lists.signals.len().to_string()))
            .push(pair(
                "log lines kept",
                logging::lines(None).len().to_string(),
            ));
        if let Some(time) = world.get_resource::<Time>() {
            rows = rows
                .push(pair("frames", time.frame_count().to_string()))
                .push(pair("game time", format!("{:.1} s", time.elapsed_secs())));
        }
        let systems: usize = Stage::of_a_frame()
            .into_iter()
            .map(|stage| self.game.systems(stage).len())
            .sum();
        rows = rows.push(pair("systems in a frame", systems.to_string()));
        if let Some(registry) = world.get_resource::<TypeRegistry>() {
            // How many entities have each registered component, the commonest first.
            let mut counts: Vec<(&str, usize)> = registry
                .iter()
                .map(|kind| (short(kind.name), (kind.entities)(world).len()))
                .filter(|(_, count)| *count > 0)
                .collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            rows = rows.push(heading("Components"));
            for (name, count) in counts {
                rows = rows.push(pair(name, count.to_string()));
            }
        }
        page(rows)
    }
}

/// A module's name without the crate it is in: `mira::render::environment` is
/// `render::environment`.
fn short_module(module: &str) -> String {
    module
        .split_once("::")
        .map_or(module, |(_, rest)| rest)
        .to_owned()
}

/// A system's name without the path to it: the function and the module it is in, and for
/// one made for a type, the type by its own name. `mira::ecs::event::update<mira::app::Quit>`
/// is `event::update<Quit>`.
pub(super) fn short_path(path: &str) -> String {
    let (name, made_for) = match path.split_once('<') {
        Some((name, made_for)) => (name, Some(made_for.trim_end_matches('>'))),
        None => (path, None),
    };
    let parts: Vec<&str> = name.split("::").collect();
    let name = match parts.len() {
        0 | 1 => name.to_owned(),
        n => format!("{}::{}", parts[n - 2], parts[n - 1]),
    };
    match made_for {
        Some(made_for) => format!(
            "{name}<{}>",
            short(made_for.rsplit("::").next().unwrap_or(made_for))
        ),
        None => name,
    }
}
