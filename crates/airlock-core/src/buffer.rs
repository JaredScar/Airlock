//! The broadcast delay itself: a time-ordered queue that releases items once the
//! playhead (live time minus delay) passes them. Used for audio chunks, video
//! frames/packets, and anything else that must stay in sync.

use crate::time::{Dur, Span, Ts};
use std::collections::VecDeque;

/// Enough for an hour of 20 ms audio chunks, plus margin. Past this, the oldest
/// item is dropped so a stuck output cannot grow the hold without bound.
pub const DELAY_LINE_CAP: usize = 200_000;

#[derive(Debug)]
pub struct DelayLine<T> {
    delay: Dur,
    cap: usize,
    items: VecDeque<(Ts, T)>,
}

impl<T> DelayLine<T> {
    pub fn new(delay: Dur) -> Self {
        Self::with_cap(delay, DELAY_LINE_CAP)
    }

    pub fn with_cap(delay: Dur, cap: usize) -> Self {
        DelayLine {
            delay,
            cap: cap.max(1),
            items: VecDeque::new(),
        }
    }

    pub fn delay(&self) -> Dur {
        self.delay
    }

    /// Changing the delay mid-stream causes a jump; the pipeline should only allow it
    /// while offline or ramp it (see PLAN.md, "adjustable delay").
    pub fn set_delay(&mut self, delay: Dur) {
        self.delay = delay;
    }

    /// Inserts an item. Items normally arrive in order; out-of-order items are slotted in.
    pub fn push(&mut self, ts: Ts, item: T) {
        match self.items.back() {
            Some((last, _)) if *last > ts => {
                let idx = self.items.partition_point(|(t, _)| *t <= ts);
                self.items.insert(idx, (ts, item));
            }
            _ => self.items.push_back((ts, item)),
        }
        while self.items.len() > self.cap {
            self.items.pop_front();
        }
    }

    /// The media time currently allowed to air, given the newest live media time.
    pub fn playhead(&self, live: Ts) -> Ts {
        live - self.delay
    }

    /// Removes and returns every item at or before `playhead`, in order.
    pub fn drain_due(&mut self, playhead: Ts) -> Vec<(Ts, T)> {
        let n = self.items.partition_point(|(t, _)| *t <= playhead);
        self.items.drain(..n).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Span of media currently held back (for the UI's delay meter).
    pub fn buffered(&self) -> Option<Span> {
        Some(Span::new(self.items.front()?.0, self.items.back()?.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_after_delay() {
        let mut d = DelayLine::new(Dur::from_millis(8000));
        for ms in (0..10_000).step_by(1000) {
            d.push(Ts::from_millis(ms), ms);
        }
        let ph = d.playhead(Ts::from_millis(10_000));
        assert_eq!(ph, Ts::from_millis(2000));
        let out: Vec<_> = d.drain_due(ph).into_iter().map(|(_, v)| v).collect();
        assert_eq!(out, vec![0, 1000, 2000]);
        assert_eq!(d.len(), 7);
        assert_eq!(d.buffered(), Some(Span::from_millis(3000, 9000)));
    }

    #[test]
    fn out_of_order_insert() {
        let mut d = DelayLine::new(Dur::from_millis(0));
        d.push(Ts(10), 'a');
        d.push(Ts(30), 'c');
        d.push(Ts(20), 'b');
        let out: String = d.drain_due(Ts(100)).into_iter().map(|(_, c)| c).collect();
        assert_eq!(out, "abc");
    }

    #[test]
    fn cap_drops_the_oldest_and_an_eight_hour_run_does_not_grow() {
        let mut d = DelayLine::with_cap(Dur::from_millis(8_000), 3);
        d.push(Ts::from_millis(0), 0);
        d.push(Ts::from_millis(20), 1);
        d.push(Ts::from_millis(40), 2);
        d.push(Ts::from_millis(60), 3);
        assert_eq!(d.len(), 3);
        let kept: Vec<_> = d
            .drain_due(Ts::from_millis(60))
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        assert_eq!(kept, vec![1, 2, 3]);

        let mut live = DelayLine::new(Dur::from_millis(8_000));
        let mut high = 0usize;
        for step in 0..(8 * 3_600 * 50) {
            let ts = Ts::from_millis(step * 20);
            live.push(ts, step);
            let _ = live.drain_due(live.playhead(ts));
            high = high.max(live.len());
        }
        assert!(high < 500, "hold grew to {high}");
        assert!(live.len() < 500);
    }
}

/// Moves the hold toward a new delay by at most 5% of the media that just arrived.
/// The playhead does not jump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayRamp {
    current: Dur,
    target: Dur,
}

impl DelayRamp {
    pub const MAX_STRETCH: f64 = 0.05;

    pub fn new(current: Dur) -> Self {
        DelayRamp {
            current,
            target: current,
        }
    }

    pub fn current(&self) -> Dur {
        self.current
    }

    pub fn target(&self) -> Dur {
        self.target
    }

    pub fn set_target(&mut self, target: Dur) {
        self.target = target;
    }

    /// Below 1 the output is stretched and the hold grows. Above 1 it shrinks.
    pub fn rate(&self) -> f64 {
        if self.target > self.current {
            1.0 - Self::MAX_STRETCH
        } else if self.target < self.current {
            1.0 + Self::MAX_STRETCH
        } else {
            1.0
        }
    }

    /// Applies one step after `elapsed` of new live media.
    pub fn advance(&mut self, elapsed: Dur) -> Dur {
        let max_step = (elapsed.as_secs_f64() * Self::MAX_STRETCH * 1_000_000.0).round() as i64;
        let delta = self.target.0 - self.current.0;
        self.current = Dur(self.current.0 + delta.clamp(-max_step, max_step));
        self.current
    }
}

#[cfg(test)]
mod ramp_tests {
    use super::*;

    #[test]
    fn a_longer_delay_ramps_instead_of_jumping() {
        let mut ramp = DelayRamp::new(Dur::from_millis(8_000));
        ramp.set_target(Dur::from_millis(10_000));
        assert!((ramp.rate() - 0.95).abs() < 0.001);
        let after = ramp.advance(Dur::from_millis(1_000));
        assert_eq!(after, Dur::from_millis(8_050));
        assert_eq!(ramp.advance(Dur::ZERO), Dur::from_millis(8_050));
    }

    #[test]
    fn a_shorter_delay_ramps_down() {
        let mut ramp = DelayRamp::new(Dur::from_millis(8_000));
        ramp.set_target(Dur::from_millis(6_000));
        assert!((ramp.rate() - 1.05).abs() < 0.001);
        assert_eq!(
            ramp.advance(Dur::from_millis(1_000)),
            Dur::from_millis(7_950)
        );
    }
}
