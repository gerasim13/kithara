# kithara-app — Context

Contracts and invariants for the kithara-app crate; the README is the overview.

## Buffer-pool ownership

`pools::AppPools` is the desktop composition schema. `main` builds one
`PoolRegion<AppPools>` and gives the same facade to the asset store, HTTP
client, playback worker, queues, and analysis cache. Its `u8` and `f32` slots
compete under one 256 MiB hard cap; startup allocation is declared in the
schema configuration rather than warmed later by a component.

## Broadcast service

The crate owns only the service wiring; the packaging and the origin belong to `kithara-broadcast`. A request
stays `Requested` until the Host exposes its measured output rate, which configures both the ring and the
encoder and arms the single Host mix tap. App-root cancellation ends the origin and encoder; stopping the
running phase releases the tap before the encoder drains.

Stopping blocks — it closes the feed, drains the encoder and joins the worker — so the toggle moves the handle
into an iced task and marks the service `Stopping`; only that task's completion message makes it `Off`. The GUI
tick polls `BroadcastHandle::status`, so a producer released by a device-rate change reaches `Off` the same way.

The canon puts this control in the app menu and a recorder module and the app has neither, so its REC cell sits
in the bar beside the CPU cell.

## UI host

The UI is a compiled `kithara-ui` document set and `gui::ui` is the host side of it. `Registry` declares every
endpoint the documents may bind, `AppUi::new` compiles both layout documents against it and returns
`UiDocError`, and a unit test compiles both, so a compile failure is a build defect rather than a runtime
condition. `compile_ui` merges `builtin::text_doc()` with `assets/ui/app-en.ktext.ron` before every compile;
that catalog holds only the window-manager menu words canon has no key for.

### Where the UI package is read from

`AppConfig.ui_package` names the folder holding the UI package. `main` defaults it to `assets/ui` beside the
executable, which is where a release lays its documents out, and `--ui-package` overrides it. `AppUi::new`
reads that folder over what the build embeds, so changing a document on disk changes the interface at the next
start without a rebuild.

A path that does not exist means no package was laid out and the build's own documents draw; that is what a
developer running from a build directory sees. Anything else that stops the folder being read - a permission,
a manifest that fails the `kithara-ui` contract - stops the application rather than quietly drawing the
built-in one. This is the one place the application accepts a missing input as an answer, and it is a
user-facing default rather than a state-resolution fallback: the package is optional configuration, and its
absence is not evidence of a broken contract.

`gui::ui::package::Package` is the single owner of one loaded package: the resolver it is read through, the
screens it answered for, and the skin and catalog it dresses them in. Both hosts read from that one value -
the iced host paints with `Package::skin`, and the retained host builds its window `Config` from the same
resolver and catalog rather than loading a second copy. Two packages drawing one application is the failure
this shape exists to prevent.

The application asks the package for `deck-single` and `deck-dual` by role, and `Package` resolves both once.
A manifest may also name a skin document and a caption catalog; naming a skin is what lets a package change
how the application looks without a rebuild. A manifest that names neither wears the built-in skin and the
built-in words, which is a package carrying pages and nothing else - declared optionality, not a fallback.

`Package::REQUIRED` is the whole of what a package must answer for, checked once each screen compiles:

- `deck-a/play` - the only path that starts and stops playback. A screen without it draws a player that
  cannot play.
- `deck-a/wave` - the only path that moves the position within a track. A screen without it can start a
  track and never move inside it.

Everything else a screen offers is the package's own business. The minimum is checked rather than assumed
because a screen missing a path still compiles and still draws; only the paths it answers on say whether the
application can reach it, and a press that lands nowhere reads as a dead button rather than as a package
defect.

Reading the package from disk costs 1.7 ms once at start: 10.1 ms against 8.4 ms for the same documents
embedded, 17 files and 62 KiB, measured on this laptop under `test-release`. Compilation dominates both, which
is why the resolver caches what it read rather than indexing what it might read.

### Deck addressing

