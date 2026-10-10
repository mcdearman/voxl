//! The threads that systems run on when a batch of them runs at the same moment.

use std::{
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{
        mpsc::{channel, Receiver, Sender},
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

/// Worker threads that live as long as the program.
pub struct Pool {
    jobs: Mutex<Sender<Job>>,
    /// The jobs not yet taken up: by a worker, or by a thread waiting for jobs of its own.
    queue: Arc<Mutex<Receiver<Job>>>,
    workers: usize,
}

impl Pool {
    fn new(workers: usize) -> Self {
        let (jobs, queue) = channel::<Job>();
        let queue = Arc::new(Mutex::new(queue));
        for index in 0..workers {
            let queue = Arc::clone(&queue);
            thread::Builder::new()
                .name(format!("mira systems {index}"))
                .spawn(move || loop {
                    let job = queue.lock().unwrap_or_else(|p| p.into_inner()).recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                })
                .expect("can't start a worker thread");
        }
        Self {
            jobs: Mutex::new(jobs),
            queue,
            workers,
        }
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

    /// Waits until the latch's jobs have finished, doing queued jobs meanwhile. A thread that
    /// only slept here could wait for ever: when every worker is itself waiting, as when the
    /// systems of a batch each share their work out, nobody would be left to do the jobs
    /// they wait for. An idle worker holds the queue while it sleeps on it, and then the job
    /// is that worker's to take.
    fn wait_helping(&self, latch: &Latch) {
        while !latch.finished() {
            let job = self.queue.try_lock().ok().and_then(|queue| queue.try_recv().ok());
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
            self.jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .send(wrapped)
                .expect("the worker threads are gone");
        }
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
