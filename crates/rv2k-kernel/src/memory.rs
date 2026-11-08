use rv2k_api::syscall::Error;

#[derive(Clone, Copy)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

impl Region {
    pub fn contains(self, address: usize, len: usize) -> bool {
        self.start <= address && address.checked_add(len).is_some_and(|end| end <= self.end)
    }
}

pub struct MemoryMap {
    pub stack: Region,
    pub application: Region,
    pub flash: Region,
}

impl MemoryMap {
    pub fn validate(
        &self,
        address: usize,
        len: usize,
        alignment: usize,
        write: bool,
    ) -> Result<(), Error> {
        if !alignment.is_power_of_two() || address & (alignment - 1) != 0 {
            return Err(Error::Fault);
        }
        let end = address.checked_add(len).ok_or(Error::Fault)?;
        let contains = |region: Region| region.start <= address && end <= region.end;
        if contains(self.stack) || contains(self.application) || (!write && contains(self.flash)) {
            Ok(())
        } else {
            Err(Error::Fault)
        }
    }
}

/// Backend copies bytes during dispatch; it must enforce the current task's map.
/// No task pointer is retained after the call.
pub trait UserMemory {
    fn copy_out(&mut self, address: usize, bytes: &[u8], alignment: usize) -> Result<(), Error>;
    fn copy_in(&mut self, _address: usize, _bytes: &mut [u8]) -> Result<(), Error> {
        Err(Error::Fault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_permissions_boundaries_and_overflow() {
        let map = MemoryMap {
            stack: Region {
                start: 0x1000,
                end: 0x1100,
            },
            application: Region {
                start: 0x2000,
                end: 0x2040,
            },
            flash: Region {
                start: 0x10,
                end: 0x800,
            },
        };
        for (addr, len, align, write, ok) in [
            (0x1000, 256, 4, true, true),
            (0x10fc, 4, 4, true, true),
            (0x10fc, 5, 4, true, false),
            (0x1001, 4, 4, true, false),
            (0x1100, 1, 1, true, false),
            (0x1100, 0, 1, true, true),
            (0x2000, 64, 4, true, true),
            (0x20, 8, 4, false, true),
            (0x20, 8, 4, true, false),
            (0, 0, 1, true, false),
            (usize::MAX - 3, 8, 1, false, false),
            (0x1000, 4, 0, true, false),
        ] {
            assert_eq!(map.validate(addr, len, align, write).is_ok(), ok);
        }
    }
}
