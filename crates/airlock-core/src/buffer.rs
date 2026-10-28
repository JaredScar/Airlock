//! The broadcast delay itself: a time-ordered queue that releases items once the
//! playhead (live time minus delay) passes them. Used for audio chunks, video
//! frames/packets, and anything else that must stay in sync.

use crate::time::{Dur, Span, Ts};
use std::collections::VecDeque;

#[derive(Debug)]
pub struct DelayLine<T> {
    delay: Dur,
    items: VecDeque<(Ts, T)>,
}

impl<T> DelayLine<T> {
    pub fn new(delay: Dur) -> Self {
        DelayLine {
            delay,
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
}
