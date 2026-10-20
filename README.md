# StarRailSim Sync

Keeps [StarRailSim](https://starrailsim.com)'s relic inventory in step with your Honkai: Star Rail account. It
reads the game's own network traffic on your PC, so every relic, light cone and character comes in without an
export file, and changes you make in game show up on the site as they happen.

Windows and Linux (the Steam Deck included). Nothing leaves your computer: the site connects to the app at
`ws://127.0.0.1:23314/ws`, and only StarRailSim's own pages may open that socket.

## Using it on Windows

1. Run `starrailsim-sync.exe` and accept the administrator prompt. Reading packets with Windows' built-in
   packet monitor (pktmon) needs it.
2. It installs itself to `%LOCALAPPDATA%\Programs\StarRailSim Sync`, adds a Start menu shortcut and an entry
   under Settings > Apps > Installed apps, registers to start when you sign in (a scheduled task with highest
   privileges, so there is no prompt at each sign-in), then opens its window and StarRailSim's Import page.
   Turn on sync there.
3. Start the game and click to start. The game only hands out its session key at login, so if you were
   already in game, log out and back in.

The window says what it is doing and what to do next; closing it keeps sync running in the notification area.
The tray icon is grey while it waits for the game, teal once your account is synced, and amber when it needs
you (log back in, or something stopped it). Left-click it, or use the Start menu shortcut, to bring the window
back; neither asks for administrator access again.

To update, download the new version and run it: it asks the running copy to quit and installs over it.
Uninstall from Settings > Apps > Installed apps, or with **Uninstall** in the window: both remove the program,
the sign-in task, the shortcut and the logs.

Logs: `%LOCALAPPDATA%\StarRailSim Sync\sync.log`.

## Using it on Linux

One file, `starrailsim-sync`, a static x86_64 build that runs on any distribution with nothing else installed.

1. Mark it executable (file manager: Properties > Permissions, "Allow executing as program") and open it.
2. It installs itself to `~/.local/share/starrailsim-sync`, adds itself to the app menu and to the desktop's
   login autostart, then asks for your password once, through the desktop's own prompt (polkit), to give the
   installed copy one capability: CAP_NET_RAW, for reading packets. It opens StarRailSim's Import page; turn
   on sync there.
3. Start the game and click to start, as on Windows.

The capability stays on the file, so later starts ask nothing; the app opens its packet socket first thing,
then drops the capability. Its tray icon (KDE and the Steam Deck's desktop mode, or GNOME with the
AppIndicator extension Ubuntu ships) works like the Windows one. Its window is the same page, opened in your
browser from the app itself at `http://127.0.0.1:23314/` with a token only the app hands out; starting the app
again from the app menu opens it, tray or not. An update replaces the file, so it asks for the password again.
Uninstall from the app menu entry's **Uninstall** action, or with **Uninstall** in the window.

On a Steam Deck the `deck` user has no password until you set one (`passwd` in Konsole), and the app runs in
desktop mode only. Logs and the saved account: `~/.local/state/starrailsim-sync`.

## Options

| Argument | Effect |
| --- | --- |
| `--site <url>` | Page to open (default `https://starrailsim.com/import`) |
| `--allow-origin <origin>` | Also accept this page origin on the socket (repeatable) |
| `--portable` | Run from where it is: no install, no sign-in task |
| `--replay <export.json>` | Serve a saved Reliquary Archiver export instead of capturing; needs no admin |
| `--headless` | No tray icon and no window (for automated checks) |
| `--uninstall` | Remove the installed copy (what Installed apps runs) |
| `--background` | Start without the window (what the sign-in task passes) |
| `--emulator` | Test against the PearlSR emulator instead of the live game (no admin, nothing installed) |
| `--grant-capture` | Linux: what the password prompt runs, as root, to give the installed copy CAP_NET_RAW |

## Testing without a live account

Start the PearlSR lab stack with the client (`hsr-harness\restart-lab.ps1 -LaunchClient`), then run
`starrailsim-sync.exe --emulator` before the client logs in. PearlSR speaks the live game's protocol (same
command ids and protobufs as the `reliquary` tag here) but over plain TCP on loopback with no encryption, which
packet capture cannot see. So this mode relays the client's connection instead: it listens on
`127.0.0.1:23301`, which Windows prefers over PearlSR's `0.0.0.0:23301`, passes everything through to PearlSR
on `127.0.0.2`, and reads the account on the way. Relics that arrive without substats are configured that way
in PearlSR's `config.json`.

## Building

```
cargo build --release
```

The build downloads the game's resource tables (through Reliquary Archiver's build script), so it needs network
access.

Linux, in Docker (the image is the build environment and the test bench), from the repo:

```
docker build -t starrailsim-sync-linux -f tools/linux.Dockerfile tools
docker run --rm --cpus 2 -v "$PWD:/src:ro" -v srs-cargo:/usr/local/cargo/registry -v srs-git:/usr/local/cargo/git \
  -v srs-target:/target -e CARGO_TARGET_DIR=/target -w /src starrailsim-sync-linux \
  cargo build --locked --release --target x86_64-unknown-linux-musl
```

`tools/linux-e2e.sh <binary> decoder/tests/emulator-login.pcap`, run as root in the same image, tests the
whole Linux path: a normal user's first run installs and is granted the capability (through a stand-in
`pkexec`), the recorded emulator login is replayed onto loopback, and the site's socket and the window page
must both have the account; then it quits and uninstalls. The packet socket's own test needs CAP_NET_RAW:
`cargo test -- --ignored` as root there.

**Every game patch** changes the game's protocol. Once Reliquary Archiver has released for the patch:

1. `.\tools\update-exporter.ps1 <archiver tag>` re-copies its exporter into `core/src/export` and prints the
   `reliquary` tag that release builds on.
2. Set that tag on every `reliquary` entry: `Cargo.toml`, `core/Cargo.toml` (dependency and
   build-dependency) and `decoder/Cargo.toml`.
3. If the script warns that the archiver's `build.rs` changed, mirror its downloads in `core/build.rs`.
4. `cargo test`, the release builds, and publish the new `starrailsim-sync.exe` and Linux `starrailsim-sync`.

Until then a new client's traffic cannot be decoded.

`core/src/export` is vendored rather than used as a dependency because the archiver's build script links its
own icon and version details into anything that depends on it.

## The capture-file decoder

`decoder/` is the same decoding for a browser: it reads a capture of the game logging in (pcap or pcapng, from
PCAPdroid on Android, tcpdump, Wireshark or rvictl) and returns the account as the archiver's export. The site
runs it in a Web Worker, so the capture never leaves the reader's device.

```
cargo build -p sync-decoder --target wasm32-wasip1 --release
```

then, in the site's repo, `node scripts/generate-sync-decoder.mjs <path to this repo>` copies the module in.
`decoder/tests/emulator-login.pcap` is a PearlSR login recorded through `--emulator --record` (server side
only) and is decoded by `cargo test -p sync-decoder`.

## Credits

The packet decoding and the inventory export are [Reliquary](https://github.com/IceDynamix/reliquary) and
[Reliquary Archiver](https://github.com/IceDynamix/reliquary-archiver) by IceDynamix and contributors (MIT).
`core/src/export` is copied from Reliquary Archiver unchanged, and `src/capture.rs` and `core/build.rs` are
adapted from it.
