//! Decides when the judge thread should run, and drops a result that a newer
//! request has already replaced.

use airlock_core::{Dur, Ts};

pub struct JudgeScheduler {
    interval: Dur,
    word_limit: usize,
    words_since: usize,
    last_at: Option<Ts>,
    next_gen: u64,
    inflight: Option<u64>,
    latencies_us: Vec<i64>,
}

impl Default for JudgeScheduler {
    fn default() -> Self {
        JudgeScheduler {
            interval: Dur::from_millis(1_500),
            word_limit: 24,
            words_since: 0,
            last_at: None,
            next_gen: 0,
            inflight: None,
            latencies_us: Vec::new(),
        }
    }
}

impl JudgeScheduler {
    /// True when enough media time or enough new words have arrived.
    pub fn due(&mut self, new_words: usize, now: Ts) -> bool {
        self.words_since += new_words;
        let by_time = match self.last_at {
            Some(then) => now - then >= self.interval,
            None => new_words > 0,
        };
        by_time || self.words_since >= self.word_limit
    }

    /// Starts a request. A previous in-flight generation is now stale.
    pub fn begin(&mut self, now: Ts) -> u64 {
        self.next_gen += 1;
        self.inflight = Some(self.next_gen);
        self.words_since = 0;
        self.last_at = Some(now);
        self.next_gen
    }

    /// Records latency only if this generation is still the latest.
    /// Returns false when a newer `begin` superseded it, so the caller drops the result.
    pub fn complete(&mut self, generation: u64, elapsed_us: i64) -> bool {
        if self.inflight != Some(generation) {
            return false;
        }
        self.inflight = None;
        self.latencies_us.push(elapsed_us);
        true
    }

    pub fn latencies_us(&self) -> &[i64] {
        &self.latencies_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_request_drops_the_older_result() {
        let mut sched = JudgeScheduler::default();
        assert!(sched.due(3, Ts::from_millis(1_000)));
        let first = sched.begin(Ts::from_millis(1_000));
        assert!(!sched.due(2, Ts::from_millis(1_200)));
        assert!(sched.due(1, Ts::from_millis(2_600)));
        let second = sched.begin(Ts::from_millis(2_600));
        assert!(!sched.complete(first, 400_000));
        assert!(sched.latencies_us().is_empty());
        assert!(sched.complete(second, 250_000));
        assert_eq!(sched.latencies_us(), &[250_000]);
    }
}
