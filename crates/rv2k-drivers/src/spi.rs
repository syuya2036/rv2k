use rv2k_api::syscall::Error;

pub trait SpiBus {
    fn select(&mut self, active: bool) -> Result<(), Error>;
    fn transfer(&mut self, byte: u8) -> Result<u8, Error>;
    /// Must delay at least the specified microseconds, including after reset.
    fn delay_us(&mut self, us: u16);
}

/// A device owns complete CS-delimited operations. Each operation attempts to
/// release CS before returning, even on errors, preserving the first error.
/// SpiBus implementations receive the fallible byte-based adapter below.
pub trait SpiDevice {
    fn command(&mut self, opcode: u8, value: u8, dummy: bool) -> Result<u8, Error>;
    fn read_buffer(&mut self, opcode: u8, out: &mut [u8]) -> Result<(), Error>;
    fn write_buffer(&mut self, opcode: u8, bytes: &[u8]) -> Result<(), Error>;
    fn delay_us(&mut self, us: u16);
}
impl<S: SpiBus> SpiDevice for S {
    fn command(&mut self, opcode: u8, value: u8, dummy: bool) -> Result<u8, Error> {
        transaction(self, |s| {
            s.transfer(opcode)?;
            if dummy {
                s.transfer(0)?;
            }
            s.transfer(value)
        })
    }
    fn read_buffer(&mut self, opcode: u8, out: &mut [u8]) -> Result<(), Error> {
        transaction(self, |s| {
            s.transfer(opcode)?;
            for byte in out {
                *byte = s.transfer(0)?;
            }
            Ok(())
        })
    }
    fn write_buffer(&mut self, opcode: u8, bytes: &[u8]) -> Result<(), Error> {
        transaction(self, |s| {
            s.transfer(opcode)?;
            for &byte in bytes {
                s.transfer(byte)?;
            }
            Ok(())
        })
    }
    fn delay_us(&mut self, us: u16) {
        SpiBus::delay_us(self, us);
    }
}

fn transaction<S: SpiBus + ?Sized, R>(
    spi: &mut S,
    body: impl FnOnce(&mut S) -> Result<R, Error>,
) -> Result<R, Error> {
    let result = match spi.select(true) {
        Ok(()) => body(spi),
        Err(error) => Err(error),
    };
    let end = spi.select(false);
    result.and_then(|value| end.map(|_| value))
}

pub trait SpiPins {
    fn cs(&mut self, high: bool);
    fn clock(&mut self, high: bool);
    fn mosi(&mut self, high: bool);
    fn miso(&mut self) -> bool;
    fn half_cycle(&mut self);
    fn delay_us(&mut self, us: u16);
}

/// Mode 0, MSB first. CS remains asserted across command/data bytes. No IRQ
/// masking is performed; ownership prevents another SPI user from entering.
pub struct SoftwareSpi<P> {
    pins: P,
    selected: bool,
}
impl<P: SpiPins> SoftwareSpi<P> {
    pub fn new(mut pins: P) -> Self {
        pins.cs(true);
        pins.clock(false);
        pins.mosi(false);
        Self {
            pins,
            selected: false,
        }
    }
    /// Transfer exclusive ownership of an idle bus to a device. On a selected
    /// bus, return the bus unchanged so its caller can finish the transaction.
    pub fn into_device(self) -> Result<SoftwareDevice<P>, Self> {
        if self.selected {
            Err(self)
        } else {
            Ok(SoftwareDevice { pins: self.pins })
        }
    }
    pub fn into_pins(self) -> P {
        self.pins
    }
}
impl<P: SpiPins> SpiBus for SoftwareSpi<P> {
    fn select(&mut self, active: bool) -> Result<(), Error> {
        if active && self.selected {
            return Err(Error::Busy);
        }
        self.pins.clock(false);
        self.pins.cs(!active);
        self.selected = active;
        Ok(())
    }
    fn transfer(&mut self, byte: u8) -> Result<u8, Error> {
        if !self.selected {
            return Err(Error::Invalid);
        }
        Ok(shift_byte(&mut self.pins, byte))
    }
    fn delay_us(&mut self, us: u16) {
        self.pins.delay_us(us);
    }
}

