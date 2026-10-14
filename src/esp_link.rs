/* SPI link to the ESP32-C3 radio (WiFi cartridge Rev B), master side, protocol version 1.
 *
 * Spec: wifi-game-boy-cartridge docs/ARCHITECTURE.md, "SPI link protocol". This is a line-by-line port of the
 * reference master in firmware/esp32c3/components/esp_link/esp_link.c of that repository; the host test
 * (tests-host/) replays the transcript the reference writes (sim/system/el_vectors.c) and checks every byte.
 * core only, no allocation, no hardware: the caller does the SPI transfers and owns the timing.
 */

pub const HDR: usize = 12;
pub const MAX_REQ: usize = 240;
#[allow(dead_code)]
pub const MAX_RESP: usize = 255;
pub const MAX_XFER: usize = HDR + 260;
pub const GAP_US: u64 = 100;

const M_MAGIC: u8 = 0xC5;
const S_MAGIC: u8 = 0x5C;
const POLL: u8 = 0;
const REQUEST: u8 = 1;
const F_BUSY: u8 = 2;
const F_RESP: u8 = 4;
const F_ACCEPTED: u8 = 8;

const NO_RADIO_MS: u32 = 5000;
const RADIO_TIMEOUT_MS: u32 = 12000;
const LIVE_STALE_MS: u32 = 1000;
const RESEND_MS: u32 = 10;

/* Mailbox (cart RAM bank 15) offsets */
pub const MB_CMD: usize = 0x00;
pub const MB_SEQ: usize = 0x01;
pub const MB_STATUS: usize = 0x02;
pub const MB_ACK: usize = 0x03;
pub const MB_LENL: usize = 0x04;
pub const MB_LENH: usize = 0x05;
pub const MB_RAIL: usize = 0x06;
pub const MB_RES: usize = 0x07;
pub const MB_REQ: usize = 0x10;
pub const MB_RESP: usize = 0x100;
/// Mailbox bytes cleared at start: header, request, response.
pub const MB_CLEAR: usize = 0x200;

/// The 0x2000-byte mailbox bank. On the cartridge the Game Boy writes it through DMA at any time, so the
/// implementation must use volatile accesses.
pub trait Mailbox {
    fn get(&self, off: usize) -> u8;
    fn set(&mut self, off: usize, v: u8);
}

pub fn crc16(p: &[u8]) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in p {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// Bytes a body of n payload bytes takes on the wire (payload + CRC, padded to 4); 0 for n == 0.
pub fn body_len(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        (n + 2 + 3) & !3
    }
}

fn put_crc(buf: &mut [u8], at: usize, from: usize, n: usize) {
    let c = crc16(&buf[from..from + n]);
    buf[at] = (c >> 8) as u8;
    buf[at + 1] = c as u8;
}

fn crc_ok(p: &[u8], n: usize) -> bool {
    let c = crc16(&p[..n]);
    p[n] == (c >> 8) as u8 && p[n + 1] == c as u8
}

fn hdr_ok(h: &[u8], magic: u8) -> bool {
    h.len() >= HDR && h[0] == magic && crc_ok(h, 10)
}

#[derive(Default)]
pub struct Master {
    last_seen_seq: u8,
    pending: bool,
    accepted: bool,
    seq: u8,
    ack_valid: bool,
    ack: u8,
    fetch_due: bool,
    fetch_len: u8,
    ever_valid: bool,
    session: u8,
    t_pending_ms: u32,
    t_valid_ms: u32,
    t_accept_ms: u32,
    t_req_ms: u32,
    pub xfers: u32,
    pub bad_hdr: u32,
    pub bad_body: u32,
    pub resent: u32,
}

impl Master {
    /// Clears A000-A1FF. Call before the Game Boy runs. session: random, new on every boot (the ESP may stay
    /// powered through an RP2350 restart and must drop its old job); 0 is replaced by 1.
    pub fn new(mb: &mut impl Mailbox, now_ms: u32, session: u8) -> Self {
        for i in 0..MB_CLEAR {
            mb.set(i, 0);
        }
        Master {
            session: if session == 0 { 1 } else { session },
            t_pending_ms: now_ms,
            t_valid_ms: now_ms,
            t_accept_ms: now_ms,
            t_req_ms: now_ms,
            ..Default::default()
        }
    }

    fn finish(&mut self, mb: &mut impl Mailbox, status: u8, data: &[u8]) {
        for i in MB_REQ..MB_RESP {
            mb.set(i, 0); // the request may hold a WiFi password: keep it out of the .sav
        }
        for (i, &b) in data.iter().enumerate() {
            mb.set(MB_RESP + i, b);
        }
        mb.set(MB_RESP + data.len(), 0);
        mb.set(MB_LENL, data.len() as u8);
        mb.set(MB_LENH, (data.len() >> 8) as u8);
        mb.set(MB_STATUS, status);
        mb.set(MB_ACK, self.seq); // last: the ROM polls ACK
        self.pending = false;
        self.accepted = false;
        self.fetch_due = false;
        self.ack_valid = true;
        self.ack = self.seq;
    }

