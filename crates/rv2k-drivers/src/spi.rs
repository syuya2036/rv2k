use rv2k_api::syscall::Error;

pub trait SpiBus {
    fn select(&mut self, active: bool) -> Result<(), Error>;
    fn transfer(&mut self, byte: u8) -> Result<u8, Error>;
    /// Must delay at least the specified microseconds, including after reset.
    fn delay_us(&mut self, us: u16);
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
        let mut input = 0;
        for shift in (0..8).rev() {
            self.pins.mosi(byte & (1 << shift) != 0);
            self.pins.half_cycle();
            self.pins.clock(true);
            self.pins.half_cycle();
            input = (input << 1) | self.pins.miso() as u8;
            self.pins.clock(false);
        }
        Ok(input)
    }
    fn delay_us(&mut self, us: u16) {
        self.pins.delay_us(us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;
    struct Pins {
        cs: bool,
        clock: bool,
        mosi: bool,
        wire: Vec<bool>,
        reads: usize,
    }
    impl SpiPins for Pins {
        fn cs(&mut self, high: bool) {
            self.cs = high;
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
        fn half_cycle(&mut self) {}
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
}
