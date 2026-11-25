# BitCraft Tracker

> **Keep this file up to date.** Any agent (or human) changing behavior, data
> sources, status rules, UI layout, or project settings must update the
> relevant section here in the same change. If a decision below is reversed,
> edit it rather than appending a contradiction. Record the *why*, not just the
> *what*.

## What it is

A tray / menu bar app (macOS, Windows, Linux) that shows, at a glance, the
status of the user's character in the game **BitCraft**. Written in Rust with
[GPUI](https://gpui.rs) (Zed's UI framework). **All game data comes from
one place**, the community read-only SpacetimeDB relay
[relay.bitcraftsync.app](https://relay.bitcraftsync.app): its websocket
mirrors for everything about the character, its HTTP API for player
search and the region list (user request: "one source of truth, a half
functioning app is unpredictable"; see Data sources). There is no main window or Dock
icon; everything lives in the tray icon and its popup panel.

The goal is a quick "is my character still doing something useful?" check
while the user is AFK-crafting/gathering, without tabbing into the game.

**History**: this started as a native macOS SwiftUI app (`MenuBarExtra`); it
was ported to Rust/GPUI to run on Windows and Linux too, and the Swift app
was then removed. Behavior was kept identical; decisions below that mention
"user preference" came from that app's development.

## What it shows

**Tray icon** (`src/tray.rs`, drawn with tiny-skia, 36px for HiDPI):

1. **Status indicator**: a thin ring in the overall status color, around a
   **stamina pie** in the same color (filled slice = current/max stamina,
   faint tint = empty part, solid dot when stamina is unknown). Pie starts at
   12 o'clock and fills **counter-clockwise** (user preference; clockwise
   "felt wrong").

**Label text** (`Model::label_text`), e.g.
`Leatherworking 22% — T2 grown; T1 needs water · Resting`. On **macOS** it's
shown next to the icon (`set_title`, plain text, one color). Windows/Linux
trays can't show text, so it's the **tooltip** there.

1. **Last skill** that received experience.
2. **Craft progress** of the actively worked-on craft, as **percent done**,
   directly after the skill name (no icon, no separator; user preference).
   Hidden when no craft progressed in the last 30s.
3. **Plant summary**, after an em dash, parts joined with `; `:
   `T1, T2 grown` (tiers where *every* plant is grown),
   `T1 needs fertilizer` / `T2 needs water` (tiers where *any* plant needs it).
   Hidden when there's nothing to report. Informational text, not a status
   report (doesn't change the indicator color).
4. **Active issues**: messages of all non-healthy status reports, worst
   first, separated by `·`.

**Offline**: when the character is signed out of BitCraft, a `Player`
report goes **alert / "Player offline"** (red ring; user asked for red, and
for "Player offline" rather than just "Offline"). The label then shows the
**plant summary + the issues** (`T2 needs fertilizer · Player offline`): no
skill or craft (stale while offline), but plants keep growing, so the user
asked to keep their status (an earlier request had hidden it). Going offline
is an error-state notification (the report is `healthy / Online` while
online, so it's a transition). Activity, Satiation and Food reports are
**cleared** while offline (idling offline isn't a problem, and clearing
means coming back online is a "first report", so it never notifies).

Detection (`Model::evaluate`): the relay's `player_state.signed_in` (the
game's own value; changes instantly). Unknown (relay has no
`player_state` row yet) counts as online. BitJita's `signedIn` used to be
a fallback; removed with the other fallbacks.

**Popup panel** (`src/ui.rs`, `Panel`), opened by clicking the tray icon:
header (status dot, connection state of the relay),
per-source status reports, stats rows (stamina bar `current / max`,
**colored by stamina** (`status::stamina_rgb`, user request): green above
30%, blending to orange at 20% and to red at 5% and below; **food bar** right below it; **satiation bar**
`current / max`, green, red at/below the alert threshold; total XP; last
skill; last gain), plants, passive crafts,
crafts (progress bar, percent, Paused/Done), an **Update available**
row when GitHub has a newer release (see Releases), character picker,
*Show/Hide info window*, Quit.

- **Plants section**: a **growth tracker listing only tiers with plants
  currently growing** (count + time until the whole tier is grown, `~` when
  estimated), then separate rows for *Needs fertilizer*, *Needs water* and
  *Ready to harvest* with per-tier counts (user asked for growing-only in the
  tracker and the needs shown separately).
- **Gathering section** (user request; above Crafts, only while
  gathering): the resource last hit (`Model::active_gather`) like a craft
  row: name, value, green bar (`ui::GATHER_RGB`), and "skill · tier ·
  health / max" below. **Forestry, foraging, mining** (and anything else
  extracted): the resource's **health left** in percent (rounded up).
  **Fishing**: the spot's **in-game timer** instead (user's choice; e.g.
  a baited lake school turns dormant after 400–500s, a frenzied ocean
  school after 900s), as time left (`duration_string`: minutes, seconds
  only under a minute), bar = time left / the timer's catalog duration;
  health if the spot has no timer. Shown while the resource was hit in
  the last 30s (`GATHER_WINDOW`, so it doesn't blink between hits; an
  earlier per-action bar was removed for that) and more recently than
  the last craft progressed. Disappears when the resource is used up
  (its rows are deleted). Not in the tray label.
- **Passive crafts section**: grouped by building + item, with running count,
  time until the whole group is done, and how many are ready to collect.
  Running groups first. Not in the label (user didn't ask for it).
- **Food bar** (below stamina, user request): green bar of the food regen
  buff's time left (remaining / buff duration) with the time (`41m`), and a
  short summary of the active food buffs on the line below (`Regen 3 ·
  Deluxe Action Speed 2`; `FoodBuff::short_name` turns "Level 3 Food Regen"
  into "Regen 3"). Food *names* were shown first but dropped (user: too
  long; several foods give the same buffs, so it was often "A / B"). Orange
  under 5 minutes left (`status::food_rgb`, user request); full red bar +
  red "None" when no food regen is active (full so the red shows); "–" until the relay
  delivered buffs.
- **Badges** (`core::badges`, user request; under each section title in
  the popup and in the info window): one pill per *type + tier* and state,
  with a count, e.g. `Grain T1 ×6`, `Ingot T2`, `Rope T5`. Colors: **red**
  = action needed (plant needs fertilizer/water), **orange** = processing
  (growing, passive craft running, craft worked on in the last 30s),
  **gray** = idle (paused craft, queued passive craft), **green** =
  finished (grown, ready to collect, craft complete). Most urgent first.
  Crafts and passive crafts use the crafted item's **tag** + tier (relay
  `item_desc` columns 10 / 9), falling back to the item name. Plants use the crop from the placeable name
  (`<stage> <quality> <crop>`; quality = tier): Embergrain → Grain,
  Starbulb → Bulbs, Wispweave → Fiber (the only three crops in the
  catalog; the user's examples were "Grain" and "Bulbs").
  **Hover text** (`Badge::tooltip`, user request) by state: gray → percent
  done (`8% done`; queued passive crafts: `Queued`), red → what's needed
  (`Needs fertilizer`, `3 need fertilizer · 1 needs water`), orange → time
  left (`4m left`, `next 4m · all 14m`, `~` = estimate, only when the relay's
  growth timer row is missing), green → `Ready to
  harvest` / `Ready to collect` / `Done`. Active crafts have no time left
  (the rate-based ETA was removed), so orange crafts show percent too.
  **Pulse** (user request): a plant / passive craft badge pulses
  (opacity animation) when something in it finishes within 30s
  (`Badge::ends_soon`, `ui::PULSE_WITHIN`). Only orange badges with a known
  end time; GPUI redraws every frame while one pulses.
  **Tier rows** (`badges::by_tier`, user request): when a section has
  more than one tier, its badges are split into one row per tier with a
  `T2` heading (ascending; untiered last) and the badges drop the tier
  (`Grain ×6`). With a single tier it's one row with `Grain T1 ×6`. In
  the info window the tier headings sit (left-aligned, flush with the label) in the label column under
  "Plants"/"Passive"/"Crafts" (user request, so all badges line up); in
  the popup they're a narrow column left of the badges, under the section
  title.
- **Timers** (plants, passive crafts, food, last gain; `core::types::duration_string`):
  `1h 05m`, `4m` (minutes rounded up, so never `0m` with time left), and
  seconds only under a minute, `42s` (user preference; was `4m 12s`).
- Light/dark colors follow the window appearance (`Theme`).

**Info window** (`src/info.rs`, user request): a small (260px) always-on-top
card, toggled from the popup, for keeping an eye on the character while
playing. Top-right corner of the main display by default; **movable** by
dragging (anywhere on macOS/Linux, the header on Windows). Position and
open state are saved in `settings.json` (`info_origin`, `info_window`) and
restored at launch; a saved position on a display that's gone falls back to
the default. Never takes focus. Content:

- header: overall status dot, character name, ✕ (closes it);
- status line: the worst issue, or the Activity message when all is well,
  with the last skill; the current craft (active, else the last worked on)
  with percent and bar, or, while gathering and more recent than the
  craft, the gather target (same rule as the popup's Gathering section):
  name + health %, and a 16px green `labeled_bar` with `health / max`
  inside (user request, like stamina/health/food; a fishing timer shows
  its time left inside instead); hidden offline;
- bars (16px, value centered **inside** the bar, user request;
  `info::labeled_bar`): stamina (`current / max`, popup colors),
  **health** (relay `health_state`, max = stats value 0; pink, orange at
  ≤ 50%, red at ≤ 15%, `status::health_rgb`), food (time left, popup colors; **hover** lists the
  active food buffs, one per line with time left, e.g. `Regen 3 · 41m`;
  user request, instead of a summary line under the bar); hidden
  while offline (user request; stale, and the status line says "Player
  offline");
- one row each for plants, passive crafts and crafts: **badges** (see
  below), with "next grown in 5m" / "next done in 4m" underneath.

Same data and timer format as the popup; it re-renders on every model
change and the 1s tick, and is sized to its content like the popup.
Staying on top: macOS uses GPUI's `PopUp` kind (popup window level, all
Spaces, next to fullscreen apps; `Floating` is normal level there).
Windows: `HWND_TOPMOST` + `WS_EX_NOACTIVATE` (`platform::make_overlay`).
Dragging: macOS `performWindowDragWithEvent` from the mouse-down (GPUI's
`start_window_move` is a no-op there), Linux `start_window_move`, Windows
`WindowControlArea::Drag`. Linux/Windows behavior is untested (on X11 a
PopUp is a "notification" window; the WM may not let it move).

## Status rules

Colors: green = healthy, orange = warning, red = alert. Overall = worst report.
**Red is reserved for real problems** (no food, low satiation, offline,
disconnected); being idle is orange (user request). Idle still notifies (see
Notifications).
Implemented in `src/core/trackers.rs` (`ActivityCheck::evaluate`,
`SatiationTracker::report_current`) and `src/model.rs` (`Model::evaluate`:
Player, Connection).

| Source | Condition | Status | Message |
|---|---|---|---|
| Activity | character moved in the last 10s (relay position changed), unless counting XP gained after the last move | healthy | Active |
| Activity | last action was a craft and it completed | warning | Craft finished |
| Activity | counting XP | healthy | Gaining experience |
| Activity | stamina "ready" (API max known) | warning | Idle: stamina full / Idle: stamina above N% (or Craft idle (N%) if the last action was an unfinished craft) |
| Activity | stamina regenerating | warning | Resting |
| Activity | last action was an unfinished craft, no XP, no regen | warning | Craft idle (N%) |
| Activity | no XP, no regen in 10s | warning | Idle: no XP, no regen |
| Activity | nothing received yet | warning | No XP or regen yet |
| Satiation | satiation ≤ 15 | alert | Low satiation (N) |
| Food | food regen buff active / none | healthy / alert | Nm Ss left / No food |
| Player | signed out / signed in | alert / healthy | Player offline / Online |
| Connection | relay: every region live / connecting (incl. "still loading") / a region disconnected / every region refused | healthy / warning / alert / alert | Connected / Connecting… / Disconnected / Relay unavailable |

Activity rows are checked top to bottom.

- **Active** (user request): moving around means the player is at the
  keyboard, so it overrides every other Activity rule. Movement = the
  `mobile_entity_state` location changed (`Model::update_position` →
  `moved_at`, window `ActivityCheck::move_window` = 10s). Source: the
  relay (BitJita's live channel was a second source; removed with the
  fallbacks). **Idle right after Active is quiet** (user request: the
  player stopped the action by walking away, not worth an alert): when
  Activity goes from "Active" to an idle message, neither the went-idle
  notification nor the idle sound fires, until Activity is something other
  than idle again (`Model::idle_after_moving`).

- **Counting XP** = XP gained in the last 20s *and* stamina hasn't
  regenerated since: XP while regenerating is improbable (user's rule), so
  once regen starts, earlier XP no longer means "active". "Regenerated" =
  **2+ stamina increases after the XP without spending in between**
  (`ActivityCheck::rest_ticks`, `StaminaTracker::regen_ticks_since`). A
  single tick isn't enough: stamina ticks up between craft actions
  (passive / food regen), which showed "Craft idle" while crafting (full
  stamina topped back up, or the next action's spend arriving before its
  XP). Costs ~1s before "Resting" shows. The 20s window (was 5s) bridges gaps
  between XP ticks; regeneration overrides it. It's checked **before**
  Resting (user request: switch to "Gaining experience" as soon as XP
  arrives, not when the stamina spend follows), and XP after the last move
  overrides "Active" too.
- **Regenerating** = stamina went up in the last 10s and hasn't gone down since
  (`StaminaTracker::is_regenerating`); spending stamina ends it immediately,
  so resuming work flips straight back to green.
- **"Ready"** depends on the last skill that gained XP
  (`ActivityCheck::ready_fraction`): full stamina by default, **20% for
  Fishing**, because fishing rarely needs full stamina to continue. Add other
  skills there if they need a different threshold. Use `Skill` variants, not
  raw ids.
- **Craft rules** (user request: yellow when a craft just finished, red when
  the last craft still needs progress but you're idle). "Last action was a
  craft" = `CraftTracker::last_worked` (last craft with live progress, kept
  after completion in case its row disappears) progressed no more than
  5s before the last XP gain. XP later than that means you moved on (e.g. a
  craft that isn't tracked), and the normal rules apply. "Craft
  finished" stays until then; resting during an unfinished craft is still
  "Resting".
- There is **no low-stamina warning** (removed): you run out of stamina
  exactly when you stop to regenerate, so it only duplicated "Resting".
- **Food** (user request, re-added once reliable data existed): reported
  only once the relay delivered buffs and the food buff types; cleared
  while offline.
  Only the "Food Regen" buff counts (every food gives one); extra "Food
  Buffs" alone (e.g. action speed) don't. "No food" is a normal alert, so it
  notifies when food runs out (5 min cooldown).
- **Connection** is the relay's state (the only data source; an outage
  means stale data). Worst region wins (`net::overall_state`), ignoring
  regions that refuse for good; transient refusals ("still loading")
  count as connecting.
- Messages are kept short because they're shown in the label.

## Notifications

Desktop notifications via `notify-rust` (`src/notifier.rs`; macOS
Notification Center, Windows toasts, freedesktop on Linux), sent from a
thread. Produced by `Model` (`collect_notifications`,
`collect_alert_notifications`), never while loading. Sent for:

| Event | Rule |
|---|---|
| Error state | A status source changes **to alert from a known non-alert status**. A source's *first* report doesn't count. **5 min cooldown per source** (e.g. flapping connection). Alert → alert (message change) is silent. |
| Went idle | Not when it comes straight from "Active" (moving; see Status rules). Activity changes to an idle message (`ActivityCheck::is_idle`: "Idle…", "Craft idle…") from a known non-idle one (e.g. Gaining experience / Resting → Idle: stamina full). Idle → idle (message change) is silent. Same 5 min cooldown. Idle used to be red and notified as an error state; it became orange, and this keeps the notification. |
| Craft finished | Once per craft, when its relay progress reaches the total. |
| Passive craft finished | When a passive craft's computed end time passes (1s tick) or it's reported complete, once per craft. **Batched**: everything finishing in the same check is one notification. Crafts already finished in the first list are silent. |
| Plants grown | Per tier, between two updates: growing count drops to 0 **and** grown count increased. Not on the first list. |

Connection reports `healthy` when connected (instead of clearing) so a later
disconnect is a transition and notifies.

**macOS**: notifications must be sent "as" an app bundle.
`notifier::init()` sets it at startup: our bundle id when packaged as a
`.app`, else Terminal's (`cargo run` has none). Unset,
`mac-notification-sys` looks up an app named "use_default" via AppleScript,
which pops up a "Where is use_default?" dialog that also steals focus (and
closes the popup).

## Sounds

Settings section in the popup (*Settings* button next to Quit; user
request): play a sound when an active **craft finishes**, when the
character **runs out of food** (Food report turns red, same 5 min
cooldown as the notification), and when **idle with stamina above 90%**
(Activity idle message + stamina > `model::IDLE_SOUND_STAMINA`; once each
time it becomes true). Each is a checkbox (all off by default), plus a
**volume slider** (20 segments of 5%, click or drag; default 50%) and a
*Test* button. Stored in `settings.json` (`sounds`).

Sounds are synthesized chimes (`src/sound.rs`, sine notes with decay; no
sound files): rising two notes = craft finished, falling twice = out of
food, three pulses = idle. Played with **rodio** (no default features, so
no decoders) on a short-lived thread. The model emits `Sound` events
(`Model::take_sounds`, never while loading); `AppState::flush` plays the
enabled ones.

## Architecture

```
src/main.rs        GPUI app; tray click → popup placement; keepalive window
src/ui.rs          AppState (model + tray + popup + search) and the Panel view
src/info.rs        Floating info window (toggle, drag, saved position)
src/model.rs       Routes network events into trackers; loading/offline;
                   status evaluation; notifications; label text
src/platform.rs    Native bits GPUI lacks: tray DPI scale, hide Dock icon,
                   set window frame, global mouse state, no-activate /
                   topmost, window drag (macOS AppKit / Win32)
src/core/          No GPUI, no I/O (unit-tested):
  status.rs        HealthStatus (healthy < warning < alert) + StatusMonitor
  trackers.rs      Experience/Stamina/Satiation/Craft/Plant/PassiveCraft
                   trackers + ActivityCheck
  skill.rs         `enum Skill` (value = skill id) + display names
  types.rs         Data passed from net → model (CraftInfo, PassiveCraft,
                   Plant, PlayerSummary); times are Unix seconds (f64)
  version.rs       Release tag vs current version (update check)
src/net/           Background Tokio runtime ("network" thread):
  mod.rs           Command/Outgoing channels; per-character tracking
                   (relay hub over every mirrored region)
  api.rs           HTTP: relay player search + /health (regions),
                   GitHub update check
  relay.rs         SpacetimeDB relay, one feed per region: XP, stamina,
                   satiation, crafts, passive crafts, plants, food,
                   stats, health, position, online, username
src/tray.rs        Tray icon rendering/updates
src/notifier.rs    Desktop notifications
src/sound.rs       Alert sounds (synthesized chimes, rodio)
src/settings.rs    settings.json (tracked character, info window, sounds)
```

**Data flow is one way**: the network thread sends `net::Outgoing` messages
(per-character events tagged with the user id, search results) over an
`async_channel`; a GPUI foreground task feeds them into `Model::handle`. The
model produces `net::Command`s (track character, search) and notifications, which `AppState::flush` forwards, then updates the
tray. A 1s tick (`Model::tick`) re-evaluates time-based rules. Events for a
character that's no longer tracked are dropped.

### Key design decisions

- **Status is decoupled from data.** `StatusMonitor` knows nothing about XP or
  stamina. Any source calls `monitor.report(source, status, message)`; the
  indicator shows the worst. Trackers collect data; rules that combine sources
  live in their own check (`ActivityCheck`). Add new warnings by adding a
  report source, not by touching existing ones.
- **Core logic has no GPUI/I/O** (`src/core`), so rules are unit-testable
  (`cargo test`). Add a test when changing a status rule.
- **No gray text in the menu bar** (macOS user feedback: hard to read). The
  tray title is plain text anyway. Loading uses the warning color (orange
  ring, empty pie) and the text "Loading…".
- **Reconnect** (per region feed, `relay::run`) with exponential backoff
  1s → 30s. **Keepalive**: ping every 15s; a ping still unanswered at the
  next tick (> 10s) fails the connection (silent dead sockets after sleep
  / Wi-Fi switch). No explicit reconnect-on-wake or network-change
  handling (the Swift app had both); the ping detects it within ~30s.
  (A BitJita-specific "stale feed" reconnect existed for its websocket,
  which could stop delivering on a healthy connection; gone with it.)
- **Loading state** (`Model::is_loading`): at launch and after switching
  character, until every relay region delivered its rows
  (`NetEvent::RelayLoaded`), capped at 20s. Tray shows the
  orange ring + "Loading…", the panel "Loading <name>…" instead of the data
  sections, and no notifications fire. Typically 1–2s.
- **HTTP polling**: only the relay's `/health`, every 5 min (15s retry
  until the first answer), for the region list. The panel shows a
  "Passive crafts — Loading…" placeholder until the first relay list
  arrives.
- **Character selection** is by username search in the panel: the
  tracked character's name with a *Change* button that opens a search field
  (typed via key events; GPUI has no text input widget), debounced 300ms,
  results clickable, Enter picks the first, Escape cancels. `settings.json`
  stores `user_id` (entity ID, what all queries use) and `username`
  (display only; refreshed from the relay's `player_username_state`). Default `user_id` is the
  original user's character.
- **Popup window**: a borderless `WindowKind::PopUp` window under the tray
  icon (above it when the tray is in the bottom half of the screen, e.g. the
  Windows taskbar). **It doesn't take focus** on macOS/Windows (user
  request: the game keeps keyboard focus): macOS PopUp windows are
  non-activating panels (opened with `focus: false`, no app activation);
  on Windows GPUI activates on show, so `WS_EX_NOACTIVATE` is set
  (`platform::set_no_activate`). Without focus there's no focus-loss
  event, so `main::watch_outside_clicks` polls the global mouse buttons
  every 25ms (`platform::pressed_mouse_location`: `NSEvent
  pressedMouseButtons` / `GetAsyncKeyState`) and closes it on a press
  outside it. Focus-loss closing is off there (AppKit briefly made the
  panel key and then resigned it, which closed it right after opening).
  *Change* (character search) takes focus for typing (`activate_window`;
  a non-activating panel becomes key without activating the app). Escape
  closes it while it has focus. Linux has no global mouse query: there it
  takes focus and closes on focus loss, as before. A press that just
  closed it (e.g. on the tray icon) doesn't reopen it with the click that
  follows (400ms).
  **Sized to content**: GPUI has no size-to-content or move API, so the panel
  is measured after layout (`on_children_prepainted` → `fit_window`) and the
  native window frame is set (`platform::set_window_frame`: AppKit `setFrame`
  on macOS, `SetWindowPos` on Windows, GPUI `resize` on Linux), keeping the
  edge next to the tray fixed (GPUI's own `resize` on macOS keeps the bottom
  edge, so the popup would slide under the menu bar). Taller than the screen
  → scrolls. On Wayland the compositor positions windows.
- **Keepalive window**: GPUI quits on Linux/Windows when the last window
  closes, so an invisible 1×1 window stays open (not needed on macOS).
- **No tray available** (e.g. GNOME without the AppIndicator extension): the
  panel opens as a window at startup instead. `--open` opens it at launch
  regardless.
- **macOS**: activation policy set to Accessory at startup (no Dock icon;
  GPUI starts as a regular app).

## Data sources

**One source, no fallbacks** (user request: "I would like one source of
truth, a half functioning app is unpredictable", then: replace every
BitJita REST call and websocket with the relay). Everything comes from
`relay.bitcraftsync.app`. There used to be BitJita fallbacks for most data
(REST crafts, passive crafts, placeables, stats, `signedIn`; live craft
progress, position, stats and buffs), rules for mixing them (newest wins,
source-switch baselines, cached stats and food types in `settings.json`),
and BitJita as the primary source for XP / stamina / satiation (live
websocket), player search and username (REST). All removed: when the
relay is down, data is stale and the **Connection** report says so.

| Data | Source |
|---|---|
| XP, stamina, satiation, crafts, passive crafts, plants, food buffs, max stamina/satiation/health, health, position, online status, username | Relay websocket (per region) |
| Player search, mirrored regions | Relay HTTP API |

### SpacetimeDB relay — `src/net/relay.rs`, hub in `src/net/mod.rs`

BitCraft runs on SpacetimeDB. We use the community read-only mirror
**https://relay.bitcraftsync.app** (user request; source
`ekscrypto/spacetimedb-bitcraft-mirror`, MIT; BitJita and other tools
share it). It
replaced BitJita's relay (`relay.bitjita.com`, "BITCONNECT"), which was
often down for long periods (`/healthz` 503) and slow to deliver food
buffs. **No account or BitCraft token** (the relay issues its own
identities). Unofficial, no published terms/limits; could change without
notice. Read-only: `OneOffQuery` / `CallReducer`
are rejected. Connecting to Clockwork's own SpacetimeDB with the player's
game token was rejected: account risk, and the relay has the same data.

- `wss://relay.bitcraftsync.app:<3000 + region>/v1/database/idc-<region>/subscribe?compression=None`,
  websocket subprotocol `v1.json.spacetimedb` (the relay recommends
  `v2.bsatn`, but v1 JSON works). **Each region is its own instance on
  port 3000 + region** (global: 3000, `idc-global`); live ones are listed
  at `https://relay.bitcraftsync.app/health`. A port with no region behind
  it answers **nginx 502** on the handshake, treated as a lasting refusal
  (`relay::session`; otherwise it would retry forever and the relay would
  never count as live). **One database per region**, and rows
  live in the region of the *thing*: placeables and crafting stations stay
  in their region, while the player's buffs move with the player. (Bug
  found: following only the player's region showed 0 plants/crafts after
  the player traveled from region 14 to 13, while buffs still came through.)
  So the **relay hub** (`relay_hub`) runs one feed per region: every
  `idc-<N>` source listed by `/health` (was 7, 8, 9, 12, 13, 14, 17, 18,
  19), re-read every 5 min and **only added** (a region missing from one
  answer keeps its feed and rows). Player and claim regions aren't looked
  up anymore: rows only exist in mirrored regions, which are all
  followed. All queries filter by the character, so each is small. Crafts, passive crafts and plants are
  merged across regions; food comes from the player's region.
- **Delivery**: lists (crafts, passive crafts, plants) and the player's
  rows are sent once **every followed region delivered its rows** at
  least once (regions refusing for good, e.g. not mirrored, retried every
  5 min, are skipped), then on every change; so a region that's still
  loading never looks like missing crafts. A region that's reconnecting
  keeps its last rows (stale until it's back; the Connection report
  shows it). `RelayState::Live` = every region live; a dropped region is
  `Disconnected` until its retry. Errors like
  "module is still loading its initial state from upstream; retry
  shortly" (relay restarting, 8+ min seen) are retried with the normal
  backoff and count as connecting.
- **Name lookups**: recipe → `item_desc` and `building_state` →
  `building_desc` need a second subscribe round. A region holds its data
  until that re-subscribe's `InitialSubscription` arrived (updates can
  come in between), so names never show half-resolved ("Forge" instead
  of "Ingot T2").
