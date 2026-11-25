# BitCraft Tracker

Cross-platform (macOS, Windows, Linux) tray app showing the status of a
BitCraft character, built with Rust and [GPUI](https://gpui.rs). Data comes
from [BitJita](https://bitjita.com). Behavior, status rules, data sources and
design decisions are documented in [`AGENTS.md`](AGENTS.md).

## Run

```sh
cargo run --release            # tray icon; click it for the panel
cargo run --release -- --open  # also open the panel at launch
BITCRAFT_DEBUG=1 cargo run     # log status/label changes to stderr
```

Settings (tracked character) are stored in
`<config dir>/bitcraft-tracker/settings.json`
(macOS `~/Library/Application Support`, Linux `~/.config`, Windows `%APPDATA%`).

### Platform requirements

- **macOS**: nothing extra. Shaders are compiled at runtime
  (`runtime_shaders`), so Xcode's Metal toolchain isn't needed to build.
- **Windows**: MSVC toolchain (Visual Studio Build Tools).
- **Linux**: GPUI's dependencies (Vulkan, xkbcommon, wayland/X11 libs,
  fontconfig), e.g. on Debian/Ubuntu:
  `sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libfontconfig-dev libssl-dev`.
  The tray uses the D-Bus StatusNotifierItem protocol (`ksni`, no GTK): works
  on KDE, XFCE, etc.; GNOME needs the *AppIndicator and KStatusNotifierItem*
  extension. Without a tray the panel opens as a window instead.

## Releases

Bump `version` in `Cargo.toml`, commit, then tag and push:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` builds Windows (`.exe`), macOS (universal
`.app`) and Linux (binary) and publishes a GitHub release. Running apps
show *Update available* in the popup (checked at startup and every 12h).
Builds aren't code-signed: Windows SmartScreen and macOS Gatekeeper warn
on first launch (macOS: *System Settings → Privacy & Security → Open
Anyway*).

## Layout

```
src/main.rs        GPUI app, tray click → popup placement, keepalive window
src/ui.rs          AppState (model + tray + popup) and the Panel view
src/model.rs       Port of AppModel: routes events, loading/offline, notifications
src/core/          No GPUI/I/O: status monitor, trackers + rules, skills, types
src/net/           Tokio thread: live websocket, REST poll, SpacetimeDB relay
src/tray.rs        Tray icon (ring + stamina pie drawn with tiny-skia)
src/notifier.rs    Desktop notifications (notify-rust)
src/settings.rs    settings.json
```

Data flows one way: the network thread sends `net::Outgoing` messages over a
channel; a GPUI foreground task feeds them into `Model`, which produces
commands (subscribe, reconnect, track character) and notifications that
`AppState::flush` forwards.

## Platform notes

- **Menu bar text**: macOS shows the label next to the icon (plain text, one
  color). Windows/Linux trays only show icons, so the label is the tooltip.
- **Popup**: a borderless GPUI window placed under the tray icon (above it
  when the tray is at the bottom of the screen, e.g. the Windows taskbar);
  it closes when it loses focus. On Wayland, windows can't position
  themselves, so the compositor decides where it appears.
- **Reconnect on wake / network change**: not implemented; the 15s ping
  detects dead connections within ~30s instead.
- **Keepalive window**: GPUI quits on Linux/Windows when the last window
  closes, so an invisible 1×1 window stays open.
- **Popup height**: the window is resized to the panel's measured height
  after layout (GPUI has no "size to content" or move API; `platform.rs`
  sets the native frame on macOS/Windows so the anchored edge stays put).
- **macOS notifications** need an app bundle to send as. Unbundled
  (`cargo run`) they're sent as Terminal; without setting this,
  `mac-notification-sys` asks "Where is use_default?" in a dialog.
- **TLS**: rustls with the `ring` provider (installed in `main`), not aws-lc,
  so Windows builds don't need cmake/NASM.
