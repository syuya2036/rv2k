/// Convert a wrapping 32-bit free-running hardware counter to logical ticks.
/// Sample at least once per counter wrap; masked interrupts do not lose time.
pub struct Clock {
    last: u32,
    remainder: u32,
    period: u32,
}
impl Clock {
    pub const fn new(period: u32, counter: u32) -> Self {
        assert!(period != 0);
        Self {
            last: counter,
            remainder: 0,
            period,
        }
    }
    pub fn sample(&mut self, counter: u32) -> u32 {
        let elapsed = counter.wrapping_sub(self.last);
        self.last = counter;
        let mut ticks = elapsed / self.period;
        let remainder = elapsed % self.period;
        // Add two remainders without a 33-bit sum or 64-bit division on RV32E.
        let until_tick = self.period - remainder;
        if self.remainder >= until_tick {
            ticks += 1;
            self.remainder -= until_tick;
        } else {
            self.remainder += remainder;
        }
        ticks
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrap_remainder_and_multiple_ticks_while_irq_masked() {
        let mut c = Clock::new(10, u32::MAX - 5);
        assert_eq!(c.sample(u32::MAX), 0);
        assert_eq!(c.sample(4), 1);
        assert_eq!(c.sample(39), 3);
        assert_eq!(c.sample(44), 1);
        assert_eq!(c.sample(44), 0);
    }
    #[test]
    fn large_deltas_and_periods_match_wide_arithmetic() {
        for period in [1, 2, 10, 240_000, u32::MAX / 2 + 1, u32::MAX] {
            let mut c = Clock::new(period, 0);
            let mut last = 0;
            let mut remainder = 0u64;
            let mut counter = 0u32;
            for i in 0..10000u32 {
                counter = if i < 3 {
                    counter.wrapping_add(u32::MAX)
                } else {
                    counter.wrapping_mul(1664525).wrapping_add(1013904223)
                };
                let elapsed = counter.wrapping_sub(last) as u64 + remainder;
                last = counter;
                assert_eq!(c.sample(counter), (elapsed / period as u64) as u32);
                remainder = elapsed % period as u64;
                assert_eq!(c.remainder, remainder as u32);
            }
        }
    }
}
