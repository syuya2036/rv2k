pub const WATERMARK: u32 = 0xa5a5_a5a5;
pub const GUARD_WORDS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Usage {
    pub used: u16,
    pub guard_ok: bool,
}

/// A statically allocated stack range. Read with volatile accesses so live
/// compiler stack writes are observed without creating an aliased slice.
pub struct StackRegion {
    base: *const u32,
    words: usize,
}
impl StackRegion {
    /// # Safety
    /// The range must remain readable and word aligned for this object's life.
    /// Its size must fit u16 bytes. The caller excludes concurrent stack writes
    /// during inspection (normally by masking interrupts).
    pub unsafe fn new(base: *const u32, words: usize) -> Self {
        Self { base, words }
    }
    pub fn guard_ok(&self) -> bool {
        self.words >= GUARD_WORDS
            && (0..GUARD_WORDS).all(|i| unsafe { self.base.add(i).read_volatile() == WATERMARK })
    }
    pub fn inspect(&self) -> Usage {
        let mut guard_ok = self.words >= GUARD_WORDS;
        let mut untouched = 0;
        for index in 0..self.words {
            let word = unsafe { self.base.add(index).read_volatile() };
            if index < GUARD_WORDS && word != WATERMARK {
                guard_ok = false;
            }
            if index == untouched && word == WATERMARK {
                untouched += 1;
            }
        }
        Usage {
            used: ((self.words - untouched) * 4) as u16,
            guard_ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_water_and_nonleading_guard_damage() {
        let mut words = [WATERMARK; 64];
        let ptr = words.as_mut_ptr();
        let region = unsafe { StackRegion::new(ptr, words.len()) };
        assert_eq!(
            region.inspect(),
            Usage {
                used: 0,
                guard_ok: true
            }
        );
        unsafe {
            ptr.add(63).write_volatile(1);
            ptr.add(40).write_volatile(1);
        }
        assert_eq!(
            region.inspect(),
            Usage {
                used: 96,
                guard_ok: true
            }
        );
        unsafe {
            ptr.add(2).write_volatile(0);
        }
        assert_eq!(
            region.inspect(),
            Usage {
                used: 248,
                guard_ok: false
            }
        );
    }
}