/// Exclusive software-SPI device: no externally accessible select/transfer
/// can leave a transaction open. GPIO operations are infallible, so its owned
/// transactions need neither per-byte state checks nor transport error paths.
pub struct SoftwareDevice<P> {
    pins: P,
}
impl<P> SoftwareDevice<P> {
    pub fn into_pins(self) -> P {
        self.pins
    }
}
impl<P: SpiPins> SoftwareDevice<P> {
    fn select(&mut self, active: bool) {
        self.pins.clock(false);
        self.pins.cs(!active);
    }
}
impl<P: SpiPins> SpiDevice for SoftwareDevice<P> {
    fn command(&mut self, opcode: u8, value: u8, dummy: bool) -> Result<u8, Error> {
        self.select(true);
        shift_byte(&mut self.pins, opcode);
        if dummy {
            shift_byte(&mut self.pins, 0);
        }
        let value = shift_byte(&mut self.pins, value);
        self.select(false);
        Ok(value)
    }
    fn read_buffer(&mut self, opcode: u8, out: &mut [u8]) -> Result<(), Error> {
        self.select(true);
        shift_byte(&mut self.pins, opcode);
        for byte in out {
            *byte = shift_byte(&mut self.pins, 0);
        }
        self.select(false);
        Ok(())
    }
    fn write_buffer(&mut self, opcode: u8, bytes: &[u8]) -> Result<(), Error> {
        self.select(true);
        shift_byte(&mut self.pins, opcode);
        for &byte in bytes {
            shift_byte(&mut self.pins, byte);
        }
        self.select(false);
        Ok(())
    }
    fn delay_us(&mut self, us: u16) {
        self.pins.delay_us(us);
    }
}
#[inline(never)]
fn shift_byte<P: SpiPins>(pins: &mut P, byte: u8) -> u8 {
    let mut input = 0u32;
    let mut output = byte as u32;
    for _ in 0..8 {
        pins.mosi(output & 0x80 != 0);
        output <<= 1;
        pins.half_cycle();
        pins.clock(true);
        pins.half_cycle();
        input = (input << 1) | pins.miso() as u32;
        pins.clock(false);
    }
    input as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;
    #[derive(Debug, PartialEq)]
    struct Pins {
        cs: bool,
        clock: bool,
        mosi: bool,
        wire: Vec<bool>,
        reads: usize,
        cs_edges: Vec<bool>,
        half_cycles: usize,
    }
    impl SpiPins for Pins {
        fn cs(&mut self, high: bool) {
            self.cs = high;
            self.cs_edges.push(high);
        }
        fn clock(&mut self, high: bool) {
            if high && !self.clock && !self.cs {
                self.wire.push(self.mosi);
            }
            self.clock = high;
        }
        fn mosi(&mut self, high: bool) {
            self.mosi = high;
        }
        fn miso(&mut self) -> bool {
            let value = 0x3cu8 & (1 << (7 - self.reads % 8)) != 0;
            self.reads += 1;
            value
        }
        fn half_cycle(&mut self) {
            self.half_cycles += 1;
        }
        fn delay_us(&mut self, _: u16) {}
    }
    #[test]
    fn mode_zero_msb_first_and_transaction_ownership() {
        let mut spi = SoftwareSpi::new(Pins {
            cs: false,
            clock: true,
            mosi: false,
            wire: Vec::new(),
            reads: 0,
            cs_edges: Vec::new(),
            half_cycles: 0,
        });
        assert_eq!(spi.transfer(0xa5), Err(Error::Invalid));
        spi.select(true).unwrap();
        assert_eq!(spi.select(true), Err(Error::Busy));
        assert_eq!(spi.transfer(0xa5), Ok(0x3c));
        spi.select(false).unwrap();
        let pins = spi.into_pins();
        assert!(pins.cs);
        assert!(!pins.clock);
        assert_eq!(
            pins.wire,
            [true, false, true, false, false, true, false, true]
        );
        assert_eq!(pins.reads, 8);
    }
    fn pins() -> Pins {
        Pins {
            cs: false,
            clock: true,
            mosi: false,
            wire: Vec::new(),
            reads: 0,
            cs_edges: Vec::new(),
            half_cycles: 0,
        }
    }
    // The reference exposes only the original byte interface, so its new
    // transaction methods use the fallible default implementations.
    struct ByteBus(SoftwareSpi<Pins>);
    impl SpiBus for ByteBus {
        fn select(&mut self, active: bool) -> Result<(), Error> {
            self.0.select(active)
        }
        fn transfer(&mut self, byte: u8) -> Result<u8, Error> {
            self.0.transfer(byte)
        }
        fn delay_us(&mut self, us: u16) {
            SpiBus::delay_us(&mut self.0, us);
        }
    }
    fn sequence(bus: &mut impl SpiDevice) {
        assert_eq!(bus.command(0x19, 0xa5, false), Ok(0x3c));
        assert_eq!(bus.command(0x04, 0, true), Ok(0x3c));
        let mut out = [0; 3];
        bus.read_buffer(0x3a, &mut out).unwrap();
        assert_eq!(out, [0x3c; 3]);
        bus.write_buffer(0x7a, &[1, 2, 0xff]).unwrap();
        bus.read_buffer(0x3a, &mut []).unwrap();
        bus.write_buffer(0xff, &[]).unwrap();
    }
    #[test]
    fn specialized_transactions_match_byte_bus_wire_cs_dummy_and_clock_edges() {
        let mut reference = ByteBus(SoftwareSpi::new(pins()));
        let mut specialized = SoftwareSpi::new(pins()).into_device().ok().unwrap();
        sequence(&mut reference);
        sequence(&mut specialized);
        let a = reference.0.into_pins();
        let b = specialized.into_pins();
        assert_eq!(a, b);
        let bytes: Vec<_> = b
            .wire
            .chunks_exact(8)
            .map(|bits| bits.iter().fold(0u8, |n, bit| (n << 1) | u8::from(*bit)))
            .collect();
        assert_eq!(
            bytes,
            [0x19, 0xa5, 0x04, 0, 0, 0x3a, 0, 0, 0, 0x7a, 1, 2, 0xff, 0x3a, 0xff]
        );
        assert_eq!(b.reads, bytes.len() * 8);
        assert_eq!(b.half_cycles, bytes.len() * 16);
        assert!(b.cs && !b.clock);
        assert_eq!(
            b.cs_edges,
            [true]
                .into_iter()
                .chain([false, true].repeat(6))
                .collect::<Vec<_>>()
        );
    }
    struct FailingBus {
        selects: Vec<bool>,
        bytes: Vec<u8>,
        fail_at: Option<usize>,
        fail_select: bool,
        fail_end: bool,
    }
    impl SpiBus for FailingBus {
        fn select(&mut self, active: bool) -> Result<(), Error> {
            self.selects.push(active);
            if active && self.fail_select {
                Err(Error::Busy)
            } else if !active && self.fail_end {
                Err(Error::Io)
            } else {
                Ok(())
            }
        }
        fn transfer(&mut self, byte: u8) -> Result<u8, Error> {
            self.bytes.push(byte);
            if self.fail_at == Some(self.bytes.len() - 1) {
                Err(Error::Timeout)
            } else {
                Ok(0x3c)
            }
        }
        fn delay_us(&mut self, _: u16) {}
    }
    #[test]
    fn default_transactions_cleanup_every_failure_and_keep_the_first_error() {
        for operation in 0..3 {
            let count = if operation == 0 { 3 } else { 4 };
            for failed in 0..count + 2 {
                let mut bus = FailingBus {
                    selects: Vec::new(),
                    bytes: Vec::new(),
                    fail_at: (failed < count).then_some(failed),
                    fail_select: failed == count,
                    fail_end: true,
                };
                let mut out = [0; 3];
                let result = match operation {
                    0 => bus.command(0x04, 0xa5, true).map(|_| ()),
                    1 => bus.read_buffer(0x3a, &mut out),
                    _ => bus.write_buffer(0x7a, &[1, 2, 3]),
                };
                let expected = if failed < count {
                    Error::Timeout
                } else if failed == count {
                    Error::Busy
                } else {
                    Error::Io
                };
                assert_eq!(result, Err(expected));
                assert_eq!(bus.selects, [true, false]);
                assert_eq!(
                    bus.bytes.len(),
                    if failed < count {
                        failed + 1
                    } else if failed == count {
                        0
                    } else {
                        count
                    }
                );
            }
        }
    }
    #[test]
    fn exclusive_device_requires_idle_bus_and_failed_conversion_preserves_owner() {
        let mut bus = SoftwareSpi::new(pins());
        bus.select(true).unwrap();
        let mut bus = match bus.into_device() {
            Err(bus) => bus,
            Ok(_) => panic!("selected bus transferred"),
        };
        assert_eq!(bus.transfer(0xa5), Ok(0x3c));
        bus.select(false).unwrap();
        let mut device = bus.into_device().ok().unwrap();
        assert_eq!(device.command(0x19, 0, false), Ok(0x3c));
        assert!(device.into_pins().cs);
    }
}