A deck is addressed by channel letter, and the letter is its position in the session. The letter appears in two
independent places — the control path (`deck-<letter>/`, `mixer/<letter>/`, `overview/<letter>/`) and the
`deck=` scope of a binding — so they must agree; a unit test walks the compiled tree asserting every
deck-scoped binding is addressed by the letter it reads. The micro bar is the one place carrying no letter:
`gui::ui::scope::MICRO_DECK` names the deck it drives and the same test admits `micro-bar/` only for that deck.
`scope` owns the mapping both ways (`deck_index`, `deck_letter`). Only lowercase ASCII maps to a position, and
the session bounds the letter, so one past the last deck resolves to nothing rather than a neighbour.

One position indexes every list a deck appears in: `Decks` is built from `DeckSet::decks()` in session order and
`ViewCache::refresh` resizes against `Decks`, so the address tree joins them by position alone. Changing the
session's deck list means rebuilding the view model with it; no key survives them drifting apart.

Each `Deck` owns one cancellation token below the app shutdown token. Its player and queue receive independent
children; its state controller and analysis listener share a third child. Dropping a deck cancels that subtree
without cancelling the app root or a sibling deck.

The first segment of a control path names a layout instance and `gui::ui::events::route` is the host's own list
of them, held against the documents by unit test, so an instance the documents mint cannot go unanswered.

### Drag, drop, focus

The library reports the drag it started on `library/tracks`, each deck module reports the pointer crossing it on
`deck-<letter>/drop`, and `ViewCache` joins them at the release; neither side addresses the other. The dragged
row and the hovered deck stay separate facts: hover only changes on a crossing, so clearing it with the drop
would strand a second drag onto the deck the pointer never left. While the drag is in flight `ui.drag.track`
names the row and the layout draws it at the pointer; the library's Deck column is a marker, not a control. A
drop focuses the deck it landed on: `deck.focused` marks it in the overview row and the keyboard's Delete
reaches it. `ViewCache` owns focus next to hover, both naming a deck by position, and the layout bounds both.

### Deck controls

- Tempo travel is `±TEMPO_RANGE` (50) percent, clamped where the deck applies it. The TEMPO block is the app's
  whole reach to the timestretch: one wheel surface, a detent anywhere on it moves tempo by `TEMPO_STEP` (1.5)
  percent, a held press drags the same way, a double click returns to zero.
- The block prints the playing BPM beside the tempo percent — the analysed BPM scaled by the tempo, an em dash
  while no analysis carries one. The deck's own bar prints the track BPM, which the tempo does not move.
- `deck.view.zoom_in` / `zoom_out` apply `kithara_ui::render::zoom_in` / `zoom_out` to the per-deck zoom the app
  owns, held to the bounds a wheel over the wave answers to; a deck no press has reached starts from
  `DEFAULT_ZOOM`.
- The stream-quality cell appears only where there is a choice: a deck with an empty `abr_variants` ladder
  answers `deck.stream.quality_hidden` and the cell leaves the row. The app supplies the rungs and owns the open
  flag per deck; a pick becomes `DeckMsg::SetQuality`, which sets the ABR mode on the deck's own
  `current_abr_handle` and mirrors it in the deck state.
- The mixer channel keeps the EQ; `GainDb` carries the knob's dB travel.

The deck module is retained-hosted, but the tempo surface stays on iced: the engine observes each decoded event first,
and an unanswered wheel event reaches the same child unchanged. The Hero Wave and five transport buttons have engine
descriptors; the tempo row deliberately does not.

`Kithara` owns one EQ mode for the whole app; every deck keeps only its own desired gains in `UiState`.
Right-clicking either knob bank opens its host-owned pointer popover in `ViewCache`, which owns no product
state, and selecting a mode replaces every deck's player layout before the shared mode is committed. Which bank
the strip draws follows from one read, `deck.eq.bands`, and the popover marks its rung through
`deck.eq.selected` scoped by the band count the row stands for, so one mode answers one question per rung and a
third topology needs no third endpoint. Switching modes remaps each deck's middle gains independently: one MID
is copied to both four-band mids, and two mids are averaged on the way back.

### Window chrome and telemetry