    fn master_hdr(&self, tx: &mut [u8], typ: u8) {
        tx[0] = M_MAGIC;
        tx[1] = typ;
        tx[2] = if self.ack_valid { 1 } else { 0 };
        tx[3] = if self.ack_valid { self.ack } else { 0 };
        tx[7] = self.session;
    }

    /// Builds the next transaction into tx; returns its length (a multiple of 4).
    pub fn next(&mut self, now_ms: u32, mb: &mut impl Mailbox, tx: &mut [u8; MAX_XFER]) -> usize {
        let s = mb.get(MB_SEQ);
        if s != self.last_seen_seq {
            self.last_seen_seq = s;
            self.pending = true;
            self.accepted = false;
            self.fetch_due = false;
            self.seq = s;
            self.t_pending_ms = now_ms;
            self.t_req_ms = now_ms.wrapping_sub(RESEND_MS);
            mb.set(MB_STATUS, 1);
        }
        if self.pending && !self.accepted && now_ms.wrapping_sub(self.t_pending_ms) >= NO_RADIO_MS {
            let msg: &[u8] = if self.ever_valid && now_ms.wrapping_sub(self.t_valid_ms) < NO_RADIO_MS {
                b"RADIO NOT ACCEPTING"
            } else {
                b"NO RADIO POWER"
            };
            self.finish(mb, 3, msg);
        } else if self.pending && self.accepted && now_ms.wrapping_sub(self.t_accept_ms) >= RADIO_TIMEOUT_MS {
            self.finish(mb, 3, b"RADIO TIMEOUT");
        }
        if now_ms.wrapping_sub(self.t_valid_ms) >= LIVE_STALE_MS {
            mb.set(MB_RAIL, 0);
            mb.set(MB_RES, 0);
        }

        tx.fill(0);
        let mut n = HDR;
        if self.fetch_due {
            self.master_hdr(tx, POLL);
            n += body_len(self.fetch_len as usize);
        } else if self.pending && !self.accepted && now_ms.wrapping_sub(self.t_req_ms) >= RESEND_MS {
            let mut len = 0usize;
            while len < MAX_REQ && mb.get(MB_REQ + len) != 0 {
                len += 1;
            }
            self.master_hdr(tx, REQUEST);
            tx[4] = self.seq;
            tx[5] = mb.get(MB_CMD);
            tx[6] = len as u8;
            if len > 0 {
                for i in 0..len {
                    tx[HDR + i] = mb.get(MB_REQ + i);
                }
                put_crc(tx, HDR + len, HDR, len);
                n += body_len(len);
            }
            if now_ms != self.t_pending_ms {
                self.resent += 1;
            }
            self.t_req_ms = now_ms;
        } else {
            self.master_hdr(tx, POLL);
        }
        put_crc(tx, 10, 0, 10);
        n
    }

    /// The transaction finished: rx holds the bytes the ESP sent (same length as the transaction).
    pub fn done(&mut self, now_ms: u32, mb: &mut impl Mailbox, rx: &[u8]) {
        let was_fetch = self.fetch_due;
        self.fetch_due = false;
        self.xfers += 1;
        if !hdr_ok(rx, S_MAGIC) {
            self.bad_hdr += 1;
            return;
        }
        self.ever_valid = true;
        self.t_valid_ms = now_ms;
        mb.set(MB_RAIL, rx[6]);
        mb.set(MB_RES, rx[7]);
        if !self.pending {
            return;
        }
        let f = rx[1];
        if !self.accepted {
            if f & F_ACCEPTED != 0 && rx[8] == self.seq {
                self.accepted = true;
                self.t_accept_ms = now_ms;
            } else {
                return;
            }
        }
        let mine = rx[3] == self.seq;
        if f & F_RESP != 0 && mine {
            let len = rx[5] as usize;
            if len == 0 {
                self.finish(mb, rx[4], &[]);
            } else if was_fetch && rx.len() >= HDR + body_len(len) {
                if crc_ok(&rx[HDR..], len) {
                    let status = rx[4];
                    self.finish(mb, status, &rx[HDR..HDR + len]);
                } else {
                    self.bad_body += 1;
                }
            } else {
                self.fetch_due = true;
                self.fetch_len = len as u8;
            }
        } else if !(f & F_BUSY != 0 && mine) {
            self.accepted = false; // the ESP lost the job (restart): send the request again
            self.t_req_ms = now_ms.wrapping_sub(RESEND_MS);
        }
    }

    /// True if another transaction is due in this tick (a response fetch), after GAP_US.
    pub fn more(&self) -> bool {
        self.fetch_due
    }
}
