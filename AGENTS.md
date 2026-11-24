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
report goes **alert / "Offline"** (red ring; user asked for red). The label
then shows **only the issues**: no skill, craft or plant summary (user asked
to hide plants while offline). Going offline is an error-state notification
(the report is `healthy / Online` while online, so it's a transition).
Activity and Satiation reports are **cleared** while offline (idling offline
isn't a problem, and clearing means coming back online is a "first report",
so it never notifies). Detection: BitJita's `players/<id>` `signedIn` (polled
every 60s), overridden to *online* by any live websocket event in the last
30s, since `signedIn` can lag. Unknown `signedIn` counts as online.

**Popup panel** (`src/ui.rs`, `Panel`), opened by clicking the tray icon:
header (status dot, connection state, `· relay` while the relay is live),
per-source status reports, stats rows (stamina bar `current / max` in the
overall status color; **satiation bar** `current / max`, green, red at/below
the alert threshold; total XP; last skill; **food**; last gain), plants,
passive crafts,
crafts (progress bar, percent, Paused/Done), character picker, Quit.

- **Plants section**: a **growth tracker listing only tiers with plants
  currently growing** (count + time until the whole tier is grown, `~` when
  estimated; `coarse_duration_string`: `1h 05m`, `4m` (minutes rounded up),
  seconds only under a minute, `42s`; user preference), then separate rows for *Needs fertilizer*, *Needs water* and
  *Ready to harvest* with per-tier counts (user asked for growing-only in the
  tracker and the needs shown separately).