- Protocol: send `{"Subscribe":{"query_strings":[SQL…],"request_id":1}}`
  (replaces the whole set; also used to add lookups) → `InitialSubscription`
  (all current rows) → `TransactionUpdate`s (`deletes` + `inserts` per table).
  Rows are JSON strings: **objects with the columns in schema order** in
  `InitialSubscription`, **arrays** in `TransactionUpdate`s (seen on
  bitcraftsync). `relay::rows` turns both into column arrays, so the
  column indices below apply; object order needs serde_json's
  **`preserve_order`** feature (set in `Cargo.toml`). Nested product types
  inside a row stay as they are (e.g. `active_buffs` entries are arrays).
  Sums `[tag, payload]`; timestamps
  `{"__timestamp_micros_since_unix_epoch__": micros}` in objects,
  `[micros]` in arrays (`relay::date` takes both). Joins (`SELECT a.* FROM a JOIN b ON … WHERE …`)
  work and can repeat a row once per match → the row store is ref-counted.
  `LIMIT`, `LIKE` and HTTP `/sql` aren't available (hence the HTTP API
  for search). Schema (column order/types):
  `GET https://relay.bitcraftsync.app:3014/v1/database/idc-14/schema?version=9`.
- **Food buffs** (`Feed::food`): `active_buff_state` (pk entity_id) =
  `[entity_id, [[buff_id, [start], duration, values], …]]`, ~370 entries
  per player; inactive ones have start `[0]`. **start is Unix seconds**,
  duration seconds (food: 900/1800/3600). `buff_desc` is subscribed only for
  the two food buff types (`buff_type_id` 1790361536 "Food Regen", 11 rows;
  861783812 "Food Buffs", 96 rows): a match = food buff; column 2 = name.
  Expiry is computed from start + duration on each tick. The hub sends
  the started buffs (`NetEvent::RelayBuffs`) **only when they change**
  (every region update re-sends data). Which buffs are food comes from
  `buff_desc` (`NetEvent::FoodBuffDescs`); until both arrived there's no
  Food report (it would say "No food"). Not subscribed
  anymore: `food_desc` (413 rows; column 8 = `[[buff_id, duration option],
  …]` maps food items → buffs). It was used to name the food, but 369 foods
  share 223 buff sets (often ambiguous) and the names were too long. `buff_desc.online_timestamp` is true for food buffs; whether
  that pauses the timer while signed out is unverified (it doesn't matter
  while playing).
