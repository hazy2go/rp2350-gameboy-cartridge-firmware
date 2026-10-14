/* Link tester: the cartridge's SPI master (../src/esp_link.rs, unchanged) on a Raspberry Pi Pico 2, talking to a
 * XIAO ESP32C3 that runs the radio app with the XIAO board profile. A USB serial console stands in for the Game
 * Boy: each line you type becomes a mailbox request, and the answer is printed.
 *
 *   <cmd> [payload]   e.g. "1", "2", "3 https://example.org/", "5", "6 2|password", "4"
 *   stats             link counters and the live rail/reservoir bytes
 *
 * Wiring (3.3 V logic on both, common GND):
 *   Pico 2 GPIO2 (SCK)  -> XIAO D4 (IO6)      Pico 2 GPIO3 (MOSI) -> XIAO D5 (IO7)
 *   Pico 2 GPIO0 (MISO) <- XIAO D0 (IO2)      Pico 2 GPIO5 (CS)   -> XIAO D10 (IO10)
 * Same SPI0 pins as the cart (GPIO0/2/3); CS is GPIO5 here because the RP2350A has no GPIO46.
 */
#![no_std]
#![no_main]

#[path = "../../src/esp_link.rs"]
mod esp_link;

use core::cell::RefCell;
use core::fmt::Write as _;

use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::spi::{self, Spi};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_rp::{bind_interrupts, pac};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::UsbDevice;
use esp_link::Mailbox;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

struct Mb([u8; 0x2000]);
impl Mailbox for Mb {
    fn get(&self, off: usize) -> u8 {
        self.0[off]
    }
    fn set(&mut self, off: usize, v: u8) {
        self.0[off] = v;
    }
}

struct Link {
    mb: Mb,
    master: esp_link::Master,
}

static LINK: StaticCell<Mutex<CriticalSectionRawMutex, RefCell<Link>>> = StaticCell::new();

fn session_byte() -> u8 {
    let mut acc = Instant::now().as_ticks() as u8;
    for _ in 0..16 {
        acc = acc.rotate_left(1) ^ pac::ROSC.randombit().read().randombit() as u8;
    }
    if acc == 0 { 1 } else { acc }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let mut cs = Output::new(p.PIN_5, Level::High);
    let mut cfg = spi::Config::default();
    cfg.frequency = 8_000_000; // as on the cart; mode 0
    let spi = Spi::new_blocking(p.SPI0, p.PIN_2, p.PIN_3, p.PIN_0, cfg);

    let mut mb = Mb([0; 0x2000]);
    let master = esp_link::Master::new(&mut mb, Instant::now().as_millis() as u32, session_byte());
    let link = LINK.init(Mutex::new(RefCell::new(Link { mb, master })));

    let driver = Driver::new(p.USB, Irqs);
    let mut config = embassy_usb::Config::new(0x2e8a, 0x000a);
    config.manufacturer = Some("wifi-gb-cart");
    config.product = Some("link tester");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        embassy_usb::Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            &mut [],
            CONTROL_BUF.init([0; 64]),
        )
    };
    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64)
    };
    unwrap!(spawner.spawn(usb_task(builder.build())));
    unwrap!(spawner.spawn(console_task(class, link)));

    // the link loop: the same steps as esp_link_task in the cart firmware (src/main.rs)
    let mut tx = [0u8; esp_link::MAX_XFER];
    let mut rx = [0u8; esp_link::MAX_XFER];
    let mut spi = spi;
    loop {
        Timer::after(Duration::from_millis(1)).await;
        let now = Instant::now().as_millis() as u32;
        for k in 0..2 {
            if k > 0 {
                Timer::after_micros(esp_link::GAP_US).await;
            }
            let n = link.lock(|l| {
                let mut l = l.borrow_mut();
                let Link { mb, master } = &mut *l;
                master.next(now, mb, &mut tx)
            });
            cs.set_low();
            let ok = spi.blocking_transfer(&mut rx[..n], &tx[..n]).is_ok();
            cs.set_high();
            if !ok {
                rx[..n].fill(0);
            }
            let more = link.lock(|l| {
                let mut l = l.borrow_mut();
                let Link { mb, master } = &mut *l;
                master.done(now, mb, &rx[..n]);
                master.more()
            });
            if !more {
                break;
            }
        }
    }
}

