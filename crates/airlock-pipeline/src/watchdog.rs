//! If the pipeline stops publishing, start it again. Times here are wall-clock
//! microseconds because a dead process has no media clock.

#[derive(Debug, Clone)]
pub struct Watchdog {
    timeout_us: u64,
    last_us: u64,
    restarts: u32,
}

impl Watchdog {
    pub fn new(timeout_us: u64, now_us: u64) -> Self {
        Watchdog {
            timeout_us,
            last_us: now_us,
            restarts: 0,
        }
    }

    pub fn pet(&mut self, now_us: u64) {
        self.last_us = now_us;
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// Restarts once the silence is longer than the timeout. Returns true when it did.
    pub fn poll(&mut self, now_us: u64) -> bool {
        if now_us.saturating_sub(self.last_us) <= self.timeout_us {
            return false;
        }
        self.restarts += 1;
        self.last_us = now_us;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_restarts_within_the_timeout() {
        let mut dog = Watchdog::new(2_000_000, 0);
        assert!(!dog.poll(1_000_000));
        assert!(dog.poll(2_000_001));
        assert_eq!(dog.restarts(), 1);
        assert!(!dog.poll(3_000_000));
        dog.pet(3_000_000);
        assert!(!dog.poll(4_000_000));
    }
}
