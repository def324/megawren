use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// A bounded four-second rate window absorbs the SDK's bursty byte callbacks.
pub struct Rate {
    samples: VecDeque<(Instant, i64)>,
    value: f64,
}
impl Rate {
    pub fn new(now: Instant) -> Self {
        Self {
            samples: VecDeque::from([(now, 0)]),
            value: 0.0,
        }
    }
    pub fn observe(&mut self, bytes: i64, now: Instant) -> f64 {
        let &(last, previous) = self.samples.back().unwrap();
        if bytes < previous {
            *self = Self::new(now);
            return 0.0;
        }
        if now.duration_since(last) < Duration::from_millis(500) {
            return self.value;
        }
        self.samples.push_back((now, bytes));
        while self.samples.len() > 2
            && now.duration_since(self.samples[1].0) >= Duration::from_secs(4)
        {
            self.samples.pop_front();
        }
        // At most ten half-second samples, including the window's boundary.
        while self.samples.len() > 10 {
            self.samples.pop_front();
        }
        let &(start, initial) = self.samples.front().unwrap();
        let elapsed = now.duration_since(start).as_secs_f64();
        self.value = if elapsed >= 1.0 {
            (bytes - initial).max(0) as f64 / elapsed
        } else {
            0.0
        };
        self.value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bursty_callbacks_settle_and_a_stall_decays_to_zero() {
        let start = Instant::now();
        let mut rate = Rate::new(start);
        for half in 1..=20 {
            let bytes = (half / 4) * 4000;
            let speed = rate.observe(bytes, start + Duration::from_millis(half as u64 * 500));
            if half >= 10 {
                assert!((1900.0..=2100.0).contains(&speed), "{speed}");
            }
        }
        for half in 21..=30 {
            rate.observe(20000, start + Duration::from_millis(half * 500));
        }
        assert_eq!(rate.value, 0.0);
        assert!(rate.samples.len() <= 10);
        assert_eq!(rate.observe(0, start + Duration::from_secs(16)), 0.0);
    }
}
