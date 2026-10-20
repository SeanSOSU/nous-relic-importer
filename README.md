# StarRailSim Sync

Keeps [StarRailSim](https://starrailsim.com)'s relic inventory in step with your Honkai: Star Rail account. It
reads the game's own network traffic on your PC, so every relic, light cone and character comes in without an
export file, and changes you make in game show up on the site as they happen.

Windows only. Nothing leaves your PC: the site connects to the app at `ws://127.0.0.1:23314/ws`, and only
StarRailSim's own pages may open that socket.

## Using it

1. Run `starrailsim-sync.exe` and accept the administrator prompt. Reading packets with Windows' built-in
   packet monitor (pktmon) needs it.
2. It installs itself to `%LOCALAPPDATA%\Programs\StarRailSim Sync`, registers to start when you sign in (a
   scheduled task with highest privileges, so there is no prompt at each sign-in), and opens StarRailSim's
   Import page. Turn on sync there.
3. Start the game and click to start. The game only hands out its session key at login, so if you were
   already in game, log out and back in.

The tray icon says what it is doing: grey while it waits for the game, teal once your account is synced, amber
when it needs you (log back in, or something stopped it). Left-click opens the site; right-click has
**Start with Windows**, **Uninstall** and **Quit**.

Logs: `%LOCALAPPDATA%\StarRailSim Sync\sync.log`.

## Options

| Argument | Effect |
| --- | --- |
| `--site <url>` | Page to open (default `https://starrailsim.com/import`) |
| `--allow-origin <origin>` | Also accept this page origin on the socket (repeatable) |
| `--portable` | Run from where it is: no install, no sign-in task |
| `--replay <export.json>` | Serve a saved Reliquary Archiver export instead of capturing; needs no admin |

## Building

```
cargo build --release
```

The build downloads the game's resource tables (through Reliquary Archiver's build script), so it needs network
access.

**Every game patch** changes the game's protocol. Bump the `reliquary` and `reliquary-archiver` tags in
`Cargo.toml` together, to the versions Reliquary Archiver's own release for that patch uses, rebuild and
publish. Until then a new client's traffic cannot be decoded.

## Credits

The packet decoding and the inventory export are [Reliquary](https://github.com/IceDynamix/reliquary) and
[Reliquary Archiver](https://github.com/IceDynamix/reliquary-archiver) by IceDynamix and contributors (MIT).
The capture loop in `src/capture.rs` is adapted from Reliquary Archiver.
