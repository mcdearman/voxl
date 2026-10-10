//! The engine's log: printed as before, and also kept, so that it can be read from inside
//! the program (the app's Log panel, an agent asking what the game said).

use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

/// One thing that was logged.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Counts up from the first line, so that a reader can ask for what is new.
    pub number: u64,
    pub level: log::Level,
    /// Where it came from: the module that logged it.
    pub from: String,
    pub text: String,
}

/// How many lines are kept.
pub const KEPT: usize = 2000;

struct Kept {
    lines: VecDeque<Line>,
    next: u64,
}

fn kept() -> &'static Mutex<Kept> {
    static KEPT_LINES: OnceLock<Mutex<Kept>> = OnceLock::new();
    KEPT_LINES.get_or_init(|| {
        Mutex::new(Kept {
            lines: VecDeque::new(),
            next: 0,
        })
    })
}

fn keep(level: log::Level, from: &str, text: String) {
    let mut kept = kept().lock().unwrap_or_else(|p| p.into_inner());
    let number = kept.next;
    kept.next += 1;
    kept.lines.push_back(Line {
        number,
        level,
        from: from.to_owned(),
        text,
    });
    while kept.lines.len() > KEPT {
        kept.lines.pop_front();
    }
}

/// The lines kept, oldest first; with `after`, only those numbered higher.
pub fn lines(after: Option<u64>) -> Vec<Line> {
    let kept = kept().lock().unwrap_or_else(|p| p.into_inner());
    kept.lines
        .iter()
        .filter(|line| after.is_none_or(|after| line.number > after))
        .cloned()
        .collect()
}

/// Forgets the lines kept. Their numbers go on from where they were.
pub fn clear() {
    kept()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .lines
        .clear();
}

/// Prints through the usual logger, and keeps what it prints.
struct Both(env_logger::Logger);

impl log::Log for Both {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.0.matches(record) {
            keep(record.level(), record.target(), record.args().to_string());
            self.0.log(record);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// Starts logging: to the terminal as `RUST_LOG` says (info and above unless it says
/// otherwise), and into what [`lines`] reads. Does nothing if a logger is already set.
pub fn start() {
    let logger = env_logger::Builder::from_env(env_logger::Env::default())
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .build();
    let most = logger.filter();
    if log::set_boxed_logger(Box::new(Both(logger))).is_ok() {
        log::set_max_level(most);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_logged_is_kept_and_numbered() {
        // Other tests log too, so only this test's own lines are looked at.
        let mine = |lines: Vec<Line>| -> Vec<(log::Level, String)> {
            lines
                .into_iter()
                .filter(|line| line.from == "logging-test")
                .map(|line| (line.level, line.text))
                .collect()
        };
        keep(log::Level::Info, "logging-test", "started".into());
        let so_far = lines(None);
        let last = so_far.last().expect("the line just kept").number;
        keep(log::Level::Warn, "logging-test", "low on memory".into());
        assert_eq!(
            mine(lines(None)),
            [
                (log::Level::Info, "started".to_owned()),
                (log::Level::Warn, "low on memory".to_owned())
            ]
        );
        // Only what is new since a line already read.
        assert_eq!(
            mine(lines(Some(last))),
            [(log::Level::Warn, "low on memory".to_owned())]
        );
        // No more than so many are kept, the oldest going first.
        for n in 0..KEPT + 5 {
            keep(log::Level::Debug, "logging-test-flood", n.to_string());
        }
        let all = lines(None);
        assert_eq!(all.len(), KEPT);
        assert_eq!(all.last().unwrap().text, (KEPT + 4).to_string());
        assert!(mine(all).is_empty(), "the first lines have gone");
    }
}
