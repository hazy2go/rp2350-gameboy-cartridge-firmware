//! Replays the transcript written by the reference C implementation (wifi-game-boy-cartridge,
//! sim/system/el_vectors.c) and checks the Rust master byte for byte: every MOSI frame, the mailbox
//! header after every tick and every stored response. Regenerate the file with el_vectors and copy it here.
use croco_host_tests::esp_link::*;

struct Mb([u8; 0x2000]);
impl Mailbox for Mb {
    fn get(&self, off: usize) -> u8 {
        self.0[off]
    }
    fn set(&mut self, off: usize, v: u8) {
        self.0[off] = v;
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn crc_check_value() {
    assert_eq!(crc16(b"123456789"), 0x29B1);
}

#[test]
fn replay_reference_transcript() {
    let text = include_str!("esp_link_vectors.txt");
    let mut mb = Mb([0; 0x2000]);
    let mut m = Master::new(&mut mb, 0, 0xA7);
    let mut last: Option<(u32, Vec<u8>, Vec<u8>)> = None;
    let (mut xfers, mut checks) = (0, 0);
    let run = |m: &mut Master, mb: &mut Mb, ms: u32, mosi: &[u8], miso: &[u8], line: usize| {
        let mut tx = [0u8; MAX_XFER];
        let n = m.next(ms, mb, &mut tx);
        assert_eq!(&tx[..n], mosi, "MOSI differs at ms {ms} (line {line})");
        m.done(ms, mb, miso);
    };
    for (ln, l) in text.lines().enumerate() {
        let f: Vec<&str> = l.split(' ').collect();
        match f[0] {
            "W" => {
                let off = usize::from_str_radix(f[2], 16).unwrap();
                mb.0[off] = u8::from_str_radix(f[3], 16).unwrap();
            }
            "X" => {
                let ms: u32 = f[1].parse().unwrap();
                let (mosi, miso) = (unhex(f[2]), unhex(f[3]));
                run(&mut m, &mut mb, ms, &mosi, &miso, ln + 1);
                xfers += 1;
                last = Some((ms, mosi, miso));
            }
            "R" => {
                let (ms0, mosi, miso) = last.clone().unwrap();
                let n: u32 = f[1].parse().unwrap();
                for k in 1..=n {
                    run(&mut m, &mut mb, ms0 + k, &mosi, &miso, ln + 1);
                    xfers += 1;
                }
                last = Some((ms0 + n, mosi, miso));
            }
            "M" => {
                assert_eq!(&mb.0[..MB_REQ], &unhex(f[2])[..], "mailbox header differs at ms {} (line {})", f[1], ln + 1);
                checks += 1;
            }
            "P" => {
                let want = unhex(f[2]);
                assert_eq!(&mb.0[MB_RESP..MB_RESP + want.len()], &want[..], "response differs at ms {} (line {})", f[1], ln + 1);
                checks += 1;
            }
            "S" => {
                // the RP2350 restarted: new master, new session (the ESP keeps running)
                let ms: u32 = f[1].parse().unwrap();
                m = Master::new(&mut mb, ms, u8::from_str_radix(f[2], 16).unwrap());
                last = None;
            }
            _ => panic!("bad line {}", ln + 1),
        }
    }
    println!("{xfers} transactions, {checks} mailbox checks, master: {} bad hdr, {} bad body, {} resent",
             m.bad_hdr, m.bad_body, m.resent);
    assert!(xfers > 18000 && checks > 20);
}