The window opens without system decorations, so the bar of whichever shape draws is the chrome: each bar carries
a `drag` surface and a `window` control set, executed by `Message::Window` against the window this app opened,
and a unit test holds those four addresses as the whole of it. Resizing comes back through the layout's
`resize_edges` flag, which lays eight drag zones over the window's own edges; the platform window menu and
fullscreen stay out of reach. The CPU cell reports `engine.load` — the heaviest deck's audio-engine load, not
processor time — bound twice, as a `Meter` bar and as text, off the same per-frame deck snapshots as every other
deck read rather than off the live atomics.

### Reads and host-owned view state

`ReadRoot::new` is the one place the app state is cut into domains; each node below it holds one slice and
answers only its own addresses, so no type carries the whole vocabulary, and `Walk` turns the renderer's flat
endpoint key into a walk over it. A binding scope (`@deck=a`) selects an instance rather than naming a path
segment: the node owning the instances spends it. `ViewCache` owns what the renderer borrows but the model does
not hold: converted waveform columns, formatted strings, per-deck zoom and quality-menu flag, collapsed modules,
the hovered and focused deck, and the deck layout. Four smaller views sit beside them, one owner each:
`MenuState` (which menu group is open), `Modules` (which pane the menu switched off), `WindowState` (what the
single window reports), `LibraryView` (the library's own query and scope) and `StageView` (the tempo-map window
edges and the visualisation preset, answered by `TempoNode`/`VisNode`). A view is read through `ReadRoot` and
written only by `ui::events`; nothing else holds a second copy.

`AppUi` carries the compiled document set and a `Clock`. The clock is what answers `ui.clock.seconds`, so a frame is
reproducible from the state that produced it: `update` steps it once per tick and both hosts read the same value, rather
than each sampling a wall clock of its own.

### Layout switching

Both deck layouts are compiled once at startup and the menu picks between them through `ui.layout.decks`. A
layout lays out a deck whole or not at all — body, overview row and channel strip appear together — and
`DeckLayout::decks` is the single owner of how many that is. Narrowing returns `Message::PauseHiddenDecks`,
pausing every deck the layout stops laying out, while the session keeps the deck and its queue so widening
brings it back where it was, paused. `ViewCache::set_layout` bounds the cache's two pointers into the deck list:
a hover on a dropped deck clears and a focus on one moves to the first laid-out deck, since a deck that no
longer renders reports no pointer crossing and nothing later would correct them.

The two layout documents repeat their frame because the layout schema has no include: only deck bodies, overview
rows and mixer channels may differ, and the bar and library nodes must stay identical. The switch is a menu row
per layout, addressed by the deck count it lays out, and `DeckLayout::from_decks` is the one place that count
becomes a layout. Unit tests hold both ends: the menu carries a row per layout, and pressing a row applies the
layout it names.

### Window shape

Each layout document is rooted in a `Split` measuring `Height` and declaring `(w: Fill, h: Fill)`, so the window
answers the box it is given whatever it draws. Its children arrive by band as the window grows taller: the micro
bar alone at the minimum, then the browser panel, then the overview row, then the deck row with the full bar in
place of the micro one. The deck row is itself a `Split` measuring `Width`, so the mixer and the second deck
arrive at the widths that draw them.

Every threshold is a number the compiled tree already answered, held by unit test rather than by comment: a
height band equals the summed minimum of the blocks that stand in it, and the browser's band equals the micro
bar's declared box plus the track list's own minimum. Width bands are held by the compiler
(`room::check_layout_cells`), so a band promising less room than its cell needs is a compile error. A block the
menu switches off still counts toward the bands above it, since `min_size` counts an `Optional` as standing — so
a window can draw the micro bar while the room a hidden pane would take is what keeps the decks out.

The micro bar is one module standing in one place, addressed as `micro-bar/<control>` and driving `MICRO_DECK`.
Its root `Row` measures `Width` and reveals each cell at the width it earns, with the menu, the play button, the
stretched place and the window controls standing at every width. The stretched place is two cells sharing one
band edge — a `WindowDrag` below it, the wave above — so the window stays movable at the bar's smallest and a
cell arriving beside the wave narrows it rather than taking it away.

`CompiledUi::min` is that bar's own `compiled_min`, since it is the only cell standing in the room the root
split settles on; `AppUi::window_min` takes the larger of the two layouts' and `frontend::window_settings` hands
it to `iced` as `min_size`.

