# BitCraft Tracker

> **Keep this file up to date.** Any agent (or human) changing behavior, data
> sources, status rules, UI layout, or project settings must update the
> relevant section here in the same change. If a decision below is reversed,
> edit it rather than appending a contradiction. Record the *why*, not just the
> *what*.

## What it is

A tray / menu bar app (macOS, Windows, Linux) that shows, at a glance, the
status of the user's character in the game **BitCraft**. Written in Rust with
[GPUI](https://gpui.rs) (Zed's UI framework). Data comes from the community
site [BitJita](https://bitjita.com): live websocket, REST API, and its
read-only SpacetimeDB relay. There is no main window or Dock icon; everything
lives in the tray icon and its popup panel.

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

Detection (`Model::evaluate`): the relay's `player_state.signed_in` when
available (the game's own value; changes instantly). Without the relay:
BitJita's `players/<id>` `signedIn` (polled every 60s, can lag), overridden
to *online* by any live websocket event in the last 30s. Unknown counts as
online.

**Popup panel** (`src/ui.rs`, `Panel`), opened by clicking the tray icon:
header (status dot, connection state, `· relay` while the relay is live),
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
  delivered buffs (relay only; REST buff data is unreliable).
- **Badges** (`core::badges`, user request; under each section title in
  the popup and in the info window): one pill per *type + tier* and state,
  with a count, e.g. `Grain T1 ×6`, `Ingot T2`, `Rope T5`. Colors: **red**
  = action needed (plant needs fertilizer/water), **orange** = processing
  (growing, passive craft running, craft worked on in the last 30s),
  **gray** = idle (paused craft, queued passive craft), **green** =
  finished (grown, ready to collect, craft complete). Most urgent first.
  Crafts and passive crafts use the crafted item's **tag** + tier (relay
  `item_desc` columns 10 / 9; REST `items[].tag` / `.tier`), falling back
  to the item name. Plants use the crop from the placeable name
  (`<stage> <quality> <crop>`; quality = tier): Embergrain → Grain,
  Starbulb → Bulbs, Wispweave → Fiber (the only three crops in the
  catalog; the user's examples were "Grain" and "Bulbs").
  **Hover text** (`Badge::tooltip`, user request) by state: gray → percent
  done (`8% done`; queued passive crafts: `Queued`), red → what's needed
  (`Needs fertilizer`, `3 need fertilizer · 1 needs water`), orange → time
  left (`4m left`, `next ~4m · all 14m`, `~` = estimate, `Time left
  unknown` for plants already growing at launch), green → `Ready to
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
  with percent and bar (hidden offline);
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
| Food | food regen buff active / none (relay data only) | healthy / alert | Nm Ss left / No food |
| Player | signed out / signed in | alert / healthy | Player offline / Online |
| Connection | connected / connecting / disconnected | healthy / warning / alert | Connected / Connecting… / Disconnected |

Activity rows are checked top to bottom.

- **Active** (user request): moving around means the player is at the
  keyboard, so it overrides every other Activity rule. Movement = the
  `mobile_entity_state` location changed (`Model::update_position` →
  `moved_at`, window `ActivityCheck::move_window` = 10s). Source: BitJita
  live channel (works without the relay; first version used the relay
  only and did nothing while the relay was restarting) and the relay
  when live. **Idle right after Active is quiet** (user request: the
  player stopped the action by walking away, not worth an alert): when
  Activity goes from "Active" to an idle message, neither the went-idle
  notification nor the idle sound fires, until Activity is something other
  than idle again (`Model::idle_after_moving`).

- **Counting XP** = XP gained in the last 20s *and* after stamina last went
  up: XP while regenerating is improbable (user's rule), so once regen starts,
  earlier XP no longer means "active". The 20s window (was 5s) bridges gaps
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
  after completion since REST drops completed crafts) progressed no more than
  5s before the last XP gain. XP later than that means you moved on (e.g. a
  new craft REST hasn't discovered yet), and the normal rules apply. "Craft
  finished" stays until then; resting during an unfinished craft is still
  "Resting".
- There is **no low-stamina warning** (removed): you run out of stamina
  exactly when you stop to regenerate, so it only duplicated "Resting".
- **Food** (user request, re-added once reliable data existed): reported
  only after the relay delivered `active_buff_state`; cleared while offline.
  Only the "Food Regen" buff counts (every food gives one); extra "Food
  Buffs" alone (e.g. action speed) don't. "No food" is a normal alert, so it
  notifies when food runs out (5 min cooldown).
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
| Craft finished | Once per craft, when live progress reaches the total, or when REST reports a tracked craft as `completed` (if the live event was missed). |
| Passive craft finished | When a passive craft's computed end time passes (1s tick) or it's reported complete, once per craft. **Batched**: everything finishing in the same check is one notification. Crafts already finished on first load are silent, and so are those in the first list after **switching source** (REST ↔ relay): REST once listed no passive crafts while the relay had 3 finished ones, which notified as "finished" (`Model::switch_source`). |
| Plants grown | Per tier, between two updates: growing count drops to 0 **and** grown count increased. Not on the first load or the first list after a source switch. |

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
                   (live task + 60s REST poll + relay); caches
  live.rs          BitJita websocket: subscribe, ping, reconnect w/ backoff
  api.rs           BitJita REST + flexible date parsing (`parse_date`)
  relay.rs         SpacetimeDB relay: passive crafts + plants, live
src/tray.rs        Tray icon rendering/updates
src/notifier.rs    Desktop notifications
src/sound.rs       Alert sounds (synthesized chimes, rodio)
src/settings.rs    settings.json (tracked character)
```

**Data flow is one way**: the network thread sends `net::Outgoing` messages
(per-character events tagged with the user id, search results) over an
`async_channel`; a GPUI foreground task feeds them into `Model::handle`. The
model produces `net::Command`s (track character, subscribe, reconnect,
search) and notifications, which `AppState::flush` forwards, then updates the
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
- **Websocket subscriptions are tracked as a set** and re-sent on reconnect;
  `Command::Subscribe` adds channels at runtime (crafts discovered via REST).
  Channels added while (re)connecting are kept and sent with the next
  subscribe (losing them once made active crafts show as "Paused").
- **Reconnect** with exponential backoff 1s → 30s. **Keepalive**: ping every
  15s; a ping still unanswered at the next tick (> 10s) fails the connection
  (silent dead sockets after sleep / Wi-Fi switch). No explicit
  reconnect-on-wake or network-change handling (the Swift app had both); the
  ping detects it within ~30s.
- **Stale feed**: BitJita can stop delivering events on a connection that
  otherwise stays healthy (pings answered), observed after an outage on their
  side, while a fresh connection gets them. So if `players/<id>` says
  `signedIn` but no live event arrived for 60s on a connection at least 60s
  old, the model sends `Command::ReconnectLive` (`reconnect_if_feed_stale`,
  checked on the 60s poll). Relies on satiation updating ~1/s while signed
  in; a wrong `signedIn` only costs a reconnect per minute.
- **Loading state** (`Model::is_loading`): at launch and after switching
  character, until the websocket is connected and the core REST requests
  (crafts, stats, placeables, player) finished, capped at 20s. Tray shows the
  orange ring + "Loading…", the panel "Loading <name>…" instead of the data
  sections, and no notifications fire. Typically 1–2s.
- **REST polling**: every 60s, the requests of a round run **in parallel**
  (`tokio::join!`). Loading doesn't wait for passive crafts: that endpoint
  sometimes takes ~10s+ (BitJita cache seems cold; fast on repeat), so the
  panel shows a "Passive crafts — Loading…" placeholder until it arrives.
- **Character selection** is by username search in the panel: the
  tracked character's name with a *Change* button that opens a search field
  (typed via key events; GPUI has no text input widget), debounced 300ms,
  results clickable, Enter picks the first, Escape cancels. `settings.json`
  stores `user_id` (entity ID, what all channels/endpoints use) and `username`
  (display only; refreshed from `players/<id>`). Default `user_id` is the
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

## Data sources (BitJita)

### SpacetimeDB relay (crafts, passive crafts, plants, food; live) — `src/net/relay.rs`, hub in `src/net/mod.rs`

BitCraft runs on SpacetimeDB. BitJita runs a read-only relay ("BITCONNECT",
https://relay.bitjita.com) that mirrors it; **no account or BitCraft token**
(the relay issues its own identities; connecting without one works too).
Unofficial, no published terms/limits; could change without notice, hence the
REST fallback. Connecting to Clockwork's own SpacetimeDB with the player's
game token was rejected: account risk, and the relay has the same data.

- `wss://relay.bitjita.com/v1/database/idc-<region>/subscribe`, websocket
  subprotocol `v1.json.spacetimedb`. **One database per region**, and rows
  live in the region of the *thing*: placeables and crafting stations stay
  in their region, while the player's buffs move with the player. (Bug
  found: following only the player's region showed 0 plants/crafts after
  the player traveled from region 14 to 13, while buffs still came through.)
  So the **relay hub** (`relay_hub`) runs one feed per region: every
  mirrored region (`MIRRORED_REGIONS`: 7, 8, 9, 12, 13, 14, 17, 18, 19), plus
  the player's `regionId` and their claims' regions (`players/<id>`), updated
  on each 60s poll. All queries filter by the character, so each is small.
  Crafts, passive crafts and plants are merged across regions; food comes
  from the player's region.
- **Liveness**: the relay counts as live (`RelayState::Live`, shared
  `AtomicBool`) only when **every** followed region is live; regions that
  refuse for good (`SubscriptionError` other than "loading", e.g. not
  mirrored; retried every 5 min) are ignored. While not live, REST provides
  crafts/plants/passive crafts and relay lists aren't sent, so a region
  that's still loading never hides data. While the relay restarts it refuses
  with "module is still loading its initial state from upstream; retry
  shortly" (`/healthz` 503, observed for 8+ min; it also briefly served
  incomplete tables); that's retried with the normal backoff.
- Protocol: send `{"Subscribe":{"query_strings":[SQL…],"request_id":1}}`
  (replaces the whole set; also used to add lookups) → `InitialSubscription`
  (all current rows) → `TransactionUpdate`s (`deletes` + `inserts` per table).
  Rows are JSON strings of arrays in column order; sums `[tag, payload]`,
  timestamps `[micros]`. Joins (`SELECT a.* FROM a JOIN b ON … WHERE …`)
  work and can repeat a row once per match → the row store is ref-counted.
  `LIMIT` / HTTP `/sql` aren't available. Schema (column order/types):
  `GET /v1/database/idc-14/schema?version=9`.
- **Food buffs** (`Feed::food`): `active_buff_state` (pk entity_id) =
  `[entity_id, [[buff_id, [start], duration, values], …]]`, ~370 entries
  per player; inactive ones have start `[0]`. **start is Unix seconds**,
  duration seconds (food: 900/1800/3600). `buff_desc` is subscribed only for
  the two food buff types (`buff_type_id` 1790361536 "Food Regen", 11 rows;
  861783812 "Food Buffs", 96 rows): a match = food buff; column 2 = name.
  Expiry is computed from start + duration on each tick. Not subscribed
  anymore: `food_desc` (413 rows; column 8 = `[[buff_id, duration option],
  …]` maps food items → buffs). It was used to name the food, but 369 foods
  share 223 buff sets (often ambiguous) and the names were too long. `buff_desc.online_timestamp` is true for food buffs; whether
  that pauses the timer while signed out is unverified (it doesn't matter
  while playing).
- **Player rows** (`Feed::signed_in`, `Feed::stats`): `player_state` (pk is
  **column 1**, entity_id; column 0 is the teleport location, hence
  `pk_column`) column 6 = `signed_in` (`sign_in_timestamp` resets to 0 on
  sign-out); `character_stats_state` = [entity_id, values] (same indices as
  REST stats: 1 = max stamina 262, 19 = max satiation 100, verified equal).
  Like buffs, these rows **move with the player** (present in region 13 and
  absent in 14 after traveling), so the hub takes the player's rows (food,
  stats, online status) from the live region that has `player_state` and
  sends `NetEvent::RelayPlayer` (None when no live region has it → REST
  fallback). `health_state` = [entity_id, last_health_decrease_timestamp,
  health, died_timestamp] is subscribed the same way (info window; no
  REST fallback, BitJita's live `health_state` channel sent nothing when
  probed).
- **Position** (`Feed::position`): `mobile_entity_state` = [entity_id,
  chunk_index, timestamp, location_x, location_z, destination_x,
  destination_z, dimension, is_walking, …]; location changes → "Active"
  (see Status rules). Player region, like the other player rows.
- **Active crafts** (`Feed::crafts`): `progressive_action_state` (owner
  filter) = [entity_id, building_entity_id, function_type, progress,
  recipe_id, craft_count, last_crit_outcome, owner, lock_expiration,
  preparation]; total = recipe `actions_required` (column 14) × craft_count
  (= REST `totalActionsRequired`, verified 130 × 552 = 71,760); name via
  recipe → `item_desc`, building via `building_state` → `building_desc`.
  Crafts whose recipe hasn't arrived yet are skipped (total 0 would look
  finished). Progress changes arrive live and count as "last progressed".
  The BitJita live channel is still subscribed as a fallback.
