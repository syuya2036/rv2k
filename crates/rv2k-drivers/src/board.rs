//! J4M6 SOP8: physical pin aliases are one resource, including SWIO aliases.
use crate::{
    spi::{SoftwareSpi, SpiPins},
    uart::{Console, UartTx},
};
use rv2k_api::syscall::Error;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pad {
    Pa1,
    Pa2,
    Pc1,
    Pc2,
    Pc4,
    Pd1,
    Pd4,
    Pd5,
    Pd6,
    Pc5,
    Pc6,
    Pc7,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    Uart,
    SpiClock,
    SpiMosi,
    SpiMiso,
    SpiSelect,
    Debug,
}
pub fn physical_pin(pad: Pad) -> Option<u8> {
    Some(match pad {
        Pad::Pa1 | Pad::Pd6 => 1,
        Pad::Pa2 => 3,
        Pad::Pc1 => 5,
        Pad::Pc2 => 6,
        Pad::Pc4 => 7,
        Pad::Pd1 | Pad::Pd4 | Pad::Pd5 => 8,
        _ => return None,
    })
}
pub fn owner(pad: Pad) -> Result<Owner, Error> {
    Ok(match physical_pin(pad).ok_or(Error::Invalid)? {
        1 => Owner::Uart,
        3 => Owner::SpiClock,
        5 => Owner::SpiMosi,
        6 => Owner::SpiMiso,
        7 => Owner::SpiSelect,
        _ => Owner::Debug,
    })
}
pub fn claim_gpio(pad: Pad) -> Result<(), Error> {
    owner(pad)?;
    Err(Error::Busy)
}

