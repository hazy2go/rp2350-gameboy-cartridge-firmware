# Link tester (Raspberry Pi Pico 2 + XIAO ESP32C3)

Tests the real SPI link before the cart exists: the cartridge's master code (`../src/esp_link.rs`, unchanged) runs on an RP2350 (Pico 2, RP2350A) against the real radio app on a XIAO ESP32C3. A USB serial console stands in for the Game Boy.

**Wiring** (both 3.3 V, plus GND to GND):

| Pico 2 | XIAO ESP32C3 | signal |
|---|---|---|
| GPIO2 | D4 (IO6) | SCK |
| GPIO3 | D5 (IO7) | MOSI |
| GPIO0 | D0 (IO2) | MISO |
| GPIO5 | D10 (IO10) | CS (on the cart: GPIO46, which the RP2350A does not have) |
| GND | GND | |

**XIAO:** build the radio app with the XIAO profile and flash it (wifi-game-boy-cartridge, `firmware/esp32c3/app`):
```
idf.py -B build_xiao -D SDKCONFIG=sdkconfig_xiao -D "SDKCONFIG_DEFAULTS=sdkconfig.defaults;sdkconfig.xiao" build
idf.py -B build_xiao -p /dev/cu.usbmodemXXXX flash
```
On its USB console, `wifi <ssid> <password>` stores the credentials.

**Pico 2:**
1. `cargo build --release` in this folder.
2. `picotool uf2 convert target/thumbv8m.main-none-eabihf/release/link-tester -t elf link-tester.uf2 --family rp2350-arm-s`.
3. Hold BOOTSEL, plug in, and copy the .uf2 to the drive that appears.

**Use:** `screen /dev/cu.usbmodemXXXX` (the Pico's port) and type requests:
```
1                          -> status 2 in 4 ms: PONG
4                          -> status 2 in ...: RAIL 0.00V RES 4.60V   (the XIAO profile's fixed readings)
2                          -> 2026-10-14 21:05:33
3 https://example.org/     -> the first 255 bytes ('\n' shown as |)
5                          -> SCAN lines
stats                      -> xfers / bad_hdr / bad_body / resent
```
**What to look for:**
- `bad_hdr` should stay near 0 once the XIAO is running.
- Any CRC failures at 8 MHz show up as `bad_hdr`, `bad_body` or `resent`, and say whether the 100 us gap and SPI mode 0 hold on real silicon. Log the numbers in the hardware repo's LOGBOOK.
- No level shifting or MISO buffer is needed here: there is only one slave on the bus.
