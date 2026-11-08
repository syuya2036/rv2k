//! Shared word arithmetic for the RV32EC runtime, which has no M extension.
use core::num::NonZeroU32;

/// Quotient and remainder without widening, multiplication, or division.
#[inline(never)]
pub fn div_rem(mut value: u32, divisor: NonZeroU32) -> (u32, u32) {
    let mut shifted = divisor.get();
    let mut bit = 1u32;
    let mut quotient = 0;
    let half = value >> 1;
    // Doubling cannot overflow: shifted <= value/2 <= u32::MAX/2.
    while shifted <= half {
        shifted <<= 1;
        bit <<= 1;
    }
    while bit != 0 {
        if value >= shifted {
            value -= shifted;
            quotient |= bit;
        }
        shifted >>= 1;
        bit >>= 1;
    }
    (quotient, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn division_matches_native_boundaries_and_varied_words() {
        let check =
            |n: u32, d: u32| assert_eq!(div_rem(n, NonZeroU32::new(d).unwrap()), (n / d, n % d));
        for n in [0, 1, 2, 9, 10, 655359, 1 << 31, u32::MAX - 1, u32::MAX] {
            for d in [1, 2, 3, 10, 240000, (1 << 31) - 1, 1 << 31, u32::MAX] {
                check(n, d);
            }
        }
        for n in 0..1024 {
            for d in 1..128 {
                check(n, d);
            }
        }
        let mut n = 0x12345678u32;
        for _ in 0..10000 {
            n = n.wrapping_mul(1664525).wrapping_add(1013904223);
            check(n, n.rotate_left(13) | 1);
            check(n, 10);
        }
    }
}