- **Passive crafts section**: grouped by building + item, with running count,
  time until the whole group is done, and how many are ready to collect.
  Running groups first. Not in the label (user didn't ask for it).
- **Food row**: time until the food regen runs out (`45m 39s left`) with a
  short summary of the active food buffs below it (`Regen 3 · Deluxe Action
  Speed 2`; `FoodBuff::short_name` turns "Level 3 Food Regen" into "Regen
  3"). Food *names* were shown first but dropped (user: too long; several
  foods give the same buffs, so it was often "A / B"). Red "None" when no
  food regen is active; "–" until the relay delivered buffs (relay only;
  REST buff data is unreliable).
- Light/dark colors follow the window appearance (`Theme`).

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
| Activity | last action was a craft and it completed | warning | Craft finished |
| Activity | no counting XP, stamina "ready" (API max known) | warning | Idle: stamina full / Idle: stamina above N% (or Craft idle (N%) if the last action was an unfinished craft) |
| Activity | stamina regenerating (immediately, even with recent XP) | warning | Resting |
| Activity | counting XP | healthy | Gaining experience |
| Activity | last action was an unfinished craft, no XP, no regen | warning | Craft idle (N%) |
| Activity | no XP, no regen in 10s | warning | Idle: no XP, no regen |
| Activity | nothing received yet | warning | No XP or regen yet |
| Satiation | satiation ≤ 15 | alert | Low satiation (N) |
| Food | food regen buff active / none (relay data only) | healthy / alert | Nm Ss left / No food |
| Player | signed out / signed in | alert / healthy | Offline / Online |
| Connection | connected / connecting / disconnected | healthy / warning / alert | Connected / Connecting… / Disconnected |

Activity rows are checked top to bottom.

- **Counting XP** = XP gained in the last 20s *and* after stamina last went
  up: XP while regenerating is improbable (user's rule), so once regen starts,
  earlier XP no longer means "active". The 20s window (was 5s) bridges gaps
  between XP ticks; regeneration overrides it.
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
| Went idle | Activity changes to an idle message (`ActivityCheck::is_idle`: "Idle…", "Craft idle…") from a known non-idle one (e.g. Gaining experience / Resting → Idle: stamina full). Idle → idle (message change) is silent. Same 5 min cooldown. Idle used to be red and notified as an error state; it became orange, and this keeps the notification. |
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

## Architecture

```
src/main.rs        GPUI app; tray click → popup placement; keepalive window
src/ui.rs          AppState (model + tray + popup + search) and the Panel view
src/model.rs       Routes network events into trackers; loading/offline;
                   status evaluation; notifications; label text
src/platform.rs    Native bits GPUI lacks: tray DPI scale, hide Dock icon,
                   set popup window frame (macOS AppKit / Win32)
src/core/          No GPUI, no I/O (unit-tested):
  status.rs        HealthStatus (healthy < warning < alert) + StatusMonitor
  trackers.rs      Experience/Stamina/Satiation/Craft/Plant/PassiveCraft
                   trackers + ActivityCheck
  skill.rs         `enum Skill` (value = skill id) + display names
  types.rs         Data passed from net → model (CraftInfo, PassiveCraft,
                   Plant, PlayerSummary); times are Unix seconds (f64)
src/net/           Background Tokio runtime ("network" thread):
  mod.rs           Command/Outgoing channels; per-character tracking
                   (live task + 60s REST poll + relay); caches
  live.rs          BitJita websocket: subscribe, ping, reconnect w/ backoff
  api.rs           BitJita REST + flexible date parsing (`parse_date`)
  relay.rs         SpacetimeDB relay: passive crafts + plants, live
src/tray.rs        Tray icon rendering/updates
src/notifier.rs    Desktop notifications
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
  Windows taskbar); closes when it loses focus. A click that just closed it
  (focus loss happens before the tray click) doesn't reopen it (400ms).
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

### SpacetimeDB relay (passive crafts + plants, live) — `src/net/relay.rs`

BitCraft runs on SpacetimeDB. BitJita runs a read-only relay ("BITCONNECT",
https://relay.bitjita.com) that mirrors it; **no account or BitCraft token**
(the relay issues its own identities; connecting without one works too).
Unofficial, no published terms/limits; could change without notice, hence the
REST fallback. Connecting to Clockwork's own SpacetimeDB with the player's
game token was rejected: account risk, and the relay has the same data.

- `wss://relay.bitjita.com/v1/database/idc-<region>/subscribe`, websocket
  subprotocol `v1.json.spacetimedb`. One database per region; region = the
  player's `regionId` from `players/<id>` (relay started/restarted by the
  poll loop when it's known or changes). Mirrored: idc-global and 7, 8, 9,
  12, 13, 14, 17, 18, 19 (others: `SubscriptionError` → REST, retried every
  5 min). While the relay restarts it refuses with "module is still loading
  its initial state from upstream; retry shortly" (`/healthz` 503, observed
  for 8+ min); that's retried with the normal backoff. Placeables/crafts in a
  different region than the player is in are not seen.
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
- While the relay is live (shared `AtomicBool` in `net/mod.rs`), the REST
  passive-crafts and placeables requests are skipped / their results dropped.

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
| `character_stats_state:<player id>` | `values[1]` = max stamina, `values[19]` = max satiation | Same `values` array as REST `players/<id>/stats` (format from BitJita's own frontend). **Only sent when a stat changes** (gear/buff), no snapshot on subscribe, so the REST poll provides the initial value. |
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
| `players/<id>/stats` | `stats.values[1]` = **max stamina**, `stats.values[19]` = **max satiation** (index order from BitJita's frontend stat list: maxHealth, maxStamina, …, satiationRegenRate (18), maxSatiation (19), …). Polled every 60s. Inferred mapping, not documented. |
| `players/<id>/placeables` | Plants fallback. `instances[]`: `placeable{name, tag, tier}`, `placeableId`, `availableInteractions[].verbPhrase`, `updatedAt`, `nextStateAt`, `growthEndTimestamp`. Crops have tag `Farmed Crop`; others (e.g. sift collectors, `Sift`) are ignored. `urgency` is not useful for crops. |
| `placeables/<placeableId>` | Catalog entry; `growth[].time` = `[min, max]` seconds for timed stages (e.g. Growing Basic Starbulb: 300s). Cached per type; only fetched for growing plants with a start time and no reported end. |
| `players?q=<text>` | Player search: case-insensitive substring on username, **≥ 2 chars**, **max 50 results, alphabetical, no paging**. Re-ranked client-side: exact, then prefix, then rest. UI hints to type more when 50 come back. |
| `players/<id>` | `username`, `signedIn` (offline state), `regionId` (relay database). Polled every 60s. `skillMap` was the source for `skill.rs` (hardcoded). |

### Data decisions

- **Max stamina**: API value when available (REST poll + live
  `character_stats_state`, whichever came last); if current stamina exceeds it
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
- **Settings file**: `<config dir>/bitcraft-tracker/settings.json` (macOS
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
  `sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libfontconfig-dev`.
- **Verified on macOS only** (live data, panel, notifications). Windows and
  Linux haven't been built or run yet; cross-checking Windows from macOS
  fails in `ring`'s build script (needs an MSVC C toolchain).

## Known limitations / ideas

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
