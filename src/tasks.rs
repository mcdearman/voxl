use std::sync::{
    mpsc::{channel, Receiver, Sender},
    Mutex,
};

/// Work done in the background, such as reading files, chunk generation and meshing, on
/// the threads the whole engine shares ([`Pool`](crate::ecs::Pool)): the same ones systems
/// run on, which take a frame's work first and leave a worker free for it. A resource, and
/// only a handle: every `TaskPool` hands its work to the same threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct TaskPool;

impl TaskPool {
    /// A handle to the shared threads. The number is not used: how much is done at once is
    /// settled for the whole engine, not asked for by each part of it.
    pub fn new(_threads: usize) -> Self {
        Self
    }

    /// How many background jobs are done at once.
    pub fn threads(&self) -> usize {
        crate::ecs::Pool::global().background_threads()
    }

    /// Runs `work` in the background and delivers its result to `results`.
    pub fn spawn<T: Send + 'static>(
        &self,
        results: &Sender<T>,
        work: impl FnOnce() -> T + Send + 'static,
    ) {
        let results = results.clone();
        crate::ecs::Pool::global().spawn(move || {
            // The receiver is gone if the app is shutting down; nothing to do about it.
            let _ = results.send(work());
        });
    }
}

/// Both ends of a result channel, for storing in a resource.
pub struct Mailbox<T> {
    pub sender: Sender<T>,
    /// Behind a lock so that a resource holding a mailbox can be shared between threads; a
    /// receiver alone can only be moved between them.
    receiver: Mutex<Receiver<T>>,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver: Mutex::new(receiver),
        }
    }
}

impl<T> Mailbox<T> {
    fn receiver(&self) -> std::sync::MutexGuard<'_, Receiver<T>> {
        self.receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A result that has arrived, if one has.
    pub fn try_recv(&self) -> Result<T, std::sync::mpsc::TryRecvError> {
        self.receiver().try_recv()
    }

    /// Waits for the next result.
    pub fn recv(&self) -> Result<T, std::sync::mpsc::RecvError> {
        self.receiver().recv()
    }
}