pub trait Mmio {
    fn read(&mut self, address: usize) -> u32;
    fn write(&mut self, address: usize, value: u32);
}
const RCC: usize = 0x40021018;
const AFIO: usize = 0x40010004;
const A: usize = 0x40010800;
const C: usize = 0x40011000;
const D: usize = 0x40011400;
fn modify(m: &mut impl Mmio, address: usize, clear: u32, set: u32) {
    let old = m.read(address);
    m.write(address, (old & !clear) | set);
}
fn mode(m: &mut impl Mmio, port: usize, pin: u32, value: u32) {
    modify(m, port, 15 << (pin * 4), value << (pin * 4));
}
pub fn configure(m: &mut impl Mmio) {
    modify(m, RCC, 0, 1 | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 14));
    // Latches first: CS inactive, mode-0 clock and MOSI low.
    m.write(C + 0x10, 1 << 4);
    m.write(A + 0x14, 1 << 2);
    m.write(C + 0x14, 1 << 1);
    mode(m, A, 1, 4);
    mode(m, D, 4, 4);
    mode(m, D, 5, 4); // non-driving common aliases
    mode(m, A, 2, 2);
    mode(m, C, 1, 2);
    mode(m, C, 2, 4);
    mode(m, C, 4, 2);
    // Remap 2: PD6 TX. Preserve SWIO configuration and all other remaps.
    modify(m, AFIO, (1 << 21) | (1 << 15) | (1 << 2), 1 << 21);
    mode(m, D, 6, 0xa);
    m.write(0x40013808, 208);
    m.write(0x4001380c, (1 << 13) | (1 << 3)); // TX only, HSI 24MHz
}
struct Volatile;
impl Mmio for Volatile {
    fn read(&mut self, address: usize) -> u32 {
        unsafe { core::ptr::read_volatile(address as *const u32) }
    }
    fn write(&mut self, address: usize, value: u32) {
        unsafe {
            core::ptr::write_volatile(address as *mut u32, value);
        }
    }
}
pub struct Ch32Pins {
    _private: (),
}
pub struct Ch32Tx {
    _private: (),
}
/// # Safety
/// Caller exclusively owns the J4M6 GPIO/AFIO/UART registers, uses the 24MHz HSI,
/// and has not enabled a competing driver. Call once before scheduling clients.
pub unsafe fn j4m6() -> (SoftwareSpi<Ch32Pins>, Console<Ch32Tx>) {
    configure(&mut Volatile);
    (
        SoftwareSpi::new(Ch32Pins { _private: () }),
        Console::new(Ch32Tx { _private: () }),
    )
}
impl SpiPins for Ch32Pins {
    fn cs(&mut self, high: bool) {
        Volatile.write(C + if high { 0x10 } else { 0x14 }, 1 << 4);
    }
    fn clock(&mut self, high: bool) {
        Volatile.write(A + if high { 0x10 } else { 0x14 }, 1 << 2);
    }
    fn mosi(&mut self, high: bool) {
        Volatile.write(C + if high { 0x10 } else { 0x14 }, 1 << 1);
    }
    fn miso(&mut self) -> bool {
        Volatile.read(C + 8) & (1 << 2) != 0
    }
    fn half_cycle(&mut self) {
        core::hint::spin_loop();
    }
    fn delay_us(&mut self, us: u16) {
        // Each iteration executes at least one CPU instruction. Additional loop
        // overhead lengthens this minimum delay; exact SPI timing is measured on hardware.
        for _ in 0..us as u32 * 24 {
            core::hint::spin_loop();
        }
    }
}
impl UartTx for Ch32Tx {
    fn try_put(&mut self, byte: u8) -> Result<(), Error> {
        if Volatile.read(0x40013800) & (1 << 7) == 0 {
            return Err(Error::Again);
        }
        Volatile.write(0x40013804, byte as u32);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, vec::Vec};
    struct Registers {
        values: BTreeMap<usize, u32>,
        writes: Vec<(usize, u32)>,
    }
    impl Mmio for Registers {
        fn read(&mut self, a: usize) -> u32 {
            *self.values.get(&a).unwrap_or(&0)
        }
        fn write(&mut self, a: usize, v: u32) {
            self.values.insert(a, v);
            self.writes.push((a, v));
        }
    }
    #[test]
    fn aliases_have_one_owner_and_no_reserved_gpio_is_exposed() {
        assert_eq!(physical_pin(Pad::Pa1), physical_pin(Pad::Pd6));
        assert_eq!(physical_pin(Pad::Pd1), physical_pin(Pad::Pd4));
        assert_eq!(owner(Pad::Pd5), Ok(Owner::Debug));
        for pad in [
            Pad::Pa1,
            Pad::Pa2,
            Pad::Pc1,
            Pad::Pc2,
            Pad::Pc4,
            Pad::Pd1,
            Pad::Pd4,
            Pad::Pd5,
            Pad::Pd6,
        ] {
            assert_eq!(claim_gpio(pad), Err(Error::Busy));
        }
        for pad in [Pad::Pc5, Pad::Pc6, Pad::Pc7] {
            assert_eq!(claim_gpio(pad), Err(Error::Invalid));
        }
    }
    #[test]
    fn configure_latches_before_modes_preserves_swio_and_sets_tx_only_remap2() {
        let mut m = Registers {
            values: BTreeMap::from([
                (A, 0x44444444),
                (C, 0x44444444),
                (D, 0x44444444),
                (AFIO, 0x01008004),
            ]),
            writes: Vec::new(),
        };
        configure(&mut m);
        assert_eq!((m.read(D) >> 4) & 15, 4);
        assert_eq!((m.read(D) >> 16) & 255, 0x44);
        assert_eq!((m.read(A) >> 4) & 15, 4);
        assert_eq!((m.read(A) >> 8) & 15, 2);
        assert_eq!((m.read(D) >> 24) & 15, 0xa);
        assert_eq!(m.read(AFIO), 0x01200000);
        assert_eq!(m.read(0x4001380c), (1 << 13) | (1 << 3));
        let cs = m
            .writes
            .iter()
            .position(|p| *p == (C + 0x10, 1 << 4))
            .unwrap();
        let mode = m.writes.iter().position(|p| p.0 == C).unwrap();
        assert!(cs < mode);
        assert_eq!(
            m.read(RCC) & ((1 << 2) | (1 << 4) | (1 << 5)),
            (1 << 2) | (1 << 4) | (1 << 5)
        );
    }
}
