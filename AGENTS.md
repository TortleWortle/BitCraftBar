# BitCraftTrackerBar

> **Keep this file up to date.** Any agent (or human) changing behavior, data
> sources, status rules, UI layout, or project settings must update the
> relevant section here in the same change. If a decision below is reversed,
> edit it rather than appending a contradiction. Record the *why*, not just the
> *what*.

## What it is

A macOS menu bar app that shows, at a glance, the status of the user's
character in the game **BitCraft**. Data comes from the community site
[BitJita](https://bitjita.com) (live websocket + REST API). There is no Dock
icon or main window; everything lives in the menu bar item and its popover.

The goal is a quick "is my character still doing something useful?" check
while the user is AFK-crafting/gathering, without tabbing into the game.

## What it shows

**Menu bar label** (drawn as a single non-template `NSImage`):

```
(◔) Leatherworking 22% — T2 grown; T1 needs water · Resting
```

1. **Status indicator**: a thin ring in the overall status color, around a
   **stamina pie** in the same color (filled slice = current/max stamina,
   faint tint = empty part). Pie starts at 12 o'clock and fills
   **counter-clockwise** (user preference; clockwise "felt wrong").
2. **Last skill** that received experience.
3. **Craft progress** of the actively worked-on craft, as **percent done**,
   directly after the skill name (no icon, no separator; user preference).
   Hidden when no craft progressed in the last 30s.
4. **Plant summary**, after an em dash, parts joined with `; `:
   `T1, T2 grown` (tiers where *every* plant is grown),
   `T1 needs fertilizer` / `T2 needs water` (tiers where *any* plant needs it).
   Hidden when there's nothing to report. Informational text,
   not a status report (doesn't change the indicator color).
5. **Active issues**: messages of all non-healthy status reports, colored by
   severity, worst first, separated by `·`.

**Offline**: when the character is signed out of BitCraft, a `Player`
report goes **alert / "Offline"** (red ring, red `Offline` in the label; user
asked for red). The label then shows **only the issues**: no skill, craft or
plant summary (user asked to hide plants while offline). Going offline is an
error-state notification (the report is `healthy / Online` while online, so
it's a transition). Activity and Satiation reports are **cleared** while
offline (idling offline isn't a problem, and clearing means coming back online
is a "first report", so it never notifies). Detection: BitJita's
`players/<id>` `signedIn` (polled every 60s), overridden to *online* by any
live websocket event in the last 30s, since `signedIn` can lag. Unknown
`signedIn` counts as online.

**Popover panel**: overall status + per-source reports, stamina bar with
`current / max`, total XP, last skill, last gain, list of crafts (progress
bar, percent, Paused/Done), plants, character picker, Quit. The stats grid
also shows a **satiation bar** like the stamina bar, `current / max`
(green, red at/below the alert threshold).

Passive crafts section: grouped by building + item, with running count, time
until the whole group is done, and how many are ready to collect. Running
groups first. Not shown in the menu bar (user didn't ask for it).

Plants section: a **growth tracker listing only tiers with plants currently
growing** (count + time until the whole tier is grown, `~` when estimated),
then separate rows for *Needs fertilizer*, *Needs water* and *Ready to
harvest* with per-tier counts (user asked for growing-only in the tracker and
the needs shown separately).

## Status rules

Colors: green = healthy, orange = warning, red = alert. Overall = worst report.

| Source | Condition | Status | Message |
|---|---|---|---|
| Activity | last action was a craft and it completed | warning | Craft finished |
| Activity | no counting XP, stamina "ready" (API max known) | alert | Idle: stamina full / Idle: stamina above N% (or Craft idle (N%) if the last action was an unfinished craft) |
| Activity | stamina regenerating (immediately, even with recent XP) | warning | Resting |
| Activity | counting XP | healthy | Gaining experience |
| Activity | last action was an unfinished craft, no XP, no regen | alert | Craft idle (N%) |
| Activity | no XP, no regen in 10s (= no food active) | alert | Idle: no XP, no regen |
| Activity | nothing received yet | alert | No XP or regen yet |

**Craft rules** (user request: yellow when a craft just finished, red when
the last craft still needs progress but you're idle). "Last action was a
craft" = `CraftTracker.lastWorkedCraft` (last craft with live progress, kept
after completion since REST drops completed crafts) progressed no more than
5s before the last XP gain. XP later than that means you moved on (e.g. a new
craft REST hasn't discovered yet), and the normal rules apply. "Craft
finished" stays until then; resting during an unfinished craft is still
"Resting".

There is **no low-stamina warning** (removed): you run out of stamina exactly
when you stop to regenerate, so it only duplicated "Resting".

Rows are checked top to bottom. **Counting XP** = XP gained in the last 20s
*and* after stamina last went up: XP while regenerating is improbable (user's
rule), so once regen starts, earlier XP no longer means "active".
**Regenerating** = stamina went up in the last 10s and hasn't gone down since
(`StaminaTracker.isRegenerating`); spending stamina ends it immediately, so
resuming work flips straight back to green.
| Satiation | satiation ≤ 15 | alert | Low satiation (N) |
| Player | signed out / signed in | alert / healthy | Offline / Online |
| Connection | connected / connecting / disconnected | healthy / warning / alert | Connected / Connecting… / Disconnected |

"Ready" depends on the last skill that gained XP (`ActivityCheck.readyFractionBySkill`):
full stamina by default, **20% for Fishing** (`Skill.fishing`), because fishing rarely
needs full stamina to continue. Add other skills there if they need a
different threshold.

The XP window is **20s** (was 5s) to bridge gaps between XP ticks during
activity; regeneration overrides it (see above).
Use `Skill` cases rather than raw skill ids in rules.

Messages are kept short because they're shown in the menu bar.

## Notifications

macOS notifications with the default sound (`Notifier.swift`, UserNotifications;
banners also shown while the app is active). If permission is denied, only a
sound plays (`NSSound "Glass"`). Sent for:

| Event | Rule |
|---|---|
| Error state | A status source changes **to alert from a known non-alert status** (`StatusMonitor.onChange`). A source's *first* report doesn't count, so the red "No XP or regen yet" at launch is silent. **5 min cooldown per source** (e.g. flapping connection). Alert → alert (message change) is silent. |
| Craft finished | Once per craft, when live progress reaches the total, or when REST reports a tracked craft as `completed` (if the live event was missed). |
| Passive craft finished | When a passive craft's computed end time passes (1s tick) or BitJita reports it `complete`, once per craft. **Batched**: everything finishing in the same check is one notification. Crafts already finished on first load are silent. |
| Plants grown | Per tier, between two placeable polls: growing count drops to 0 **and** grown count increased. Not on the first load. Lags with BitJita's placeable data. |

Connection reports `healthy` when connected (instead of clearing) so a later
disconnect is a transition and notifies.

## Architecture

```
MyApp.swift            MenuBarExtra (.window style), owns AppModel
Notifier.swift         macOS notifications + sound fallback
AppModel.swift         Wires everything; 1s tick; 60s REST refresh; routes events
Status/
  HealthStatus.swift   healthy < warning < alert (Comparable) + colors
  StatusMonitor.swift  Keyed reports from independent sources; overall = max
Networking/
  BitJitaClient.swift  Websocket: subscribe, dynamic subscribe, reconnect w/ backoff,
                       keepalive ping, reconnect on wake / network change
  BitJitaMessages.swift  Websocket message types + channel names
  RelayFeed.swift      Live passive crafts + plants via BitJita's SpacetimeDB relay
  BitJitaAPI.swift     REST: /players?q=, /players/<id>, /crafts, /passive-crafts,
                       /stats, /placeables, /items/<id>,
                       /placeables/<placeableId>; BitJitaDate (flexible timestamps)
Trackers/
  ExperienceTracker.swift  XP totals, last gain time/amount/skill (data only)
  StaminaTracker.swift     stamina, regen/drain times, max stamina (data only)
  SatiationTracker.swift   satiation; reports low satiation
  CraftTracker.swift       crafts from REST + live progress; active craft
  PassiveCraftTracker.swift  passive crafts, computed end times, grouping
  PlantTracker.swift       crops per tier by stage, growth end times
  ActivityCheck.swift      combines XP + stamina into the "Activity" report
  Skill.swift              `enum Skill: Int` (raw value = skill id) + display names
Views/
  MenuBarLabel.swift   Renders the menu bar image (ring, pie, text, symbols)
  StatusPanelView.swift  Popover content
  CharacterPicker.swift  Tracked character name + username search
```

### Key design decisions

- **Status is decoupled from data.** `StatusMonitor` knows nothing about XP or
  stamina. Any source calls `monitor.report(source, status, message)`; the
  indicator shows the worst. Trackers collect data; rules that combine sources
  live in their own check (`ActivityCheck`). Add new warnings by adding a
  report source, not by touching existing ones.
- **No gray text in the menu bar**: `secondaryLabelColor` is hard to read
  there (user feedback). Neutral text and `·` separators use `labelColor`;
  loading uses the warning color.
- **Menu bar label is one drawn `NSImage`**, not SwiftUI views. MenuBarExtra
  labels are template-rendered (monochrome) and ignore most layout, so colors
  and spacing are only reliable when drawn manually. The image uses a drawing
  handler so `labelColor` etc. resolve per light/dark appearance. Avoid emoji
  in the label: they don't follow dark mode (⏱ was nearly invisible); if an
  icon is needed, draw an SF Symbol tinted with `labelColor` at draw time.
- **Everything runs on the main actor** (project default isolation). Message
  volume is tiny, so no background processing.
- **Websocket subscriptions are tracked as a set** and re-sent on reconnect.
  `subscribe()` adds channels at runtime (used for crafts discovered via REST).
  Channels added while the handshake is in flight are sent right after the
  initial subscribe; previously they were silently dropped, which made active
  crafts show as "Paused".
- **Reconnect** with exponential backoff 1s → 30s when send/receive fails.
- **Silent dead connections** (sleep, Wi-Fi switch) don't make `receive()`
  fail, so the client also: pings every 15s and reconnects if no pong within
  10s; reconnects immediately on `NSWorkspace.didWakeNotification`; and
  reconnects (debounced 2s) when `NWPathMonitor` reports a changed, usable
  network. Deliberate reconnects (`reconnect()`) show "Connecting…", not
  "Disconnected", so they don't trigger an error notification.
- **Stale feed**: BitJita can stop delivering events on a connection that
  otherwise stays healthy (pings answered), observed after an outage on their
  side, while a fresh connection gets them. So if `players/<id>` says
  `signedIn` but no live event arrived for 60s on a connection at least 60s
  old, the client reconnects (`AppModel.reconnectIfFeedStale`, checked on the
  60s poll). Relies on satiation updating ~1/s while signed in; a wrong
  `signedIn` only costs a reconnect per minute.
- **Loading state** (`AppModel.isLoading`): at launch and after switching
  character, until the websocket is connected and the core REST requests
  (crafts, stats, placeables, player) finished, capped at 20s. Menu bar shows an orange
  (warning) ring + orange "Loading…", the popover a spinner instead of the data sections, and
  no notifications fire. Typically 1–2s.
- **REST polling**: every 60s, the requests of a round run **in parallel**.
  Initial loading doesn't wait for passive crafts: that endpoint sometimes
  takes ~10s+ (BitJita cache seems cold; fast on repeat), so the popover shows
  a "Passive crafts — Loading…" placeholder until it arrives.
- **Character selection** is by username search (`Views/CharacterPicker.swift`),
  not a raw ID field: the panel shows the tracked character's name with a
  *Change* button that opens a debounced (300ms) search. Picking a result
  switches tracking. `UserDefaults` stores `userId` (entity ID, what all
  channels/endpoints use) and `username` (display only; looked up via
  `players/<id>` if missing). Default `userId` is the original user's character.

## Data sources (BitJita)

### SpacetimeDB relay (passive crafts + plants, live)

BitCraft runs on SpacetimeDB. BitJita runs a read-only relay ("BITCONNECT",
https://relay.bitjita.com) that mirrors it; **no account or BitCraft token**
(the relay issues its own identities; connecting without one works too).
Unofficial, no published terms/limits; could change without notice, hence the
REST fallback. Connecting to Clockwork's own SpacetimeDB with the player's
game token was rejected: account risk, and the relay has the same data.

- `wss://relay.bitjita.com/v1/database/idc-<region>/subscribe`, websocket
  subprotocol `v1.json.spacetimedb`. One database per region; region = the
  player's `regionId` from `players/<id>`. Mirrored: idc-global and
  7, 8, 9, 12, 13, 14, 17, 18, 19 (others: `SubscriptionError` → REST,
  retried every 5 min). While the relay restarts it refuses with "module is
  still loading its initial state from upstream; retry shortly" (`/healthz`
  503, observed for 8+ min); that's retried with the normal backoff. Placeables/crafts in a different region than the player is in are
  not seen.
- Protocol: send `{"Subscribe":{"query_strings":[SQL…],"request_id":1}}`
  (replaces the whole set; also used to add lookups) → `InitialSubscription`
  (all current rows) → `TransactionUpdate`s (`deletes` + `inserts` per table).
  Rows are JSON strings of arrays in column order; sums `[tag, payload]`,
  timestamps `[micros]`. Joins (`SELECT a.* FROM a JOIN b ON … WHERE …`)
  work and can repeat a row once per match → row store is ref-counted.
  `LIMIT` / HTTP `/sql` aren't available. Schema:
  `GET /v1/database/idc-14/schema?version=9`.
- Tables used (column indices in `RelayFeed`): `passive_craft_state`
  (entity, owner, recipe_id, building_entity_id, timestamp, status
  Queued/Processing/Complete, slot), `crafting_recipe_desc` (name,
  time_requirement = duration, crafted_item_stacks), `item_desc`,
  `building_state` → `building_desc` (names), `placeable_state` (owner,
  placeable_id) → `placeable_desc` (name, tag, tier), `growth_state`
  (end_timestamp; empty for crops when checked).
- **Private** (subscribe error): `passive_craft_timer`, so passive end times
  are still start + recipe duration. Processing crafts run in parallel slots
  (`slot = some(n)`); a `Queued` craft is shown without an end time until it
  turns Processing. No Queued rows were seen in region 14.
- Crop growth start isn't stored; it's recorded when a plant is seen
  switching to `Growing …` live (estimate `~` = that + catalog duration). Plants
  already growing at launch have no time left.
- While the relay is live (`RelayFeed.isLive`), the REST passive-crafts and
  placeables results are ignored. Popover header shows `· relay`.

### BitJita live websocket and REST

Websocket `wss://live.bitjita.com/`, subscribe with
`{"type":"subscribe","channels":[...]}`. Server acks with
`{"type":"subscribed","channels":[...]}` (it acks *any* channel name, so an ack
doesn't prove a channel exists). Events: `{"type":"event","channel":...,"data":{...}}`.

| Channel | Data used | Notes |
|---|---|---|
| `experience_state:<player id>` | `experience_stacks[{skill_id, quantity}]` | **No snapshot on subscribe**, only updates. First update counts as a gain (no baseline); skill is known from the second update on. |
| `stamina_state:<player id>` | `stamina` (Double) | ~1/s while regenerating. **Stops sending when full.** |
| `satiation_state:<player id>` | `satiation` (Double) | ~1/s while it changes (slowly decreasing). |
| `character_stats_state:<player id>` | `values[1]` = max stamina | Same `values` array as REST `players/<id>/stats` (format from BitJita's own frontend). **Only sent when a stat changes** (gear/buff), no snapshot on subscribe, so the REST poll still provides the initial value. Keeps max stamina in sync between polls. |
| `progressive_action_state:<craft entity id>` | `progress`, `entity_id` | One event per craft action (+ a duplicate `preparation: true` event). |

REST `https://bitjita.com/api/`:

| Endpoint | Used for |
|---|---|
| `players/<id>/crafts` | Craft list: `entityId`, `progress`, `totalActionsRequired`, `completed`, `craftedItem[].item_id` → `items[].name`, `buildingName`. Polled every 60s to discover new crafts. |
| `players/<id>/passive-crafts` | Passive crafts (tanning tub, kiln, smelter, loom, ...). Same envelope as crafts (`craftResults`, `items`) but each result only has `entityId`, `buildingName`, `recipeId`, `recipeName` (e.g. `"Weave {0}"`, `{0}` = item), `status`, start `timestamp`, `craftedItem`, `slot`. **No progress or end time.** Only `status: "complete"` has been observed; any other value is treated as running. Completed crafts stay listed until collected. Polled every 60s. |
| `items/<itemId>` | `craftingRecipes[]` with `id`, `timeRequirement` (seconds), `isPassive`. Used to get a passive recipe's **duration** (e.g. Weave Rough Cloth Strip = 180s); end = start `timestamp` + duration. Looked up once per recipe. |
| `players/<id>/stats` | `stats.values[1]` = **max stamina**, `stats.values[19]` = **max satiation** (index order from BitJita's frontend stat list: maxHealth, maxStamina, …, satiationRegenRate (18), maxSatiation (19), …; was 100) (stat id 1 is named "Maximum Stamina" in `players/<id>/equipment`). Polled every 60s. Inferred mapping, not documented. |
| `players/<id>/placeables` | `instances[]`: `placeable{name, tag, tier}`, `placeableId`, `availableInteractions[].verbPhrase`, `updatedAt`, `nextStateAt`, `growthEndTimestamp`. Polled every 60s. Crops have tag `Farmed Crop`. Other placeables (e.g. sift collectors, tag `Sift`) are ignored. The `urgency` field is not useful for crops (grown crops report `idle`). |
| `placeables/<placeableId>` | Catalog entry for one placeable type; `growth[].time` = `[min, max]` seconds for timed stages (e.g. Growing Basic Starbulb: 300s). Fetched once per type, only for growing plants without a reported end time. |
| `placeables?q=<text>` | Catalog search; used to discover crop stage names. |
| `players?q=<text>` | Player search: case-insensitive substring on username, **≥ 2 chars**, **max 50 results, alphabetical, no paging** (`page`/`offset`/`limit` ignored). Re-ranked client-side: exact, then prefix, then rest. UI hints to type more when 50 come back. |
| `players/<id>` | `player.username` (name lookup), `player.signedIn` (offline state; polled every 60s). `skillMap` was the source for `Skill.swift` (hardcoded, not fetched). |

Channels probed that produced no events: `character_stats`, `player_state`,
`health_state`, `active_buff_state`. BitJita's frontend also uses
`health_state`, `active_buff_state`, `passive_craft_state:owner_e…`,
`placeable_state`, `growth_state` (not used here yet); like
`character_stats_state` they probably only send on change.

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
  skill = skill with the largest increase in that update.
- **Craft time remaining was removed** (rate-based ETA was too inaccurate).
  Show percent done instead, rounded down so it never shows 100% early.
- A craft is **active** if it progressed in the last 30s; otherwise "Paused".
- **Passive craft done** = BitJita `status == "complete"` **or** computed end
  time passed (computed time is usually earlier than the 60s poll). Duration
  doesn't account for any speed bonuses, so it's an estimate.
- **Crop lifecycle** (same for all 132 crop placeables in the catalog):
  `Planted …` → *Fertilizer* action (e.g. 3× Basic Fertilizer) →
  `Fertilized Planted …` → *Water* action (1× Water Bucket) →
  `Growing …` (timer) → `Grown …` → *Harvest*.
- **Stage detection** (`PlantTracker.stage`): `Grown `/`Growing ` name prefix
  first; otherwise the available interaction (`Fertilizer` / `Water`), falling
  back to the `Fertilized Planted ` / `Planted ` name prefix. Non-crops → nil.
  Grouped by `placeable.tier`.
- **Time left**: `growthEndTimestamp` (or `nextStateAt`) when reported. These
  were always null when checked (nothing was growing), so the format is
  unverified; `BitJitaDate` accepts Postgres-style strings, ISO 8601, and
  epoch seconds/ms. Fallback estimate = `updatedAt` + catalog growth duration
  (max), marked with `~`; it assumes `updatedAt` is when growing started.

## Project settings

- macOS only (`SUPPORTED_PLATFORMS = macosx`); template was multiplatform but
  `MenuBarExtra` is macOS-only. Deployment target macOS 27.
- `INFOPLIST_KEY_LSUIElement = YES` (no Dock icon).
- App Sandbox on, `ENABLE_OUTGOING_NETWORK_CONNECTIONS = YES` (websocket + REST).
- Swift 5 mode, `SWIFT_DEFAULT_ACTOR_ISOLATION = MainActor`. Watch for
  "main actor-isolated … nonisolated context" warnings when passing functions
  as values (e.g. `map(Skill.name)`); use a closure instead.
- **SwiftUI in the popover**: the MenuBarExtra window sizes to its content,
  so a `ScrollView` collapses to almost no height (`maxHeight` only caps it).
  Give scrollable lists an explicit `.frame(height:)` (see `CharacterPicker`:
  fixed row height × min(count, 10)). Use `.fixedSize()` on text that must not
  be truncated (e.g. the stamina value).
- Files are in a file-system-synchronized group: new files in
  `BitCraftTrackerBar/` are picked up without editing the pbxproj.
- Scheme is named `MyApp`. Build:
  `xcodebuild -project BitCraftTrackerBar.xcodeproj -scheme MyApp -destination 'platform=macOS' build`

## Known limitations / ideas

- **Food buff tracking was tried and removed**: `players/<id>/buffs` exists
  (food regen = stat "Passive Stamina Regeneration Rate" > 0), but BitJita
  doesn't keep buffs up to date, so the "No food buff" alert was unreliable.
  Food is inferred instead from stamina regen (Activity rule) and satiation.

- Passive crafts: the in-progress `status` value and whether `slot` implies
  queueing (crafts in one building running one after another rather than in
  parallel) are unverified; all were `complete` with `slot: null` when checked.
  If crafts turn out to queue, end times must chain instead of all starting at
  their own `timestamp`.
- BitJita returns 403 for Python's default `urllib` user agent; curl and
  URLSession work.

- Placeable data freshness depends on BitJita: when first checked, every
  instance had the same `updatedAt` (likely synced periodically or on login),
  so "grown" may lag behind the game. No websocket channel for it was found.

- Skill names are hardcoded; unknown ids show as "Skill N".
- Craft channels are never unsubscribed when crafts finish (harmless, cleared
  on reconnect/user change).
- Warning color is orange (`systemOrange`); user has called it "yellow".
  Changing it affects every warning (Resting, Connecting…).