- **Player rows** (`Feed::signed_in`, `Feed::stats`): `player_state` (pk is
  **column 1**, entity_id; column 0 is the teleport location, hence
  `pk_column`) column 6 = `signed_in` (`sign_in_timestamp` resets to 0 on
  sign-out); `character_stats_state` = [entity_id, values] (1 = max
  stamina 262, 19 = max satiation 100, 0 = max health; index order as in
  BitJita's stat list, verified against its REST stats).
  Like buffs, these rows **move with the player** (present in region 13 and
  absent in 14 after traveling), so the hub takes the player's rows (food,
  stats, online status) from the region that has `player_state`
  (preferring a live one: a reconnecting old region can still hold a
  stale copy) and sends `NetEvent::RelayPlayer` (None when no region has
  it: unknown). Subscribed the same way: `health_state` = [entity_id,
  last_health_decrease_timestamp, health, died_timestamp] (info window),
  `player_username_state` = [entity_id, username] (display name).
- **XP, stamina, satiation** (player region): `experience_state` =
  [entity_id, [[skill_id, quantity], …]], `stamina_state` = [entity_id,
  last_stamina_decrease_timestamp, stamina], `satiation_state` =
  [entity_id, satiation]. Each updates ~1/s while it changes (verified
  while mining). The hub sends `NetEvent::Experience` / `Stamina` /
  `Satiation` **only when the value changes** (every region update
  re-sends the player's rows). Unlike BitJita's live channel, the first
  XP list is a **snapshot**, so `ExperienceTracker::handle` takes it as
  the baseline (no gain; skill unknown until the first gain).
- **Position** (`Feed::position`): `mobile_entity_state` = [entity_id,
  chunk_index, timestamp, location_x, location_z, destination_x,
  destination_z, dimension, is_walking, …]; location changes → "Active"
  (see Status rules). Player region, like the other player rows.
- **Gather target** (`Feed::gather`, player region): `extract_outcome_state`
  = [entity_id, target_entity_id, last_timestamp, damage, is_crit]
  (updated per hit; verified while chopping: −30 health per hit every
  ~1.3s). Joined on the target: `resource_state` = [entity_id,
  resource_id, direction], `resource_health_state` = [entity_id, health],
  `resource_growth_timer` = [scheduled_id, scheduled_at (1 = Time),
  entity_id, growth_recipe_id] (public; the in-game timer some resources
  show, `resource_desc.show_time_left`). Second lookup round by resource
  id: `resource_desc` (name 1, max_health 4, tier 13, tag 14) and
  `extraction_recipe_desc WHERE resource_id = …` (column 15
  `experience_per_progress` = [[skill_id, xp]]: the skill; fish schools →
  12 Fishing, Young Beech Tree → 2 Forestry); by growth recipe id:
  `resource_growth_recipe_desc` = [id, resource_id, time [min, max],
  grown_resource_id, …]. A used-up resource's rows disappear
  (`resource_depleted_event` isn't replicated). `player_action_state`
  isn't used (see Known limitations).
- **Active crafts** (`Feed::crafts`): `progressive_action_state` (owner
  filter) = [entity_id, building_entity_id, function_type, progress,
  recipe_id, craft_count, last_crit_outcome, owner, lock_expiration,
  preparation]; total = recipe `actions_required` (column 14) × craft_count
  (= BitJita's `totalActionsRequired`, verified 130 × 552 = 71,760); name via
  recipe → `item_desc`, building via `building_state` → `building_desc`.
  Crafts whose recipe hasn't arrived yet are skipped (total 0 would look
  finished). Progress changes arrive live and count as "last progressed".
- Tables used (column indices in `relay.rs`): `passive_craft_state`
  (entity, owner, recipe_id, building_entity_id, timestamp, status
  Queued/Processing/Complete, slot), `crafting_recipe_desc` (name,
  time_requirement = duration, crafted_item_stacks), `item_desc`,
  `building_state` → `building_desc` (names; second lookup round),
  `placeable_state` (owner, placeable_id) → `placeable_desc` (name, tag,
  tier).
- **Plant growth** (`Feed::plants`): `placeable_growth_timer` =
  [scheduled_id, scheduled_at (sum: 0 Interval, **1 Time** = growth end),
  entity_id (the plant), growth_recipe_id] is public and gives the
  **exact** end per growing plant (verified: timer → `placeable_growth_desc`
  `[id, placeable_id, time [min, max], outcomes, …]` → outcome = the
  "Grown …" placeable). `placeable_growth_desc` (by placeable_id) is also
  subscribed for the estimate fallback while a timer row is missing.
  (`growth_state.end_timestamp` was empty for crops; no longer used.)
- **Private** (subscribe error): `passive_craft_timer`, so passive end times
  are still start + recipe duration. Processing crafts run in parallel slots
  (`slot = some(n)`); a `Queued` craft is shown without an end time until it
  turns Processing. No Queued rows were seen in region 14 (51,769 rows:
  47,933 complete, 3,836 processing).
- Crop growth start isn't stored; it's recorded when a plant is seen
  switching to `Growing …` live, for the `~` estimate (start + catalog
  duration) when there's no timer row.

### Relay HTTP API — `src/net/api.rs`

`https://relay.bitcraftsync.app` (JSON, no auth; described in its
`/llms.txt` and `/openapi.json`). The relay asks clients to cache for a
minute and not poll in a loop.

| Endpoint | Used for |
|---|---|
| `GET /player?name=<text>` | Player search: case-insensitive substring on username across all regions, **no limit** (400+ for "to"). Results: `entity_id` (string), `username`, `region`, `signed_in` (online dot), `last_active_timestamp`. Re-ranked client-side (exact, prefix, rest; alphabetical within) and cut to `api::SEARCH_LIMIT` = 50; the UI hints to type more at 50. ≥ 2 chars. |
| `GET /health` | `sources` map: `global` + `idc-<N>` per mirrored region (port, connectivity). Region list for the hub. |

Not used: `/player/<id>` (same fields as a search result; the websocket
has them live), `/player/<id>/crafts`, `/claim/*`, inventories, etc.

### BitJita (no longer used)

Everything once came from BitJita: `wss://live.bitjita.com/` channels
(`experience_state`, `stamina_state` (stopped sending when full),
`satiation_state`, `character_stats_state`, `mobile_entity_state` (~16
events/s while moving), `active_buff_state`, `progressive_action_state`)
and REST `https://bitjita.com/api/` (`players?q=` search, max 50;
`players/<id>` username / `signedIn` / regions; `/crafts`,
`/passive-crafts`, `/placeables`, `/stats`, `items/<id>`,
`placeables/<id>`). Its REST lagged behind the game and sometimes
answered 500/404; its websocket could stop delivering on a healthy
connection. Replaced by the relay (user request).

### Data decisions

- **Max stamina / satiation / health**: relay `character_stats_state`
  (1 = max stamina, 19 = max satiation, 0 = max health). Until it
  arrives: highest stamina seen in the last 10 minutes (current value
  always included, since updates stop at full). No cache across
  launches anymore (loading waits for the relay).
- **"Stamina full" / "ready"** is only trusted with the relay max; the fallback
  estimate equals the current value while regenerating, so any fraction of it
  would look reached. Without the API max, only the no-regen rule applies.
- **XP gain** = any skill's quantity increased vs the previous update. Last
  skill = skill with the largest increase in that update. (Observed: XP the
  user expected as Forestry arrived as skill 12 = Fishing; ids follow
  BitJita's `skillMap`, the source of the hardcoded `skill.rs`.)
- **Craft time remaining was removed** (rate-based ETA was too inaccurate).
  Show percent done instead, rounded down so it never shows 100% early.
- A craft is **active** if it progressed in the last 30s; otherwise "Paused".
- **Passive craft done** = reported complete **or** computed end time
  passed (start + recipe `time_requirement`; `passive_craft_timer` is
  private). Duration doesn't account for speed bonuses, so it's an
  estimate.
- **Crop lifecycle** (same for all 132 crop placeables in the catalog):
  `Planted …` → *Fertilizer* action (e.g. 3× Basic Fertilizer) →
  `Fertilized Planted …` → *Water* action (1× Water Bucket) →
  `Growing …` (timer) → `Grown …` → *Harvest*.
- **Stage detection** (`PlantTracker::stage`): name prefix: `Grown `,
  `Growing `, `Fertilized Planted ` (needs water), `Planted ` (needs
  fertilizer). Non-crops → None. Grouped by tier.
- **Time left**: relay `placeable_growth_timer` (exact). Fallback
  estimate when a plant has no timer row = seen-switching-to-growing +
  catalog growth duration (max), marked `~`.

## Project settings

- **Build/run**: `cargo run --release` (tray), `cargo run -- --open` (also
  open the panel), `BITCRAFT_DEBUG=1` (log label/status changes and
  notifications to stderr; the only way to observe the app without a
  screen), `BITCRAFT_NO_NOTIFY=1` (suppress notifications). `cargo test` for
  the core rules and date parsing.
- **Settings file** (tracked character, info window state/position,
  sounds; old `cached_stats` / `food_buff_descs` keys are ignored):
  `<config dir>/bitcraft-tracker/settings.json` (macOS
  `~/Library/Application Support`, Linux `~/.config`, Windows `%APPDATA%`).
- **GPUI** 0.2.2 with `runtime_shaders`: Metal shaders compile at runtime,
  so building on macOS doesn't need Xcode's Metal toolchain (it wasn't
  installed).
- **TLS**: rustls with the **`ring`** provider, installed at the top of
  `main` (reqwest uses `rustls-no-provider`). With both ring and aws-lc
  compiled in, rustls panicked on the first connection ("Could not
  automatically determine the process-level CryptoProvider"); ring also
  builds on Windows without cmake/NASM. Keep `aws-lc-rs` out of the tree
  (`cargo tree -i aws-lc-rs` should find nothing).
- **Tray**: `tray-icon` without default features; on Linux the `ksni`
  (D-Bus StatusNotifierItem) backend, so no GTK. Works on KDE, XFCE, etc.;
  GNOME needs the *AppIndicator and KStatusNotifierItem* extension. Tray
  events are forwarded through `TrayIconEvent::set_event_handler` into an
  async channel (wakes GPUI immediately). Click coordinates are physical
  pixels → divided by `platform::scale_factor()`.
- **Dev profile** has no debug info and no incremental cache to keep
  `target/` around 2 GB (the dev machine had only a few GB of free disk).
- **Platform requirements**: Windows needs the MSVC toolchain; Linux needs
  GPUI's libs (Vulkan, xkbcommon, wayland/X11, fontconfig), e.g.
  `sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libfontconfig-dev libasound2-dev`
  (`libasound2-dev`: ALSA, for rodio's sound output).
- **Releases** (`.github/workflows/release.yml`): pushing a tag `vX.Y.Z`
  builds on GitHub runners and publishes a GitHub release. Bump
  `version` in `Cargo.toml` first; the workflow fails if the tag doesn't
  match (otherwise the update check would compare against the wrong
  version and always say "update available"). Artifacts:
  - Windows: zipped `.exe`. Release builds use the `windows` subsystem
    (`main.rs`), so there's no console window. Unsigned: SmartScreen
    warns (*More info → Run anyway*).
  - macOS: zipped **`.app` bundle**, universal (arm64 + x86_64 via
    `lipo`), `Info.plist` from `packaging/macos/` (`LSUIElement`, bundle
    id `com.github.tortlewortle.bitcraft-tracker`). A bundle rather than a
    bare binary so notifications come from our app, not Terminal (see
    Notifications). Ad-hoc signed only (no Apple Developer account), so
    Gatekeeper blocks the first launch: *System Settings → Privacy &
    Security → Open Anyway*, or `xattr -dr com.apple.quarantine`.
  - Linux: `.tar.gz` with the binary, built on `ubuntu-22.04` (older
    glibc → runs on more distros). Users need the runtime libs listed
    below.
  Repository: `TortleWortle/BitCraftBar` (`Cargo.toml` `repository`).
- **Vendored `xattr` patch** (`vendor/xattr`, `[patch.crates-io]`): gpui
  0.2.2 → `zed-async-tar` → `xattr 0.2.3` uses `libc::ENOATTR`, which
  libc 0.2.19x no longer exports on Linux, so the Linux build failed
  (first release run). The copy uses `ENODATA` on Linux (same value),
  and `u32` instead of the deprecated `libc::uint32_t` on macOS.
  Pinning an older libc wasn't possible (tokio/mio need the new one).
  Drop the patch once gpui stops pulling xattr 0.2.
- **Future-incompat notice silenced** (`.cargo/config.toml`,
  `[future-incompat-report] frequency = "never"`): `block 0.1.6` and
  `proc-macro-error2 2.0.1` (both via gpui 0.2.2) printed it on every
  build/run; not fixable on our side. Remove once gpui drops them.
- **Update check** (`net::check_updates`, user request): at startup and
  every 12h, `GET api.github.com/repos/<repository>/releases/latest`
  (repository = `Cargo.toml` `repository`; latest skips drafts and
  prereleases). If its tag is newer than `CARGO_PKG_VERSION`
  (`core::version::is_newer`), the popup shows *Update available vX.Y.Z*
  with a *Download* button that opens the release page. **Popup only**,
  no notification (user choice). No self-update: replacing a running
  `.exe` / `.app` has edge cases, and a link covers it. Errors (offline,
  no releases yet, rate limit: 60/h unauthenticated) are silent; logged
  with `BITCRAFT_DEBUG=1`.
- **Verified on macOS only** (live data, panel, notifications). Windows and
  Linux haven't been built or run yet; cross-checking Windows from macOS
  fails in `ring`'s build script (needs an MSVC C toolchain).

## Known limitations / ideas

- **Action bar removed** (user: "flickery and not too useful"). The info
  window had a bar for the in-game action (gathering, crafting, …) from
  relay `player_action_state` = [auto_id, chunk_index, entity_id,
  start_time, duration, target, recipe_id, action_type (34-variant sum,
  schema order: None, Attack, DestroyPaving, StationaryEmote, Extract, …,
  Craft = 20, …), layer (Base / UpperBody), …]. Between actions the row
  goes idle, so it blinked on and off with every gather (an empty bar in
  between didn't help enough). start_time/duration units differ per row
  (seen: Base start in µs, UpperBody in ms). The gather bar that came
  later (see Gathering section) uses the target resource instead, held
  for 30s after the last hit, so it doesn't blink.
  rows disappear. `resource_depleted_event` isn't replicated by the relay.

- **Food via REST** (`players/<id>/buffs`) was tried in the Swift app and
  removed: BitJita doesn't keep buffs up to date.
- Skill names are hardcoded from BitJita's old `skillMap`; the relay's
  `skill_desc` (global database) could replace them.
- Passive crafts: whether crafts in one building queue (end times would need
  chaining) is unverified; only Processing (parallel slots) and Complete were
  seen.
- Skill names are hardcoded; unknown ids show as "Skill N".
- Warning color is orange; the user has called it "yellow". Changing it
  affects every warning (Resting, Connecting…, Craft finished).
- The tray label on macOS is single-colored plain text (the Swift app colored
  issues by severity). Coloring would need rendering the text into the icon
  image.
