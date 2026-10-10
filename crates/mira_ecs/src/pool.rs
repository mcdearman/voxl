//! The threads everything shares: the systems of a batch that runs at the same moment, the
//! shares of a query's work, and work in the background such as reading files and building
//! terrain.
//!
//! There are two queues. Work of the frame (systems, a query's shares) is always taken
//! first, and a thread waiting for its own jobs helps with it. Background work is taken when
//! there is none, by all the workers but one, so however much there is to load a worker is
//! left for the frame.

use std::{
    collections::VecDeque,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
    },
    thread,
};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Counts jobs still running, and wakes whoever is waiting for none.
#[derive(Default)]
struct Latch {
    running: Mutex<usize>,
    done: Condvar,
}

impl Latch {
    fn finished(&self) -> bool {
        *self.running.lock().unwrap_or_else(|p| p.into_inner()) == 0
    }

    /// Waits a moment for the count to reach none, or for the time to pass.
    fn wait_a_moment(&self) {
        let running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if *running > 0 {
            let moment = std::time::Duration::from_micros(200);
            drop(self.done.wait_timeout(running, moment));
        }
    }
}

/// What is waiting to be done.
#[derive(Default)]
struct Queues {
    frame: VecDeque<Job>,
    background: VecDeque<Job>,
    /// How many background jobs are being done now.
    background_running: usize,
}

struct Shared {
    queues: Mutex<Queues>,
    /// Told when there is something new to take, or room for more background work.
    more: Condvar,
    /// The most background jobs done at once.
    background_most: usize,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A worker's life: frame work first, then background work while there is room for it.
    fn work(&self, frame_too: bool) {
        loop {
            let (job, background) = {
                let mut queues = self.lock();
                loop {
                    if let Some(job) = frame_too.then(|| queues.frame.pop_front()).flatten() {
                        break (job, false);
                    }
                    if queues.background_running < self.background_most {
                        if let Some(job) = queues.background.pop_front() {
                            queues.background_running += 1;
                            break (job, true);
                        }
                    }
                    queues = self.more.wait(queues).unwrap_or_else(|p| p.into_inner());
                }
            };
            if background {
                // Nobody waits for a background job, so a panic in one ends the job and not
                // the thread; what was waiting for its result goes without.
                if catch_unwind(AssertUnwindSafe(job)).is_err() {
                    eprintln!("a background job panicked");
                }
                self.lock().background_running -= 1;
                self.more.notify_all();
            } else {
                job();
            }
        }
    }
}

/// Worker threads that live as long as the program.
pub struct Pool {
    shared: Arc<Shared>,
    workers: usize,
}

