# Nous Relic Importer

Keeps [StarRailSim](https://starrailsim.com)'s relic inventory in step with your Honkai: Star Rail account. It
reads the game's own network traffic on your PC, so every relic, light cone and character comes in without an
export file, and changes you make in game show up on the site as they happen.

Windows and Linux (the Steam Deck included). **Download:**
[for Windows](https://github.com/SeanSOSU/nous-relic-importer/releases/latest/download/nous-relic-importer.exe) or
[for Linux and the Steam Deck](https://github.com/SeanSOSU/nous-relic-importer/releases/latest/download/nous-relic-importer),
each the newest release's file.

Your account never leaves your computer: the site connects to the app at `ws://127.0.0.1:23314/ws`, and only
StarRailSim's own pages may open that socket. The one connection the app makes itself is its update check (see
[Updates](#updates)).

## Using it on Windows

1. Run `nous-relic-importer.exe` and accept the administrator prompt. Reading packets with Windows' built-in
   packet monitor (pktmon) needs it.
2. It installs itself to `%LOCALAPPDATA%\Programs\Nous Relic Importer`, adds a Start menu shortcut and an entry
   under Settings > Apps > Installed apps, registers to start when you sign in (a scheduled task with highest
   privileges, so there is no prompt at each sign-in), then opens its window and StarRailSim's Import page, which
   connects to it. **Open StarRailSim**, in the window or the tray menu, opens that page linked again in your
   default browser; the site itself has no sync control.
3. Start the game and click to start. The game only hands out its session key at login, so if you were
   already in game, log out and back in. From then on every change in game (a new relic, an upgrade, a new
   equip) reaches the site as it happens.

**Auto import**, in Settings and the tray menu, is the one switch for the site: off, open pages keep what
they have and say the app is paused; back on, they start over from the account as it is now. The account is still
read and saved while it is off.

The window says what it is doing and what to do next; closing it keeps sync running in the notification area.
**Settings** (the window's gear, or the tray menu) turns the same window to the switches, updates, language and
**Uninstall**; the arrow (or Esc) turns it back. **Quit**, in the tray menu, stops the app until the next sign-in.
The tray icon is grey while it waits for the game, teal once your account is synced, and amber when it needs
you (log back in, or something stopped it). Left-click it, or use the Start menu shortcut, to bring the window
back; neither asks for administrator access again.

It updates itself (see [Updates](#updates)). Running a newer download by hand works too: it asks the running copy
to quit and installs over it. Uninstall from Settings > Apps > Installed apps, or with **Uninstall** in the app's
Settings: both remove the program, the sign-in task, the shortcut and the logs.

Logs: `%LOCALAPPDATA%\Nous Relic Importer\sync.log`.

## Using it on Linux

One file, `nous-relic-importer`, a static x86_64 build that runs on any distribution with nothing else installed.

1. Mark it executable (file manager: Properties > Permissions, "Allow executing as program") and open it.
2. It installs itself to `~/.local/share/nous-relic-importer`, adds itself to the app menu and to the desktop's
   login autostart, then asks for your password once, through the desktop's own prompt (polkit), to give the
   installed copy one capability: CAP_NET_RAW, for reading packets. It opens StarRailSim's Import page, which
   connects to it.
3. Start the game and click to start, as on Windows. **Auto import** works the same way too.

The capability stays on the file, so later starts ask nothing; the app opens its packet socket first thing,
then drops the capability. Its tray icon (KDE and the Steam Deck's desktop mode, or GNOME with the
AppIndicator extension Ubuntu ships) works like the Windows one. Its window is the same page, opened in your
browser from the app itself at `http://127.0.0.1:23314/` with a token only the app hands out; starting the app
again from the app menu opens it, tray or not. An update replaces the file, so installing one asks for the password
again. Uninstall from the app menu entry's **Uninstall** action, or with **Uninstall** in the app's Settings.

On a Steam Deck the `deck` user has no password until you set one (`passwd` in Konsole), and the app runs in
desktop mode only. Logs and the saved account: `~/.local/state/nous-relic-importer`.

## Updates

Every game patch changes the game's protocol, so every patch needs a new version of the app. A few seconds after it
starts (a minute and a half when it starts at sign-in), and every six hours after that, the app reads the release
feed: `latest.json` on the latest GitHub release. It sends nothing but the request. A newer version is downloaded
and checked against the release key built into the app (`update-key.pub`, a minisign key). Its signature must also
name that exact version and platform, so a tampered file, an older genuine build or another platform's build is
refused. The verified file waits beside the installed copy, then takes over the way a hand-run update does: it asks
the running copy to quit, copies itself into place and starts. Start-at-sign-in stays as the reader left it.

- **Windows** installs it by itself once the game has sent nothing for five minutes, so an update never cuts into a
  game session. The window offers **Install now**, and Settings has an **Install updates automatically** switch;
  turned off, the update waits for **Install**.
- **Linux** downloads it by itself and waits for **Install** in the window or the tray, since the new file needs
  the password once more for its capture permission.
- A copy run with `--portable` is never replaced: the window points at the new download instead.

A version that downloaded but could not replace the installed copy is left alone for a day. The tray's **Check for
updates** and the window's **Check now** read the feed at once. The window shows each step.

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
| `--update-feed <url>` | Read updates from this feed instead of the latest GitHub release's (HTTPS, or HTTP to this machine) |
| `--update` | What the updater runs a downloaded version with: replace the installed copy and start it |

## Testing without a live account

Start the PearlSR emulator, then run `nous-relic-importer.exe --emulator` before its game client logs in. PearlSR speaks the live game's protocol (same
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
access. It downloads them from [TurnBasedGameData](https://gitlab.com/Dimbreath/turnbasedgamedata) at the commit
`core/gamedata.rev` names (from GitLab, or from its GitHub mirror when GitLab refuses: it rate-limits unauthenticated
downloads), so the same source always builds the same tables; changing that file is what makes a build download
them again.

Linux, in Docker (the image is the build environment and the test bench), from the repo:

```
docker build -t nous-relic-importer-linux -f tools/linux.Dockerfile tools
docker run --rm --cpus 2 -v "$PWD:/src:ro" -v nri-cargo:/usr/local/cargo/registry -v nri-git:/usr/local/cargo/git \
  -v nri-target:/target -e CARGO_TARGET_DIR=/target -w /src nous-relic-importer-linux \
  cargo build --locked --release --target x86_64-unknown-linux-musl
```

`tools/linux-e2e.sh <binary> decoder/tests/emulator-login.pcap`, run as root in the same image, tests the
whole Linux path: a normal user's first run installs and is granted the capability (through a stand-in
`pkexec`), the recorded emulator login is replayed onto loopback, and the site's socket and the window page
must both have the account; then it quits and uninstalls. The packet socket's own test needs CAP_NET_RAW:
`cargo test -- --ignored` as root there.

**Every game patch** changes the game's protocol, and a new client's traffic cannot be decoded until the app moves
to it. Once [Reliquary](https://github.com/IceDynamix/reliquary) has tagged the new protocol:

1. `.\tools\update-reliquary.ps1` sets Reliquary's newest tag (or `-Reliquary <tag>`) on every `reliquary` entry:
   `Cargo.toml`, `core/Cargo.toml` (dependency and build-dependency), `decoder/Cargo.toml` and `live/Cargo.toml`,
   with Reliquary's own `protobuf` version on the tests' one. It copies Reliquary Archiver's exporter into
   `core/src/export` again only when the archiver has released a different one since `core/src/export/VENDORED`'s
   tag; most patches need none.
2. If the script warns that the archiver's `build.rs` changed, mirror its downloads in `core/build.rs`.
3. The tests that replay `decoder/tests/emulator-login.pcap` fail on a new protocol, saying it needs a new emulator
   recording: record a PearlSR login on the new protocol (`--emulator --record`), put it in that file's place and set
   the counts its tests expect.
4. Put TurnBasedGameData's newest commit in `core/gamedata.rev` (its id, then its title), so the new patch's
   characters, light cones and relic sets are in the tables: the exporter leaves out any item they do not name.
5. Raise `version` in `Cargo.toml`, `cargo test --workspace`, the release builds, and publish them (below).
   Installed copies pick the new version up within six hours.

`tools/patch-check.ps1` prepares this by itself. When Reliquary has a tag newer than the one `main` builds on, it
makes a local branch, `reliquary-<tag>`, in a worktree beside the repo (`<repo>-patch`): steps 1 and 4, the next patch
version, then `cargo test --workspace` and the Windows `cargo build --release` there, committed whatever they say.
When TurnBasedGameData has moved past `main`'s `core/gamedata.rev` with no new Reliquary tag, or since that tag's branch
was made (the game's data can land before Reliquary's tag, and a hotfix can add items without one), it does the same
with step 4 alone on a branch `gamedata-<commit>`. It
reports with a Windows notification and `target/patch-check/<branch>.md`, and never pushes or publishes.
`-DryRun` says what it would do. `-Register -Jobs 2 -Affinity FFFF0` makes it a weekly scheduled task for the
signed-in user (Thursdays at 10:00, or as soon as it can after a missed one) whose cargo runs two jobs on the CPUs in
that hex mask; `Start-ScheduledTask "Nous Relic Importer patch check"` runs it now. Each run adds a line to
`target/patch-check/patch-check.log`.

### Publishing a release

Every release is signed with the release key, whose public half is `update-key.pub`. Keep the secret half out of the
repo (`*.key` is ignored) and back it up. If it is lost, installed copies can only move to a build with a new key by
hand. If it leaks, anyone who can serve the feed can push an update. `nri-release keygen <secret key file>
update-key.pub` makes a new one, protected by a password if you add `--password`.

1. Raise `version` in `Cargo.toml` and build both platforms (above).
2. Sign the builds and write the feed into `dist/`:

   ```
   cargo run -p nri-release -- sign --key <secret key file> --out dist \
     --base-url https://github.com/SeanSOSU/nous-relic-importer/releases/download/v0.2.0/ \
     windows-x86_64=target/release/nous-relic-importer.exe linux-x86_64=<the Linux build>
   ```

   It refuses a build of any version other than the one in `Cargo.toml` (each build carries its version in a marker),
   and a key other than the one `update-key.pub` holds.
3. Build the Android app, signed with its release key (`android-release.jks`, whose properties file holds its
   password; keep both out of the repo and back them up: Android installs an update only over the same key):

   ```
   cd android
   ./gradlew assembleRelease --max-workers=2 -PnriSigning=<keys folder>/android-release.properties
   cp app/build/outputs/apk/release/app-release.apk ../dist/nous-relic-importer.apk
   ```

   `apksigner verify --print-certs dist/nous-relic-importer.apk` should name `CN=Nous Relic Importer`.
4. Publish `dist/` as that release: `gh release create v0.2.0 dist/* --title "Nous Relic Importer 0.2.0"`. The installed
   copies read `releases/latest/download/latest.json`, so the newest release must not be a draft or prerelease.

`cargo run -p nri-release -- verify <download>`, with the download's `.minisig` beside it, checks a download by hand
and says what it is signed as (`minisign -Vm <download> -p update-key.pub` does too).

### Testing updates

`cargo test` covers the updater against feeds it serves itself: a newer signed build is staged, and anything else is
refused: a tampered file, an older genuine build passed off as newer, another key's signature, and a version that
already failed to install. End to end, build two versions that trust a test key (`NRI_UPDATE_KEY=<its base64 line>`
at build time). Sign the newer one with `nri-release sign --public <test public key> --version <its version>
--base-url http://127.0.0.1:8765/`, then:

- **Windows:** `tools\windows-update-e2e.ps1 -Old <older build> -Feed <signed folder>` installs the older one under a
  scratch `LOCALAPPDATA`. It checks the hand-over happens by itself with no game running, and with `-Phase active`
  that it waits out a game session (`--emulator` traffic) and five quiet minutes first. It needs no admin and
  touches no real install.
- **Linux:** `tools/linux-update-e2e.sh <older build> <signed folder>`, as root in the Docker image, installs the
  older build as a normal user, switches starting at login off, and installs the update from the window. It checks
  the new file was granted the capability and that starting at login stayed off.

`core/src/export` is vendored rather than used as a dependency because the archiver's build script links its
own icon and version details into anything that depends on it.

## Android (in development)

`android/` is the importer for a phone, one app with nothing else to install. Start turns on the app's own VPN
for the game only (Android asks once): Android routes the game's traffic to the app, a user-space TCP/IP stack
([ipstack](https://github.com/narrowlink/ipstack), Apache-2.0) ends each of the game's connections, and the app
opens the same connection to the same server and passes the bytes on unchanged (`android/engine/src/relay.rs`).
The game's own packets are copied to the decoder on the way; DNS goes to the phone's own servers. Everything
after that is the desktop app's `live/` crate: the same decoder, the same saved account, and the same page socket
on `127.0.0.1:23314` for StarRailSim open in the phone's browser. The account can also leave through the share
sheet as the archiver's export, which the site imports. The engine is `android/engine` (Rust, four JNI calls);
the app around it is plain Java with no libraries.

Building needs the Android SDK with NDK r28c, JDK 17 or newer, `cargo install cargo-ndk` and the targets
`aarch64-linux-android` and `x86_64-linux-android`. Point `android/local.properties` at the SDK
(`sdk.dir=...`), then:

```
cd android
./gradlew assembleDebug --max-workers=2
```

The Gradle build runs cargo-ndk for arm64 phones and the x86_64 emulator first (`-PcargoJobs=` sets its jobs).
The app's version is the one in the root `Cargo.toml`. `cargo test -p nri-android` covers the relay (a datagram
from a stand-in VPN interface to a real server and back) without a phone.

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
`core/src/export` is copied from Reliquary Archiver unchanged, and `live/src/decoder.rs` and `core/build.rs` are
adapted from it.