- Tables used (column indices in `relay.rs`): `passive_craft_state`
  (entity, owner, recipe_id, building_entity_id, timestamp, status
  Queued/Processing/Complete, slot), `crafting_recipe_desc` (name,
  time_requirement = duration, crafted_item_stacks), `item_desc`,
  `building_state` → `building_desc` (names; second lookup round),
  `placeable_state` (owner, placeable_id) → `placeable_desc` (name, tag,
  tier), `growth_state` (end_timestamp; empty for crops when checked).
- **Private** (subscribe error): `passive_craft_timer`, so passive end times
  are still start + recipe duration. Processing crafts run in parallel slots
  (`slot = some(n)`); a `Queued` craft is shown without an end time until it
  turns Processing. No Queued rows were seen in region 14 (51,769 rows:
  47,933 complete, 3,836 processing).
- Crop growth start isn't stored; it's recorded when a plant is seen
  switching to `Growing …` live (estimate `~` = that + catalog duration).
  Plants already growing at launch have no time left.
- While the relay is live, the REST crafts, passive-crafts and placeables
  requests are skipped / their results dropped (BitJita's REST also
  sometimes answers 500 for crafts/placeables).

### BitJita live websocket — `src/net/live.rs`

`wss://live.bitjita.com/`, subscribe with
`{"type":"subscribe","channels":[...]}`. Server acks with
`{"type":"subscribed","channels":[...]}` (it acks *any* channel name, so an ack
doesn't prove a channel exists). Events: `{"type":"event","channel":...,"data":{...}}`.

| Channel | Data used | Notes |
|---|---|---|
| `experience_state:<player id>` | `experience_stacks[{skill_id, quantity}]` | **No snapshot on subscribe**, only updates. First update counts as a gain (no baseline); skill is known from the second update on. |
| `stamina_state:<player id>` | `stamina` | ~1/s while regenerating. **Stops sending when full.** |
| `satiation_state:<player id>` | `satiation` | ~1/s while it changes (slowly decreasing). |
| `character_stats_state:<player id>` | `values[1]` = max stamina, `values[19]` = max satiation | Same `values` array as REST `players/<id>/stats` (format from BitJita's own frontend). **Only sent when a stat changes** (gear/buff), no snapshot on subscribe. Fallback for when the relay's `character_stats_state` isn't available. |
| `mobile_entity_state:<player id>` | `location_x`, `location_z` (also `destination_*`, `is_walking`, `timestamp` ms, `region_id`) | **~16 events/s** (330 in 20s while playing), so `live::handle` sends a `Position` at most once per second and only when it changed. Movement → "Active". |
| `progressive_action_state:<craft entity id>` | `progress`, `entity_id` | One event per craft action (+ a duplicate `preparation: true` event). |

Channels probed that produced no events: `character_stats`, `player_state`,
`health_state`, `active_buff_state`. BitJita's frontend also uses
`health_state`, `active_buff_state`, `passive_craft_state:owner_entity_id:<id>`,
`placeable_state`, `growth_state`; like `character_stats_state` they probably
only send on change.

### BitJita REST — `src/net/api.rs`

`https://bitjita.com/api/` (sent with a plain `BitCraftTracker/0.1` user
agent: Python's default `urllib` agent got 403).

| Endpoint | Used for |
|---|---|
| `players/<id>/crafts` | Craft list: `entityId`, `progress`, `totalActionsRequired`, `completed`, `craftedItem[].item_id` → `items[].name`, `buildingName`. Polled every 60s to discover new crafts (then followed live). |
| `players/<id>/passive-crafts` | Passive crafts (fallback when the relay isn't live). Same envelope as crafts (`craftResults`, `items`) but each result only has `entityId`, `buildingName`, `recipeId`, `recipeName` (e.g. `"Weave {0}"`, `{0}` = item), `status`, start `timestamp`, `craftedItem`, `slot`. **No progress or end time.** Only `status: "complete"` observed; anything else = running. Completed crafts stay listed until collected. Can lag behind the game considerably (reason for the relay). |
| `items/<itemId>` | `craftingRecipes[]` with `id`, `timeRequirement` (seconds). A passive recipe's **duration** (e.g. Weave Rough Cloth Strip = 180s); end = start `timestamp` + duration. Cached per recipe. |
| `players/<id>/stats` | `stats.values[1]` = **max stamina**, `stats.values[19]` = **max satiation** (index order from BitJita's frontend stat list: maxHealth, maxStamina, …, satiationRegenRate (18), maxSatiation (19), …). Polled every 60s; ignored while the relay provides the stats (REST lags behind gear/buff changes). Inferred mapping, not documented. |
| `players/<id>/placeables` | Plants fallback. `instances[]`: `placeable{name, tag, tier}`, `placeableId`, `availableInteractions[].verbPhrase`, `updatedAt`, `nextStateAt`, `growthEndTimestamp`. Crops have tag `Farmed Crop`; others (e.g. sift collectors, `Sift`) are ignored. `urgency` is not useful for crops. |
| `placeables/<placeableId>` | Catalog entry; `growth[].time` = `[min, max]` seconds for timed stages (e.g. Growing Basic Starbulb: 300s). Cached per type; only fetched for growing plants with a start time and no reported end. |
| `players?q=<text>` | Player search: case-insensitive substring on username, **≥ 2 chars**, **max 50 results, alphabetical, no paging**. Re-ranked client-side: exact, then prefix, then rest. UI hints to type more when 50 come back. |
| `players/<id>` | `username`, `signedIn` (offline state; fallback when the relay has no `player_state`), `regionId` + `claims[].regionId` (extra relay regions to follow). Polled every 60s. `skillMap` was the source for `skill.rs` (hardcoded). |

### Data decisions

- **Out-of-order events between sources** (user: "events feel flickery").
  Single-source feeds (XP, stamina, satiation, relay health/food)
  arrive in order over one websocket and are applied as they come. Where
  two sources report the same row, older values are dropped:
  - **Position** (BitJita live, throttled to 1/s, + relay): both carry the
    row's `timestamp` (ms); `Model::update_position` ignores a position
    older than the last one. Otherwise the throttled live copy could arrive
    after the relay's newer one and count as moving back and forth.
  - **Craft progress** (relay + BitJita live channel + REST): only ever
    goes up (`CraftTracker::handle_progress` ignores values ≤ current;
    REST merges with `max`).
  - **Max stamina/satiation**: the relay's stats win; REST and BitJita
    live `character_stats_state` are only applied while the relay has none.

- **Max stamina / satiation**: relay `character_stats_state` when
  available, else REST stats + BitJita live `character_stats_state`
  (whichever came last). The last values are **cached** in
  `settings.json` (`cached_stats`, per tracked character, cleared on
  switching) and applied at startup: the relay restarts for 10+ minutes
  at a time and `players/<id>/stats` has answered 404 meanwhile; if current stamina exceeds it
  (API lagging after a gear change), use current. Fallback without API: highest
  stamina seen in the last 10 minutes (current value always included, since
  updates stop at full).
- **"Stamina full" / "ready"** is only trusted with the API max; the fallback
  estimate equals the current value while regenerating, so any fraction of it
  would look reached. Without the API max, only the no-regen rule applies.
- **XP gain** = any skill's quantity increased vs the previous update. Last
  skill = skill with the largest increase in that update. (Observed: XP the
  user expected as Forestry arrived as skill 12 = Fishing; ids follow
  BitJita's `skillMap`.)
- **Craft time remaining was removed** (rate-based ETA was too inaccurate).
  Show percent done instead, rounded down so it never shows 100% early.
- A craft is **active** if it progressed in the last 30s; otherwise "Paused".
- **Passive craft done** = reported complete **or** computed end time passed.
  Duration doesn't account for speed bonuses, so it's an estimate.
- **Crop lifecycle** (same for all 132 crop placeables in the catalog):
  `Planted …` → *Fertilizer* action (e.g. 3× Basic Fertilizer) →
  `Fertilized Planted …` → *Water* action (1× Water Bucket) →
  `Growing …` (timer) → `Grown …` → *Harvest*.
- **Stage detection** (`PlantTracker::stage`): `Grown `/`Growing ` name prefix
  first; otherwise the available interaction (`fertilizer` / `water`, REST
  only), falling back to the `Fertilized Planted ` / `Planted ` name prefix.
  Non-crops → None. Grouped by tier.
- **Time left**: reported end (relay `growth_state`, REST
  `growthEndTimestamp` / `nextStateAt`) when present — always empty/null when
  checked. Fallback estimate = start + catalog growth duration (max), marked
  `~`; start = seen-switching-to-growing (relay) or `updatedAt` (REST,
  assumed to be when growing started). `api::parse_date` accepts
  Postgres-style strings, ISO 8601 and epoch seconds/ms.

## Project settings

- **Build/run**: `cargo run --release` (tray), `cargo run -- --open` (also
  open the panel), `BITCRAFT_DEBUG=1` (log label/status changes and
  notifications to stderr; the only way to observe the app without a
  screen), `BITCRAFT_NO_NOTIFY=1` (suppress notifications). `cargo test` for
  the core rules and date parsing.
- **Settings file** (tracked character, info window state/position):
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
  (seen: Base start in µs, UpperBody in ms). Gather target health was
  also explored for it: `extract_outcome_state` (player → last
  `target_entity_id`), `resource_health_state` (entity_id, health),
  `resource_state` → `resource_desc.max_health`; a used-up resource's
  rows disappear. `resource_depleted_event` isn't replicated by the relay.

- **Food via REST** (`players/<id>/buffs`) was tried in the Swift app and
  removed: BitJita doesn't keep buffs up to date. Food now comes from the
  relay only; without the relay (unmirrored region) there's no Food report,
  and food is inferred from stamina regen (Activity rule) and satiation.
- Relay `player_state` has `time_signed_in`, `sign_in_timestamp`,
  `signed_in`: a possible live alternative to the REST `signedIn` poll.
- Passive crafts: whether crafts in one building queue (end times would need
  chaining) is unverified; only Processing (parallel slots) and Complete were
  seen.
- Placeable data freshness via REST depends on BitJita (all instances had the
  same `updatedAt` when first checked); the relay fixes this when live.
- Skill names are hardcoded; unknown ids show as "Skill N".
- Craft channels are never unsubscribed when crafts finish (harmless, reset on
  character change).
- Warning color is orange; the user has called it "yellow". Changing it
  affects every warning (Resting, Connecting…, Craft finished).
- The tray label on macOS is single-colored plain text (the Swift app colored
  issues by severity). Coloring would need rendering the text into the icon
  image.