## Track analysis cache

Progressive source analysis derives the coloured waveform and an optional beat grid / BPM estimate from decoded
ranges, so each deck's `StateController` owns one `AnalysisController` and one in-memory
`TrackAnalysisCache`. The GUI frontend creates one app-wide `AnalysisPersistence` actor and clones its handle
into every controller. The actor serializes a bounded stream of writes through `AssetStore`; controllers never
write analysis resources themselves. Two identity spaces are kept separate on purpose:

- **`TrackId`** (session-scoped, from `kithara-events` via the queue) — stale guard for an in-flight run and the
  "still current" check at publish. Never persisted.
- **`AnalysisTarget`** (the track's `AssetStore` plus the `ResourceKey` derived by `ResourceConfig::asset_key`)
  — cross-session cache identity. `is_same` compares key *and* store, so one key in two stores is two entries.

`plan_analysis` returns `Serve` for a memory or disk hit and `Decode` for a genuine miss. A resumable or
configuration-incomplete hit is served immediately and then refilled from its missing ranges; only a current
track without a served result clears the visible analysis. `pump` starts no second run while the controller is
active and clears the pending queue when the runner has no analyzers. `on_track_changed` puts the current track
first and preempts a different running pass. The old pass stays in `Running` until its result channel closes,
then its last checkpoint is cached and published if still current. The controller enters `Committing` and does
not start the next pass until durable persistence acknowledges that checkpoint. Intermediate checkpoints update
memory and the current deck immediately and are offered to persistence without blocking; a full queue may drop
only that intermediate write.

`pending_order` is current-track-first, then list order. A track whose source yields no `ResourceConfig` is
skipped, and so is a source whose layout rejects the derived key. The `Option<AnalysisTarget>` seam means an
unkeyable run is decoded only while its track is current and is never cached or persisted.

The memory tier is bounded by `Consts::MAX_MEM_ENTRIES` (64) in insertion order; evicted entries are still
served from disk. An analysis with neither waveform nor beat grid is memoized in neither tier. Disk reads probe
`AssetStore::resource_state` first, because opening a missing key would create it. The disk tier stores one
progressive `AnalysisFile` per track in the track's asset scope (`analysis/track.analysis`), so the artifact is
evicted, moved and deleted with the cached audio bytes. Its fixed header and completion index identify covered
chunks, while each committed generation replaces the current payload. Restore validates the analyzer
fingerprint, source rate, extent, and configured chunk duration before resuming only missing ranges.

Invalidation has two levers. The composite codec version in `kithara-analysis` must be bumped whenever its framing
or the waveform / beat-grid encodings change. Configuration changes need no bump: `analysis_fingerprint` is written
into every blob and a mismatch is a miss, so `waveform_max_buckets` and runtime beat-analysis tuning re-analyse
on their own. Because the identity is the source location and not the bytes, a file overwritten in place keeps
its entry until the version is bumped — acceptable for a library of stable files.

## Configuration document

The application is configured by a document, in two layers. `crates/kithara-app/app.yaml` is what this build ships
with: `build.rs` embeds its text verbatim and the application parses that same text at startup, so the build decides nothing
about what a field means. A `kithara.yaml` beside the executable is laid over it — `--config <path>` names one
explicitly, and the explicit path must exist where the conventional one may be absent. That second layer reconfigures a deployed
binary without a rebuild.

`app.yaml` must never name `ui:`, `draw_pool:`, or any future `#[cfg(feature = "gui")]` section: a `lib-only` build's
`Document` declares no such field, and the workspace's integration suites load this exact shipped document through
`Config::load(None, None)` from a `lib-only` build.

The pipeline is merge → expand → type, and that order is what keeps secrets out of the logs: both earlier steps work
on the untyped YAML tree, only the expanded tree is deserialized into `document::schema::Document`, and a schema
failure is reported from the *pre*-expansion tree, naming `$KITHARA_DRM_PROD_KEY` rather than the value behind it.
`Config` keeps that pre-expansion tree and prints it from `Config::dump` (`--dump-config`) and its own `Debug`; the
typed tree carries resolved secrets and is deserialize-only, so nothing serializes it.

The merge contract (`document::merge`): two mappings merge key by key, so an overlay names only what it changes;
every other pair replaces, so a sequence such as `playlist.tracks` or `net.compression` is one setting taken whole,
never appended to. An explicit `null` blanks a key's value and keeps the key — under a crate section the field then
types as `None`, the same as never naming it, so the crate default stands. An overlay with nothing in it (empty,
comments only, a bare `---`) leaves the shipped document standing, its root having named no key to blank; one whose
root is a scalar or a sequence is refused as `LoadError::Schema` naming that file.

Secrets are never inlined. A provider's `cipher_key`, and any header value, may be a `$KITHARA_...` reference; the
values live in the gitignored workspace `.env` or in the build shell, and a reference resolves from the process
environment first and the table `build.rs` baked behind it second. One that resolves nowhere — unset, or empty —
stops startup rather than degrading, and `MissingEnv` names every unresolved reference at once with its position
(`drm.providers[0].cipher_key`). The position is load-bearing: expansion runs over the merged tree, so without it an
operator cannot tell which document to fix.

`build.rs` emits that table as the crate-private `baked::baked_env`, carrying only the names this build found a value
for and wrapping each in `obfstr!()` so a shipped secret is not a plain run of bytes in `strings` output; a name it had
no value for is answered `None` and refused at startup. Builds that talk to a real key server — today the CI
`network*` lanes — set `KITHARA_DRM_REQUIRE` to any non-empty value, and an upfront pass then validates the whole
tree, failing the build with every missing name and its position.

### Crate sections

A section names the crate that owns the setting and carries that crate's own patch type, so a value is spelled once,
in the crate that defines it: `net`, `hls`, `file`, `audio`, `assets_store`, `queue`, `player`, `host`, and under
`gui`, `ui` and `draw_pool`. Which knobs a section may name is the owning crate's contract, and that crate's
`CONTEXT.md` holds the argument for each knob left out. `host` alone names the output rate: `Deck::build` hands
`Host::requested_sample_rate` to every player.

`pools` is the one section with no crate type to carry: `pool_schema!` generates a region type per consumer, so it is
composed here from `kithara-bufpool`'s `PoolConfigPatch`, one per declared pool. `net` is applied before
`--insecure`, an override that can turn verification off and never back on.

Two sections predated this shape and are gone: `network`, whose fields moved to `net` and `hls`, and `playback`,
whose `crossfade_seconds` was a second copy of `PlayerConfig::crossfade_duration`. Naming either is refused rather
than ignored — that is what makes a move a move — and `a_playback_section_is_rejected` plus two `network` tests hold
it.

`hls`, `file` and `audio` travel as patches all the way to the track: `AppConfig` carries them,
`sources::build_resource_config` sets them on `ResourceConfig`, and `kithara-play/src/resource/build.rs` applies each
onto the configuration it builds there — nothing is copied field by field, so a knob those crates add later reaches
the built stream with no edit here.

`assets_store` arrives the same way: `main` stops the store builder one step short with `into_config()`, applies the
patch onto that config, and opens the store from it. `backend` is the one value `Config::assets_store`
resolves first — an unnamed backend becomes `StorageBackend::default`, a stable root under the system temp directory,
and deliberately not `AssetStore::open`'s own fallback, a fresh directory per launch that would move the cache
every run.

`ui` and `draw_pool` are two top-level sections rather than one nested pair because `UiConfig.draw_buffers` is a
*built* `DrawBuffers`, not a patchable field, and `DrawPoolLimits` only reaches one through `DrawBuffers::try_new`.
`Config::ui` reads `draw_pool` into a `DrawPoolLimits` off the crate default, builds the `DrawBuffers` from it, then
builds `UiConfig` through `UiConfig::builder().draw_buffers(...)` and applies `ui` onto that value — read before
build, never onto an already-built `UiConfig`, and never through `UiConfig::default()`, which would build and discard
a second `PoolRegion`. It returns a `Result`: a `draw_pool:` section can name limits the generated schema refuses,
and such a document is refused with a message rather than aborting.
