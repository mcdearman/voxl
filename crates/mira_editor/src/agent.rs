//! The agent the app talks to: a program of its own, which the window asks and listens to.
//!
//! The window knows only this much of an agent: it can be asked something, it says things
//! back over a while, and it can be told to stop. What the agent is, and how it reaches the
//! game (through mira's own tools, served for the game that is running in the window), is
//! behind [`Agent`].

use std::sync::mpsc::Sender;

/// What an agent says back while it works on what it was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// More of its answer, to go on the end of what it has said so far.
    Text(String),
    /// Something it did, in a line: a tool it used.
    Did(String),
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