impl Pool {
    fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            queues: Mutex::default(),
            more: Condvar::new(),
            // One worker is left for the frame, where there is more than one.
            background_most: workers.saturating_sub(1).max(1),
        });
        let start = |name: String, frame_too: bool| {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name(name)
                .spawn(move || shared.work(frame_too))
                .expect("can't start a worker thread");
        };
        for index in 0..workers {
            start(format!("mira worker {index}"), true);
        }
        // With no workers the frame's work is all done by the calling thread, but work in
        // the background still needs somewhere to be done.
        if workers == 0 {
            start("mira background".to_owned(), false);
        }
        Self { shared, workers }
    }

    /// The pool every schedule shares. It has one thread fewer than the machine has cores
    /// (the calling thread works too), at most 7, or `MIRA_THREADS` minus one if that is set:
    /// `MIRA_THREADS=1` means no workers, and everything runs on the calling thread.
    pub fn global() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(|| {
            let threads = std::env::var("MIRA_THREADS")
                .ok()
                .and_then(|threads| threads.parse::<usize>().ok())
                .unwrap_or_else(|| {
                    thread::available_parallelism()
                        .map_or(1, |n| n.get())
                        .min(8)
                });
            // Miri reports one core; give it workers so that it checks what they do.
            let threads = if cfg!(miri) { 4 } else { threads };
            Pool::new(threads.saturating_sub(1))
        })
    }

    /// How many worker threads there are, not counting the caller.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// How many background jobs are done at once.
    pub fn background_threads(&self) -> usize {
        self.shared.background_most
    }

    /// Has `job` done in the background, some time, on one of the workers: for work nobody
    /// waits for, such as reading a file or building a piece of terrain. It gives way to
    /// the frame's work, and hands its result back however the caller arranged (a channel).
    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        self.shared.lock().background.push_back(Box::new(job));
        self.shared.more.notify_all();
    }

    /// Waits until the latch's jobs have finished, doing queued jobs meanwhile. A thread that
    /// only slept here could wait for ever: when every worker is itself waiting, as when the
    /// systems of a batch each share their work out, nobody would be left to do the jobs
    /// they wait for.
    fn wait_helping(&self, latch: &Latch) {
        while !latch.finished() {
            // The frame's work only: a background job could keep this thread for a long time.
            let job = self.shared.lock().frame.pop_front();
            match job {
                Some(job) => job(),
                None => latch.wait_a_moment(),
            }
        }
    }

    /// Runs `others` on the workers and `mine` on this thread, and returns when all of them
    /// have finished. The jobs may borrow from the caller's stack: nothing outlives the call.
    /// A panic in any of them is raised again here, after the rest have finished.
    pub fn run<'scope>(&self, others: Vec<Box<dyn FnOnce() + Send + 'scope>>, mine: impl FnOnce()) {
        let latch = Arc::new(Latch::default());
        let panic: Arc<Mutex<Option<Box<dyn std::any::Any + Send>>>> = Arc::default();
        /// Waits for the workers even if this thread unwinds, since they borrow from it.
        struct Wait<'a>(&'a Pool, &'a Latch);
        impl Drop for Wait<'_> {
            fn drop(&mut self) {
                self.0.wait_helping(self.1);
            }
        }
        *latch.running.lock().unwrap_or_else(|p| p.into_inner()) = others.len();
        let wait = Wait(self, &latch);
        for job in others {
            // SAFETY: the job borrows data that lives for 'scope, and `wait` keeps this
            // function from returning (or unwinding) before the job has run to its end, so
            // the borrow is live for as long as the job exists.
            let job: Job =
                unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Job>(job) };
            let (latch, panic) = (latch.clone(), panic.clone());
            let wrapped: Job = Box::new(move || {
                if let Err(payload) = catch_unwind(AssertUnwindSafe(job)) {
                    panic
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get_or_insert(payload);
                }
                let mut running = latch.running.lock().unwrap_or_else(|p| p.into_inner());
                *running -= 1;
                if *running == 0 {
                    latch.done.notify_all();
                }
            });
            self.shared.lock().frame.push_back(wrapped);
        }
        self.shared.more.notify_all();
        mine();
        drop(wait);
        let payload = panic.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(payload) = payload {
            resume_unwind(payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn jobs_borrow_from_the_caller_and_all_finish_first() {
        let pool = Pool::new(3);
        let total = AtomicUsize::new(0);
        let mut mine = 0;
        for round in 1..=20 {
            let others: Vec<Box<dyn FnOnce() + Send + '_>> = (0..7)
                .map(|_| {
                    Box::new(|| {
                        total.fetch_add(round, Ordering::Relaxed);
                    }) as Box<dyn FnOnce() + Send + '_>
                })
                .collect();
            pool.run(others, || mine += round);
        }
        assert_eq!(mine, 210);
        assert_eq!(total.load(Ordering::Relaxed), 7 * 210);
        // No workers: everything waits for nothing, and nothing is lost.
        Pool::new(0).run(Vec::new(), || mine += 1);
        assert_eq!(mine, 211);
    }

    #[test]
    fn jobs_that_share_out_work_of_their_own_do_not_wait_for_each_other_for_ever() {
        // Two workers, and three jobs at once that each hand out more jobs and wait for them.
        let pool = Pool::new(2);
        let done = AtomicUsize::new(0);
        let inner = || {
            let others: Vec<Box<dyn FnOnce() + Send + '_>> = (0..4)
                .map(|_| {
                    Box::new(|| {
                        done.fetch_add(1, Ordering::Relaxed);
                    }) as Box<dyn FnOnce() + Send + '_>
                })
                .collect();
            pool.run(others, || {
                done.fetch_add(1, Ordering::Relaxed);
            });
        };
        for _ in 0..20 {
            let others: Vec<Box<dyn FnOnce() + Send + '_>> =
                vec![Box::new(inner), Box::new(inner)];
            pool.run(others, inner);
        }
        assert_eq!(done.load(Ordering::Relaxed), 20 * 3 * 5);
    }

    #[test]
    fn background_work_is_done_and_gives_way_to_the_frames() {
        use std::sync::mpsc::channel;
        for workers in [0, 1, 3] {
            let pool = Pool::new(workers);
            assert_eq!(pool.background_threads(), workers.saturating_sub(1).max(1));
            let (done, results) = channel();
            for job in 0..12 {
                let done = done.clone();
                pool.spawn(move || {
                    thread::sleep(std::time::Duration::from_millis(2));
                    let _ = done.send(job);
                });
            }
            // The frame's work is not kept waiting behind it.
            let count = AtomicUsize::new(0);
            let others: Vec<Box<dyn FnOnce() + Send + '_>> = (0..4)
                .map(|_| {
                    Box::new(|| {
                        count.fetch_add(1, Ordering::Relaxed);
                    }) as Box<dyn FnOnce() + Send + '_>
                })
                .collect();
            pool.run(others, || {});
            assert_eq!(count.load(Ordering::Relaxed), 4);
            // A job that panics does not take its thread, or the jobs after it, with it.
            pool.spawn(|| panic!("a background job fell over"));
            let done_after = done.clone();
            pool.spawn(move || {
                let _ = done_after.send(99);
            });
            let mut got: Vec<i32> = (0..13).map(|_| results.recv().unwrap()).collect();
            got.sort_unstable();
            assert_eq!(got, (0..12).chain([99]).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_panic_on_a_worker_comes_back_after_the_rest_finish() {
        let pool = Pool::new(2);
        let finished = AtomicUsize::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let others: Vec<Box<dyn FnOnce() + Send + '_>> = vec![
                Box::new(|| panic!("a worker fell over")),
                Box::new(|| {
                    thread::sleep(std::time::Duration::from_millis(20));
                    finished.fetch_add(1, Ordering::Relaxed);
                }),
            ];
            pool.run(others, || {
                finished.fetch_add(1, Ordering::Relaxed);
            });
        }));
        let payload = result.expect_err("the panic is passed on");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"a worker fell over"));
        assert_eq!(
            finished.load(Ordering::Relaxed),
            2,
            "the others were not abandoned"
        );
        // The pool is still usable.
        pool.run(vec![Box::new(|| {}) as Box<dyn FnOnce() + Send>], || {});
    }
}
