//! RFC 6298 estimator in 10 ms ticks (SRTT x8, RTTVAR x4), without floating point.
pub const INITIAL: u32 = 100;
pub const MAXIMUM: u32 = 6000;
#[derive(Clone, Copy)]
pub struct Rto {
    srtt8: u32,
    variance4: u32,
    value: u32,
}
impl Default for Rto {
    fn default() -> Self {
        Self::new()
    }
}
impl Rto {
    // TCP resets the estimator on SYN. Dormant connections need no initialized
    // flash image for a timer that has not started yet.
    pub(super) const fn dormant() -> Self {
        Self {
            srtt8: 0,
            variance4: 0,
            value: 0,
        }
    }
    pub const fn new() -> Self {
        Self {
            srtt8: 0,
            variance4: 0,
            value: INITIAL,
        }
    }
    pub fn ticks(&self) -> u32 {
        self.value
    }
    pub fn sample(&mut self, ticks: u32) {
        let ticks = ticks.clamp(1, MAXIMUM);
        if self.srtt8 == 0 {
            self.srtt8 = ticks * 8;
            self.variance4 = ticks * 2;
        } else {
            let deviation = (self.srtt8 / 8).abs_diff(ticks);
            self.variance4 = (3 * self.variance4 + 4 * deviation).div_ceil(4);
            self.srtt8 = (7 * self.srtt8 + ticks * 8).div_ceil(8);
        }
        self.value = (self.srtt8.div_ceil(8) + self.variance4.max(1)).clamp(INITIAL, MAXIMUM);
    }
    pub fn backoff(&mut self) {
        self.value = (self.value * 2).min(MAXIMUM);
    }
    pub fn syn_retransmitted(&mut self) {
        // 6298 section 5.7: SYN timeout before a valid RTT sample -> >=3 seconds.
        if self.srtt8 == 0 {
            self.value = self.value.max(300);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn estimator_uses_old_srtt_for_variation_and_bounded_backoff() {
        let mut r = Rto::new();
        assert_eq!(r.ticks(), 100);
        r.sample(100);
        assert_eq!(r.ticks(), 300);
        r.sample(200);
        // RTTVAR=62.5; SRTT=112.5; round up to avoid aggressive retransmission.
        assert_eq!(r.ticks(), 363);
        for _ in 0..12 {
            r.backoff();
        }
        assert_eq!(r.ticks(), 6000);
        r.sample(0);
        assert!(r.ticks() >= 100);
        let mut syn = Rto::new();
        syn.syn_retransmitted();
        assert_eq!(syn.ticks(), 300);
    }
}
