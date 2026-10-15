# WiFi Game Boy Cartridge: RP2350 firmware

Firmware for the cartridge side of the **WiFi Game Boy Cartridge** (Rev B): an RP2350B serves the whole Game Boy cartridge bus, loads ROMs from microSD, and talks to an ESP32-C3 radio over SPI, so Game Boy ROMs can use WiFi. By Hazy, co-created with RemAI.

Hardware, simulation, radio firmware and the ROM developer kit: **https://github.com/hazy2go/wifi-game-boy-cartridge**

![WiFi cartridge Rev B, 3D render](images/wifi_cart_board.png)

A WiFi ROM on the simulated cartridge (WiFi scan, password entry, live BTC/USD chart):

![BTC demo on the simulated cartridge](images/wifi_cart_btc_demo.png)

## What this firmware adds to Croco
Built on the Croco Cartridge V2 firmware by shilga (credit and the original docs below).
- **ESP32-C3 link** (`src/esp_link.rs`): SPI master for protocol v1 (spec: `docs/ARCHITECTURE.md` in the hardware repo). Chip select on GPIO46, 8 MHz, polled every 1 ms from core 1. Serves the WiFi mailbox in MBC5 RAM bank 15, but only to ROMs that opt in: `WIFICART` anywhere in bank 0, MBC5, 128 KB RAM. How to write such a ROM: `docs/ROM_DEV.md` in the hardware repo.
- **Logging is defmt-RTT** over the SWD Tag-Connect. GPIO46 was the debug UART TX.
- GPIO46 is driven high right after reset, before the first SD or RTC access. The pin has no pull-up, and the RP2350 resets with pull-downs.
- **MBC5 RAM banking fix:** the bank select used `& 0x03`, so banks 4-15 of a 128 KB save aliased onto banks 0-3. It now masks with the bank count from the header.
- **Link tester** (`link-tester/`): the same master on a Pico 2 against a XIAO ESP32C3, for bench tests before the cart exists.

## Build and test
- Build (macOS or Linux): `GBDK_PATH=<gbdk-2020 4.5.0> cargo build --release`, Rust target `thumbv8m.main-none-eabihf`. The devcontainer below works too.
- Host tests: `cd tests-host && cargo test --target "$(rustc -vV | sed -n 's/host: //p')"`. They replay the reference transcript from the hardware repo (`sim/system/el_vectors.c`) and check the Rust master byte for byte.
- Flash over USB with picotool (see "How to flash the firmware" below).

## Status (2026-10-16)
The cart is not built yet. The radio side runs on a XIAO ESP32C3 on real WiFi; the link test with a Pico 2 is next (`link-tester/README.md`). Progress: `docs/LOGBOOK.md` in the hardware repo.

## License
GPL-3, as upstream (`LICENSE`). This fork's additions (`src/esp_link.rs`, `link-tester/`, `tests-host/`) are GPL-3 too.

---

# Upstream: Croco Cartridge V2 firmware (shilga)

Utilizing the power of PIO to create a open-source flashcartridge for the gameboy
![Photo of the Croco Cartridge PCB](images/cartridge_photo.jpg)

### How to play ROMs on the cartridge
Format a sd card with FAT32 and put the ROMs into the root folder of the cartridge. Put the sd card into the cartridge and put that into the Gameboy.
Select the ROM you want to play in the menu and press A.

### How to build
Open repository in VS Code with the [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers) 
addon installed. VS Code should ask if it shall open the workspace in the devcontainer. 

Open terminal in VS Code and build with Cargo:
```
cargo build --release
```

### How to flash the firmware
You can upload the new firmware via USB. The firmware is able to receive commands from picotool and jumps into the Pico bootloader.
This command will send the cartridge into bootloader and flashes a new ELF from an official release or custom build:
```
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/rp2350-gameboy-cartridge -f
```

### How does it work?
The ROM is loaded from the sd card to the Hyperram connected to he microcontroller.
The RP2350B has 12 PIO state-machines which are like small co-processors which can run arbitrary code. They are designed to be efficient in IO operations.
Which is what they are mostly doing in this cartridge. They follow the Gameboy cartridge parallel bus interface and detect if the cartridge needs
to spit out or receive some data. Those requests are forwarded to the ARM core through the FIFOs. There a cascade of DMAs will happily handle those
requests by fetching the data from the Hyperram.

### How do savegames work?
In original cartridges the savegames had been stored in a RAM which was using a coin cell battery to hold the data while the gameboy is switched off.
This has pros and cons. One of the biggest cons is that the savegame is lost if the battery dies. There was a big discussion on heartbroken users 
as the savegame of their beloved childhood Pokemon Blue/Red/Yellow was gone after 20 years. One has to remember that they started selling in 1998.
The biggest pro on the savegame RAM is that the save is instantaneous. And that is what the games expect. They do not wait for the save to be done.

Why this explanation? A sd card might not have those battery limitations on one side, but writing something to it takes a lot more time. This is why the
cartridge cannot store the savegame immedietly to the sd card. The button on the cartridge is used to trigger the storing of it. It will save the savegame 
to the sd card. This will not interrupt the Gameboy in any way. That is why there is a plan to do this periodically with a fixed time in the future, but this
is not implemented yet.

### How to setup the RTC?
There is a battery driven RTC on the cartridge which advances the clock while the Gameboy is off. The time needs to be setup initially. In the main menu (there
the list of ROMs is seen) press select and in this menu select RTC config. It's best to set this time to UTC, as this makes the savegames compatible to emulators
on PCs. I plan to implement timezone support in the future to make it more user friendly.

If no ROMs with RTC support are played there is no need to setup the RTC. The cartridge functions also without the coin-cell battery.

### Does LSDJ work?
Yes. 128k saves are supported. With the newest releases it also runs on the GBC.

### How can I get the needed hardware?
Find the Kicad project files in this [repo](https://github.com/shilga/rp-gameboy-cartridge-hw/tree/master/KiCad/V2/GameboyCartridgeV2.1). Or order one on [Tindie](https://www.tindie.com/products/zeraphim/rp2350b-based-gameboy-cartridge/).


### Discord
Find the Discord server to get help or discuss about this project. <https://discord.com/invite/jxGfqw2mdt>
