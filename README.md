# StarRailSim Sync

Keeps [StarRailSim](https://starrailsim.com)'s relic inventory in step with your Honkai: Star Rail account. It
reads the game's own network traffic on your PC, so every relic, light cone and character comes in without an
export file, and changes you make in game show up on the site as they happen.

Windows only. Nothing leaves your PC: the site connects to the app at `ws://127.0.0.1:23314/ws`, and only
StarRailSim's own pages may open that socket.

## Using it

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

## Building

```
cargo build --release
```

The build downloads the game's resource tables (through Reliquary Archiver's build script), so it needs network
access.

**Every game patch** changes the game's protocol. Once Reliquary Archiver has released for the patch:

1. `.\tools\update-exporter.ps1 <archiver tag>` re-copies its exporter into `src/export` and prints the
   `reliquary` tag that release builds on.
2. Set that tag on both `reliquary` entries in `Cargo.toml` (dependency and build-dependency).
3. If the script warns that the archiver's `build.rs` changed, mirror its downloads in ours.
4. `cargo test`, `cargo build --release`, and publish the new `starrailsim-sync.exe`.

Until then a new client's traffic cannot be decoded.

`src/export` is vendored rather than used as a dependency because the archiver's build script links its own
icon and version details into anything that depends on it.

## Credits

The packet decoding and the inventory export are [Reliquary](https://github.com/IceDynamix/reliquary) and
[Reliquary Archiver](https://github.com/IceDynamix/reliquary-archiver) by IceDynamix and contributors (MIT).
`src/export` is copied from Reliquary Archiver unchanged, and `src/capture.rs` and `build.rs` are adapted
from it.