type UsbDev = UsbDevice<'static, Driver<'static, USB>>;

#[embassy_executor::task]
async fn usb_task(mut usb: UsbDev) -> ! {
    usb.run().await
}

struct Out([u8; 512], usize);
impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.1 < self.0.len() {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
        Ok(())
    }
}

async fn send(class: &mut CdcAcmClass<'static, Driver<'static, USB>>, o: &Out) {
    for chunk in o.0[..o.1].chunks(64) {
        let _ = class.write_packet(chunk).await;
    }
}

#[embassy_executor::task]
async fn console_task(
    mut class: CdcAcmClass<'static, Driver<'static, USB>>,
    link: &'static Mutex<CriticalSectionRawMutex, RefCell<Link>>,
) {
    let mut line = [0u8; 250];
    let mut len = 0usize;
    let mut seq: u8 = 0;
    loop {
        class.wait_connection().await;
        let mut buf = [0u8; 64];
        while let Ok(n) = class.read_packet(&mut buf).await {
            for &c in &buf[..n] {
                if c != b'\r' && c != b'\n' {
                    if len < line.len() {
                        line[len] = c;
                        len += 1;
                    }
                    continue;
                }
                if len == 0 {
                    continue;
                }
                let text = core::str::from_utf8(&line[..len]).unwrap_or("");
                let mut o = Out([0; 512], 0);
                if text == "stats" {
                    link.lock(|l| {
                        let l = l.borrow();
                        let m = &l.master;
                        let _ = write!(o, "xfers {} bad_hdr {} bad_body {} resent {} rail {} res {} (x20 mV)\r\n",
                                       m.xfers, m.bad_hdr, m.bad_body, m.resent, l.mb.0[esp_link::MB_RAIL],
                                       l.mb.0[esp_link::MB_RES]);
                    });
                    send(&mut class, &o).await;
                } else {
                    let (cmd, payload) = match text.split_once(' ') {
                        Some((c, p)) => (c, p),
                        None => (text, ""),
                    };
                    let cmd: u8 = cmd.parse().unwrap_or(0);
                    seq = seq.wrapping_add(1);
                    let t0 = Instant::now();
                    // as a ROM does it: payload, CMD, then SEQ last
                    link.lock(|l| {
                        let mut l = l.borrow_mut();
                        let pl = payload.as_bytes();
                        let n = pl.len().min(esp_link::MAX_REQ);
                        l.mb.0[esp_link::MB_REQ..esp_link::MB_REQ + n].copy_from_slice(&pl[..n]);
                        l.mb.0[esp_link::MB_REQ + n] = 0;
                        l.mb.0[esp_link::MB_CMD] = cmd;
                        l.mb.0[esp_link::MB_SEQ] = seq;
                    });
                    let mut status = 0u8;
                    while t0.elapsed() < Duration::from_secs(13) {
                        Timer::after_millis(16).await; // once per Game Boy frame
                        status = link.lock(|l| {
                            let l = l.borrow();
                            if l.mb.0[esp_link::MB_ACK] == seq && l.mb.0[esp_link::MB_STATUS] >= 2 {
                                l.mb.0[esp_link::MB_STATUS]
                            } else {
                                0
                            }
                        });
                        if status != 0 {
                            break;
                        }
                    }
                    let ms = t0.elapsed().as_millis();
                    link.lock(|l| {
                        let l = l.borrow();
                        if status == 0 {
                            let _ = write!(o, "TIMEOUT after {} ms\r\n", ms);
                        } else {
                            let r = &l.mb.0[esp_link::MB_RESP..esp_link::MB_RESP + 256];
                            let end = r.iter().position(|&b| b == 0).unwrap_or(255);
                            let _ = write!(o, "status {} in {} ms: ", status, ms);
                            for &b in &r[..end] {
                                let _ = o.write_char(if b == b'\n' { '|' } else if (32..127).contains(&b) { b as char } else { '?' });
                            }
                            let _ = o.write_str("\r\n");
                        }
                    });
                    send(&mut class, &o).await;
                }
                len = 0;
            }
        }
    }
}
