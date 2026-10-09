//! Catching a system's panic with its stack, so a failure can stop the game where it is
//! instead of ending it.

use std::{
    any::Any,
    backtrace::Backtrace,
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Once,
};

/// What a caught panic said and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Caught {
    pub message: String,
    /// `file:line:column` of the panic, if known.
    pub location: String,
    /// The stack from the panic down to the system that was running, one frame per line.
    pub stack: String,
}

thread_local! {
    static GUARDED: Cell<bool> = const { Cell::new(false) };
    static SEEN: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// Whether code on this thread is running inside [`catch`].
pub fn active() -> bool {
    GUARDED.with(Cell::get)
}

/// Installs a panic hook that, inside `catch`, records where the panic happened instead of
/// printing it. Everywhere else panics are reported as before.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !active() {
                return previous(info);
            }
            let location = info.location().map_or(String::new(), |l| l.to_string());
            // Miri keeps programs away from the file system, which printing a backtrace asks
            // about; the stack is left out there.
            let stack = if cfg!(miri) {
                String::new()
            } else {
                trim(&Backtrace::force_capture().to_string())
            };
            SEEN.with(|seen| *seen.borrow_mut() = Some((location, stack)));
        }));
    });
}

/// Keeps the frames between the panic machinery above and `catch` below: the part of the
/// stack that is about the code that failed.
fn trim(stack: &str) -> String {
    // A frame is its numbered line and the `at file:line` lines after it.
    let is_frame = |line: &str| {
        line.trim_start()
            .split_once(": ")
            .is_some_and(|(n, _)| n.parse::<u32>().is_ok())
    };
    let mut frames: Vec<Vec<&str>> = Vec::new();
    for line in stack.lines() {
        match frames.last_mut() {
            Some(frame) if !is_frame(line) => frame.push(line),
            _ => frames.push(vec![line]),
        }
    }
    let names = |frame: &Vec<&str>, any: &[&str]| any.iter().any(|name| frame[0].contains(name));
    let end = frames
        .iter()
        .position(|frame| names(frame, &["ecs::guard::catch"]))
        .unwrap_or(frames.len());
    let panicking = [
        "rust_begin_unwind",
        "panic_handler",
        "panic_with_hook",
        "begin_panic",
        "core::panicking::",
    ];
    let start = frames[..end]
        .iter()
        .rposition(|frame| names(frame, &panicking))
        .map_or(0, |index| index + 1);
    let mut kept = &frames[start..end];
    // Between the system and `catch` there is only the machinery that catches.
    let catching = [
        "std::panicking::",
        "std::panic::catch_unwind",
        "__rust_try",
        "AssertUnwindSafe",
    ];
    while kept.last().is_some_and(|frame| names(frame, &catching)) {
        kept = &kept[..kept.len() - 1];
    }
    kept.iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_of(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with a value that isn't text".to_owned()
    }
}

/// Fails the system that is running, as a panic would, with a message and a stack that came
/// from somewhere a panic can't cross: code in a plugin. Outside [`catch`] this is a panic.
pub fn raise(message: String, stack: String) -> ! {
    if !active() {
        panic!("{message}\n{stack}");
    }
    SEEN.with(|seen| *seen.borrow_mut() = Some((String::new(), stack)));
    // Not `panic!`: the hook would record this function as where it happened.
    std::panic::resume_unwind(Box::new(message))
}

/// Runs `body`. If it panics, the panic stops here and is returned with its stack.
pub fn catch<R>(body: impl FnOnce() -> R) -> Result<R, Caught> {
    install_hook();
    SEEN.with(|seen| *seen.borrow_mut() = None);
    let outer = GUARDED.with(|guarded| guarded.replace(true));
    let result = catch_unwind(AssertUnwindSafe(body));
    GUARDED.with(|guarded| guarded.set(outer));
    result.map_err(|payload| {
        let (location, stack) = SEEN
            .with(|seen| seen.borrow_mut().take())
            .unwrap_or_default();
        Caught {
            message: message_of(payload.as_ref()),
            location,
            stack,
        }
    })
}
