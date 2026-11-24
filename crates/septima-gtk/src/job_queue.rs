//! The cap on how many archives are worked on at once.
//!
//! Every extract and create job runs its own `7zz`. Without a cap, a batch of
//! a hundred archives starts a hundred of them together, and the ceiling is
//! the machine's memory. [`JobGate`] holds jobs back so only a few run, in the
//! order they were asked for.
//!
//! A job takes one or more *slots* out of the gate's limit. Extracting takes
//! one. Creating takes more when a single `7zz` already fills the CPU or a
//! large share of the free memory (see [`compress_slots`]), so fewer of those
//! run side by side.
//!
//! The gate is plain bookkeeping with no GTK in it: it hands jobs back to the
//! caller to start, which keeps it testable and keeps the window from calling
//! into a job while it still has the gate borrowed.

use std::collections::VecDeque;

struct Waiting<T> {
    id: u64,
    slots: usize,
    job: T,
}

pub struct JobGate<T> {
    limit: usize,
    used: usize,
    running: usize,
    next_id: u64,
    waiting: VecDeque<Waiting<T>>,
}

impl<T> std::fmt::Debug for JobGate<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobGate")
            .field("limit", &self.limit)
            .field("used", &self.used)
            .field("running", &self.running)
            .field("waiting", &self.waiting.len())
            .finish()
    }
}

impl<T> Default for JobGate<T> {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self::new(default_limit(cpus))
    }
}

