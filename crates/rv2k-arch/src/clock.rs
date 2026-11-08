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
        let elapsed = counter.wrapping_sub(self.last) as u64 + self.remainder as u64;
        self.last = counter;
        self.remainder = (elapsed % self.period as u64) as u32;
        (elapsed / self.period as u64) as u32
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
}
