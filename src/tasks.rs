use std::{
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

type Job = Box<dyn FnOnce() + Send>;

/// A fixed set of worker threads for background work such as chunk generation and meshing.
pub struct TaskPool {
    jobs: Sender<Job>,
    threads: usize,
}

impl Default for TaskPool {
    fn default() -> Self {
        // Leave a core for the main thread.
        let cores = thread::available_parallelism().map_or(4, |n| n.get());
        Self::new(cores.saturating_sub(1).clamp(1, 8))
    }
}

impl TaskPool {
    pub fn new(threads: usize) -> Self {
        let (jobs, receiver) = channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for i in 0..threads {
            let receiver = receiver.clone();
            thread::Builder::new()
                .name(format!("mira worker {i}"))
                .spawn(move || loop {
                    // The guard is a temporary, so the lock is released before the job runs.
                    let job = receiver.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break, // the pool was dropped
                    }
                })
                .expect("failed to spawn worker thread");
        }
        Self { jobs, threads }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Runs `work` on a worker and delivers its result to `results`.
    pub fn spawn<T: Send + 'static>(
        &self,
        results: &Sender<T>,
        work: impl FnOnce() -> T + Send + 'static,
    ) {
        let results = results.clone();
        let job: Job = Box::new(move || {
            // The receiver is gone if the app is shutting down; nothing to do about it.
            let _ = results.send(work());
        });
        self.jobs.send(job).expect("worker threads are gone");
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