impl<T> JobGate<T> {
    pub fn new(limit: usize) -> Self {
        Self { limit: limit.max(1), used: 0, running: 0, next_id: 0, waiting: VecDeque::new() }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Jobs started and not yet released.
    pub fn running(&self) -> usize {
        self.running
    }

    /// Jobs held back.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Ask to run `job`, which needs `slots`. Returns the job's id and, when
    /// it may start right away, the job itself for the caller to start.
    /// Otherwise it waits its turn; nothing jumps the queue.
    pub fn submit(&mut self, slots: usize, job: T) -> (u64, Option<T>) {
        let slots = slots.clamp(1, self.limit);
        let id = self.next_id;
        self.next_id += 1;
        if self.waiting.is_empty() && self.fits(slots) {
            self.used += slots;
            self.running += 1;
            return (id, Some(job));
        }
        self.waiting.push_back(Waiting { id, slots, job });
        (id, None)
    }

    /// A started job has ended; give its `slots` back. Follow with
    /// [`startable`](Self::startable).
    pub fn release(&mut self, slots: usize) {
        self.used = self.used.saturating_sub(slots.clamp(1, self.limit));
        self.running = self.running.saturating_sub(1);
    }

    /// Take a job that hasn't started out of the queue. `None` if it already
    /// started (or never existed). Follow with [`startable`](Self::startable):
    /// removing a large job at the front can let the ones behind it go.
    pub fn cancel_waiting(&mut self, id: u64) -> Option<T> {
        let at = self.waiting.iter().position(|w| w.id == id)?;
        self.waiting.remove(at).map(|w| w.job)
    }

    /// The waiting jobs that may start now, front first. They count as
    /// running from here on.
    pub fn startable(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        while let Some(front) = self.waiting.front() {
            if !self.fits(front.slots) {
                break;
            }
            let w = self.waiting.pop_front().expect("front exists");
            self.used += w.slots;
            self.running += 1;
            out.push(w.job);
        }
        out
    }

    /// With nothing running, anything fits: a job is never stuck forever.
    fn fits(&self, slots: usize) -> bool {
        self.used == 0 || self.used + slots <= self.limit
    }
}

/// How many archives to work on at once on a machine with `cpus` threads:
/// half of them, but at least 2 and at most 8.
pub fn default_limit(cpus: usize) -> usize {
    (cpus / 2).clamp(2, 8)
}

/// How many of the gate's `limit` slots one create job should take.
///
/// Two things can make a single `7zz` heavy. Threads: one job using every CPU
/// thread leaves little for a second, so at most enough jobs run to use the
/// CPU twice over. Memory: the jobs running together should fit in half of
/// what's free. Whichever allows fewer jobs decides. Unknowns (`None`) don't
/// restrict.
pub fn compress_slots(
    limit: usize,
    cpus: usize,
    threads: Option<u32>,
    estimated_memory: Option<u64>,
    memory_available: Option<u64>,
) -> usize {
    let limit = limit.max(1);
    let cpus = cpus.max(1);
    // No explicit thread count means 7zz uses them all.
    let threads = threads.map_or(cpus, |t| (t as usize).max(1));
    let by_cpu = (2 * cpus / threads).max(1);
    let by_memory = match (estimated_memory, memory_available) {
        (Some(need), Some(free)) if need > 0 => ((free / 2) / need).max(1) as usize,
        _ => limit,
    };
    let at_once = by_cpu.min(by_memory).clamp(1, limit);
    limit.div_ceil(at_once)
}

/// `MemAvailable` from `/proc/meminfo` text, in bytes.
pub fn parse_mem_available(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Memory the kernel says is available right now, in bytes.
pub fn memory_available() -> Option<u64> {
    parse_mem_available(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// A `7zz` size argument such as `64m` or `1g`, in bytes.
pub fn size_arg_bytes(arg: &str) -> Option<u64> {
    let arg = arg.trim().to_ascii_lowercase();
    let (digits, unit) = match arg.chars().last()? {
        'k' => (&arg[..arg.len() - 1], 1u64 << 10),
        'm' => (&arg[..arg.len() - 1], 1 << 20),
        'g' => (&arg[..arg.len() - 1], 1 << 30),
        'b' => (&arg[..arg.len() - 1], 1),
        _ => (arg.as_str(), 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_limit_runs_and_the_rest_wait_in_order() {
        let mut gate = JobGate::new(2);
        assert_eq!(gate.submit(1, 'a').1, Some('a'));
        assert_eq!(gate.submit(1, 'b').1, Some('b'));
        assert_eq!(gate.submit(1, 'c').1, None);
        assert_eq!(gate.submit(1, 'd').1, None);
        assert_eq!((gate.running(), gate.waiting()), (2, 2));

        assert!(gate.startable().is_empty(), "nothing was released");
        gate.release(1);
        assert_eq!(gate.startable(), vec!['c']);
        gate.release(1);
        gate.release(1);
        assert_eq!(gate.startable(), vec!['d']);
        gate.release(1);
        assert_eq!((gate.running(), gate.waiting()), (0, 0));
    }

    #[test]
    fn a_hundred_jobs_never_exceed_the_limit() {
        let mut gate = JobGate::new(4);
        let mut running = 0usize;
        let mut started = 0usize;
        for n in 0..100 {
            if gate.submit(1, n).1.is_some() {
                running += 1;
                started += 1;
            }
        }
        assert_eq!(running, 4);
        while running > 0 {
            gate.release(1);
            running -= 1;
            let next = gate.startable();
            running += next.len();
            started += next.len();
            assert!(running <= 4);
        }
        assert_eq!(started, 100);
    }

    #[test]
    fn a_heavy_job_takes_its_slots_and_nothing_jumps_past_it() {
        let mut gate = JobGate::new(4);
        assert!(gate.submit(1, "light").1.is_some());
        assert!(gate.submit(4, "heavy").1.is_none(), "doesn't fit beside the light one");
        // A small job behind the heavy one must not start ahead of it, or a
        // stream of small jobs could hold the heavy one back forever.
        assert!(gate.submit(1, "small").1.is_none());
        gate.release(1);
        assert_eq!(gate.startable(), vec!["heavy"]);
        gate.release(4);
        assert_eq!(gate.startable(), vec!["small"]);
    }

    #[test]
    fn a_job_asking_for_more_than_the_limit_still_runs() {
        let mut gate = JobGate::new(2);
        assert!(gate.submit(9, 'x').1.is_some());
        assert!(gate.submit(1, 'y').1.is_none());
        gate.release(9);
        assert_eq!(gate.startable(), vec!['y']);
    }

    #[test]
    fn cancelling_a_waiting_job_removes_it_and_frees_the_ones_behind() {
        let mut gate = JobGate::new(2);
        let (_, a) = gate.submit(1, 'a');
        assert!(a.is_some());
        let (heavy, _) = gate.submit(2, 'h');
        let (_, c) = gate.submit(1, 'c');
        assert!(c.is_none());

        assert_eq!(gate.cancel_waiting(heavy), Some('h'));
        assert_eq!(gate.cancel_waiting(heavy), None, "already gone");
        assert_eq!(gate.startable(), vec!['c'], "the job behind it now fits");
    }

    #[test]
    fn a_running_job_cannot_be_cancelled_as_waiting() {
        let mut gate = JobGate::new(1);
        let (id, started) = gate.submit(1, 'a');
        assert!(started.is_some());
        assert_eq!(gate.cancel_waiting(id), None);
        assert_eq!(gate.running(), 1);
    }

    #[test]
    fn the_default_limit_is_half_the_threads_within_2_and_8() {
        assert_eq!(default_limit(1), 2);
        assert_eq!(default_limit(4), 2);
        assert_eq!(default_limit(12), 6);
        assert_eq!(default_limit(64), 8);
    }

    #[test]
    fn create_jobs_weigh_more_when_they_fill_the_cpu() {
        // 12 threads each on a 12-thread machine: two at once (limit 6 → 3 slots).
        assert_eq!(compress_slots(6, 12, Some(12), None, None), 3);
        // No thread count given means all of them.
        assert_eq!(compress_slots(6, 12, None, None, None), 3);
        // Single-threaded jobs: as many as the limit allows.
        assert_eq!(compress_slots(6, 12, Some(1), None, None), 1);
    }

    #[test]
    fn create_jobs_weigh_more_when_memory_is_tight() {
        const GIB: u64 = 1 << 30;
        // 3 GiB each with 8 GiB free: half of free holds one.
        assert_eq!(compress_slots(6, 12, Some(1), Some(3 * GIB), Some(8 * GIB)), 6);
        // 1 GiB each with 8 GiB free: four would fit, and 2 slots each lets three run.
        assert_eq!(compress_slots(6, 12, Some(1), Some(GIB), Some(8 * GIB)), 2);
        // More than everything free: still one at a time, never zero.
        assert_eq!(compress_slots(6, 12, Some(1), Some(64 * GIB), Some(8 * GIB)), 6);
    }

    #[test]
    fn reads_mem_available_and_size_arguments() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\n";
        assert_eq!(parse_mem_available(meminfo), Some(8_000_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
        assert_eq!(size_arg_bytes("64m"), Some(64 << 20));
        assert_eq!(size_arg_bytes("1g"), Some(1 << 30));
        assert_eq!(size_arg_bytes("4096"), Some(4096));
        assert_eq!(size_arg_bytes("big"), None);
    }
}
