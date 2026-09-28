use std::collections::BTreeSet;

use kithara::{
    analysis::Waveform,
    host::{SyncExecutionReject, SyncMode},
    ui::render::WaveBucket,
};
use num_traits::cast::{AsPrimitive, ToPrimitive};

use super::{menu::MenuState, modules::Modules, scope::deck_letter, window::WindowState};
use crate::{
    analysis::{TrackArtifacts, WaveformId},
    catalog::{Catalog, CatalogEntry, is_loaded},
    engine::{DeckSnapshot, DeckSync, EngineSnapshot, HostTempo, SyncPhase, Wish, WishStage},
    gui::view::track_subtitle,
};

mod consts {
    pub(super) const EM_DASH: char = '\u{2014}';
}

#[derive(Default, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct ViewCache {
    pub(in crate::gui) deck_marks: CatalogRowMarks,
    pub(in crate::gui) collapsed: CollapsedModules,
    pub(in crate::gui) library: LibraryView,
    pub(in crate::gui) menu: MenuState,
    pub(in crate::gui) modules: Modules,
    pub(in crate::gui) drag: Option<usize>,
    pub(in crate::gui) stage: StageView,
    pub(in crate::gui) window: WindowState,
    #[field(get, vis = "pub(in crate::gui)", copy)]
    layout: DeckLayout,

    hover_deck: Option<usize>,
    #[field(get, vis = "pub(in crate::gui)")]
    decks: Vec<DeckCache>,

    #[field(get, vis = "pub(crate)")]
    focus_deck: usize,
}

#[derive(Debug, PartialEq)]
pub(in crate::gui) struct StageView {
    pub(in crate::gui) window: (f32, f32),
    pub(in crate::gui) preset: u32,
    pub(in crate::gui) host_bpm: Option<f32>,
    /// The Host BPM field: the processed tempo, and where the target is
    /// headed while the Host has yet to process it.
    pub(in crate::gui) host_text: String,
    pub(in crate::gui) host_state: String,
}

impl Default for StageView {
    fn default() -> Self {
        Self {
            preset: 0,
            window: (0.0, 1.0),
            host_bpm: None,
            host_text: String::new(),
            host_state: String::new(),
        }
    }
}

impl StageView {
    pub(in crate::gui) const BPM_CEILING: f32 = 200.0;
    pub(in crate::gui) const BPM_FLOOR: f32 = 60.0;

    pub(in crate::gui) fn bpm_window(&self) -> (f32, f32) {
        let span = Self::BPM_CEILING - Self::BPM_FLOOR;
        (
            Self::BPM_FLOOR + self.window.0 * span,
            Self::BPM_FLOOR + self.window.1 * span,
        )
    }

    fn refresh_host(&mut self, tempo: &HostTempo) {
        self.host_bpm = tempo
            .processed
            .map(|processed| processed.beats_per_minute().as_());
        self.host_text = format_bpm(self.host_bpm, 1.0);
        self.host_state = if tempo.is_refused {
            "REFUSED".to_owned()
        } else if tempo.is_pending() {
            format!("TO {:.1}", tempo.target.beats_per_minute())
        } else {
            String::new()
        };
    }

    pub(in crate::gui) fn set_edge(&mut self, edge: WindowEdge, at: f32) {
        let at = at.clamp(0.0, 1.0);
        match edge {
            WindowEdge::Min => self.window.0 = at.min(self.window.1),
            WindowEdge::Max => self.window.1 = at.max(self.window.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gui) enum WindowEdge {
    Min,
    Max,
}

#[derive(Default)]
pub(in crate::gui) struct LibraryView {
    pub(in crate::gui) scope: LibraryScope,
    pub(in crate::gui) query: String,
}

impl LibraryView {
    pub(in crate::gui) fn catalog_index(&self, catalog: &Catalog, row: usize) -> Option<usize> {
        catalog
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.scope.holds(entry))
            .nth(row)
            .map(|(index, _)| index)
    }

    pub(in crate::gui) fn groups(&self) -> impl Iterator<Item = LibraryScope> + '_ {
        let query = self.query.trim().to_lowercase();
        LibraryScope::ALL
            .into_iter()
            .filter(move |group| query.is_empty() || group.label().to_lowercase().contains(&query))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::gui) enum LibraryScope {
    #[default]
    All,
    Local,
    Stream,
}

impl LibraryScope {
    pub(in crate::gui) const ALL: [Self; 3] = [Self::All, Self::Local, Self::Stream];

    pub(in crate::gui) fn holds(self, entry: &CatalogEntry) -> bool {
        let streamed = entry.source.contains("://") && !entry.source.starts_with("file://");
        match self {
            Self::All => true,
            Self::Local => !streamed,
            Self::Stream => streamed,
        }
    }

    pub(in crate::gui) const fn index(self) -> usize {
        match self {
            Self::All => 0,
            Self::Local => 1,
            Self::Stream => 2,
        }
    }

    pub(in crate::gui) const fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Local => "LOCAL",
            Self::Stream => "STREAM",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::gui) enum DeckLayout {
    Single,
    #[default]
    Dual,
}

impl DeckLayout {
    pub(in crate::gui) const fn decks(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Dual => 2,
        }
    }

    pub(in crate::gui) const fn from_decks(decks: usize) -> Option<Self> {
        match decks {
            1 => Some(Self::Single),
            2 => Some(Self::Dual),
            _ => None,
        }
    }

    pub(in crate::gui) const fn label(self) -> &'static str {
        match self {
            Self::Single => "1 DECK",
            Self::Dual => "2 DECKS",
        }
    }
}

#[derive(Default)]
pub(in crate::gui) struct DeckCache {
    pub(in crate::gui) view: DeckViewState,
    pub(in crate::gui) bpm: String,
    pub(in crate::gui) quality: String,
    pub(in crate::gui) remain: String,
    pub(in crate::gui) subtitle: String,
    pub(in crate::gui) sync_state: &'static str,
    pub(in crate::gui) tempo: String,
    pub(in crate::gui) wave: Vec<WaveBucket>,
    pub(in crate::gui) wave_revision: u64,
    wave_src: Option<WaveformId>,
}

#[derive(Default)]
pub(in crate::gui) struct DeckViewState {
    pub(in crate::gui) zoom: Option<f64>,
    pub(in crate::gui) eq_menu_open: bool,
    pub(in crate::gui) quality_menu: bool,
}

#[derive(Default)]
pub(in crate::gui) struct CatalogRowMarks {
    rows: Vec<String>,
}

#[derive(Default)]
pub(in crate::gui) struct CollapsedModules(BTreeSet<String>);

impl ViewCache {
    pub(in crate::gui) fn close_eq_menus(&mut self) {
        for deck in &mut self.decks {
            deck.view.eq_menu_open = false;
        }
    }

    pub(in crate::gui) fn deck_mut(&mut self, index: usize) -> Option<&mut DeckCache> {
        self.decks.get_mut(index)
    }

    pub(in crate::gui) const fn drag_target(&self) -> Option<usize> {
        if self.drag.is_some() {
            self.hover_deck
        } else {
            None
        }
    }

    pub(crate) const fn laid_out_decks(&self) -> usize {
        self.layout.decks()
    }

    pub(crate) fn refresh(&mut self, snapshot: &EngineSnapshot, catalog: &Catalog) {
        self.decks
            .resize_with(snapshot.decks.len(), Default::default);
        for (cache, deck) in self.decks.iter_mut().zip(&snapshot.decks) {
            cache.refresh(deck);
        }
        self.deck_marks.refresh(&snapshot.decks, catalog);
        self.stage.refresh_host(&snapshot.host_tempo);
        self.window.refresh(self.layout, &self.modules);
    }

    pub(in crate::gui) fn set_eq_menu_open(&mut self, index: usize, open: bool) -> Option<()> {
        self.deck_mut(index)?.view.eq_menu_open = open;
        Some(())
    }

    pub(in crate::gui) fn set_hover_deck(&mut self, deck: usize, over: bool) {
        if over {
            self.hover_deck = Some(deck);
        } else if self.hover_deck == Some(deck) {
            self.hover_deck = None;
        }
    }

    pub(in crate::gui) fn set_layout(&mut self, layout: DeckLayout) {
        self.layout = layout;
        if self.hover_deck.is_some_and(|deck| deck >= layout.decks()) {
            self.hover_deck = None;
        }
        if self.focus_deck >= layout.decks() {
            self.focus_deck = 0;
        }
    }

    pub(in crate::gui) fn take_drop(&mut self) -> Option<(usize, usize)> {
        let row = self.drag.take()?;
        let deck = self.hover_deck?;
        self.focus_deck = deck;
        Some((row, deck))
    }

    pub(in crate::gui) fn toggle_module(&mut self, module: String) {
        self.collapsed.toggle(module);
    }

    #[cfg(test)]
    pub(in crate::gui) fn with_decks(decks: usize) -> Self {
        let mut cache = Self::default();
        cache.decks.resize_with(decks, DeckCache::default);
        cache
    }
}

impl DeckCache {
    fn refresh(&mut self, deck: &DeckSnapshot) {
        (self.tempo, self.bpm) = format_tempo(deck);
        self.sync_state = format_sync(&deck.sync, deck.playing);
        self.remain = format_remain(deck);
        self.subtitle = track_subtitle(deck);
        self.quality = format_quality(deck);
        self.refresh_wave(deck.analysis.artifacts.as_ref());
    }

    fn refresh_wave(&mut self, analysis: Option<&TrackArtifacts>) {
        let wave = analysis.and_then(TrackArtifacts::waveform);
        let src = analysis.and_then(TrackArtifacts::waveform_id);
        if src == self.wave_src {
            return;
        }
        self.wave_src = src;
        self.wave_revision = self.wave_revision.wrapping_add(1);
        self.wave.clear();
        if let Some(wave) = wave {
            self.wave.extend(waveform_buckets(wave));
        }
    }
}

impl CatalogRowMarks {
    pub(in crate::gui) fn get(&self, row: usize) -> Option<&String> {
        self.rows.get(row)
    }

    fn refresh(&mut self, decks: &[DeckSnapshot], catalog: &Catalog) {
        self.rows.clear();
        self.rows.extend(
            catalog
                .entries()
                .iter()
                .map(|entry| loaded_deck_letters(entry, decks)),
        );
    }
}

impl CollapsedModules {
    pub(in crate::gui) fn contains(&self, module: &str) -> bool {
        self.0.contains(module)
    }

    fn toggle(&mut self, module: String) {
        if !self.0.remove(&module) {
            self.0.insert(module);
        }
    }
}

fn loaded_deck_letters(entry: &CatalogEntry, decks: &[DeckSnapshot]) -> String {
    decks
        .iter()
        .enumerate()
        .filter(|(_, deck)| is_loaded(&deck.tracks, entry))
        .filter_map(|(at, _)| deck_letter(at))
        .map(|letter| letter.to_ascii_uppercase())
        .collect()
}

fn format_quality(deck: &DeckSnapshot) -> String {
    let stream = &deck.stream;
    let rung = stream.current.or(stream.selected).and_then(|index| {
        stream
            .variants
            .iter()
            .find(|variant| variant.index == index)
    });
    match (stream.is_auto, rung) {
        (true, Some(rung)) => format!("AUTO\u{b7}{}", rung.label),
        (true, None) => "AUTO".to_owned(),
        (false, Some(rung)) => rung.label.clone(),
        (false, None) => String::new(),
    }
}

fn format_bpm(source: Option<f32>, speed: f32) -> String {
    source.map_or_else(
        || consts::EM_DASH.to_string(),
        |bpm| format!("{:.1}", bpm * speed),
    )
}

/// The deck's tempo and BPM texts: the manual tempo off the Host's timeline,
/// else the tempo the deck's map sounds at, against the analysed BPM.
fn format_tempo(deck: &DeckSnapshot) -> (String, String) {
    if deck.sync.is_manual() {
        return (
            format!("{:+.1}%", f32::from(deck.tempo)),
            format_bpm(deck.analysis.bpm, deck.tempo.speed()),
        );
    }
    let Some(applied) = deck
        .sync
        .reported
        .and_then(|report| report.applied_tempo)
        .map(f64::from)
    else {
        return (consts::EM_DASH.to_string(), consts::EM_DASH.to_string());
    };
    let tempo = deck.analysis.bpm.map_or_else(
        || consts::EM_DASH.to_string(),
        |analysed| format!("{:+.1}%", (applied / f64::from(analysed) - 1.0) * 100.0),
    );
    (tempo, format!("{applied:.1}"))
}

/// The word a deck names its SYNC by: a pending ask first, then the Host's
/// answer.
fn format_sync(sync: &DeckSync, playing: bool) -> &'static str {
    if sync.is_refused {
        return "REFUSED";
    }
    match sync.wish {
        Some(Wish {
            on: true,
            stage: WishStage::WaitsForBeats,
        }) => return "WAITS FOR BEATS",
        Some(Wish { on: true, .. }) if playing => return "SYNCING",
        Some(Wish { on: true, .. }) => return "WAITS FOR PLAY",
        Some(Wish { on: false, .. }) if sync.is_synced() => return "RELEASING",
        Some(Wish { on: false, .. }) | None => {}
    }
    let Some(report) = sync.reported else {
        return "";
    };
    match (report.mode, report.phase) {
        (_, SyncPhase::Rejected(reason)) => format_reject(reason),
        (SyncMode::HostSync, SyncPhase::WaitingForGrid) => "WAITS FOR BEATS",
        (SyncMode::HostSync, SyncPhase::Preparing) => "SYNCING",
        (SyncMode::HostSync, SyncPhase::Converging) => "ALIGNING",
        (SyncMode::HostSync, SyncPhase::Locked) => "LOCKED",
        (SyncMode::LocalSync, SyncPhase::WaitingForGrid | SyncPhase::Preparing) => "RELEASING",
        _ => "",
    }
}

/// Why the deck's latest alignment ended without sounding.
const fn format_reject(reason: SyncExecutionReject) -> &'static str {
    match reason {
        SyncExecutionReject::Geometry => "CANNOT STRETCH",
        SyncExecutionReject::Late => "MISSED THE BEAT",
        SyncExecutionReject::Capacity => "NO FREE VOICE",
        SyncExecutionReject::ControlBusy => "ENGINE BUSY",
        SyncExecutionReject::Cancelled => "INTERRUPTED",
        SyncExecutionReject::Media => "TRACK UNREADABLE",
        _ => "SYNC FAILED",
    }
}

fn format_remain(deck: &DeckSnapshot) -> String {
    const MINUS_SIGN: char = '\u{2212}';

    let left = (deck.duration - deck.position).max(0.0);
    let total = left.floor().to_u64().unwrap_or(0);
    format!("{MINUS_SIGN}{:02}:{:02}", total / 60, total % 60)
}

fn waveform_buckets(wave: &Waveform) -> impl Iterator<Item = WaveBucket> + '_ {
    wave.buckets().iter().map(|bucket| WaveBucket {
        low: bucket.low(),
        mid: bucket.mid(),
        high: bucket.high(),
    })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use ::kithara::{
        abr::AbrMode,
        platform::{
            CancelToken,
            sync::{Arc, Mutex},
            tokio::task,
        },
        warp::BeatsPerMinute,
    };
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        analysis::{AnalysisHandle, fixtures},
        deck::DeckId,
        engine::{DeckSettings, SyncReport},
        state::{AbrVariant, UiState, listen},
        waveform::TrackAnalysis,
    };

    #[kithara::test(native, tokio)]
    async fn a_revision_offered_to_an_entry_redraws_the_deck_waveform() {
        let cancel = CancelToken::root();
        let (host, queue) = fixtures::queue_off().await;
        let (track_id, source) = fixtures::track(&host, 1, "file:///tmp/track-1.mp3").await;
        let config = fixtures::app_config(&cancel, fixtures::memory_store());
        let mut entry = fixtures::entry(&config, queue.clone(), track_id, source);
        let state = Arc::new(Mutex::new(UiState::new(&queue)));
        let (analysis, mut requests) = AnalysisHandle::channel();
        task::spawn(listen(
            queue.clone(),
            Arc::clone(&state),
            cancel.clone(),
            queue.subscribe(),
            analysis,
        ));
        let (asked, reply) = fixtures::next_subscribe(&mut requests).await;
        assert_eq!(asked, track_id);
        assert!(reply.send(entry.subscribe()).is_ok());
        let mut cache = DeckCache::default();
        cache.refresh_wave(state.lock().analysis.as_ref());
        let before = cache.wave_revision;

        let published = fixtures::snapshot("track".into(), 1, 1_000, fixtures::fingerprint(), None);
        assert!(entry.offer(fixtures::progress(published)));
        fixtures::wait_for_revision(&state, 1).await;

        cache.refresh_wave(state.lock().analysis.as_ref());
        assert_ne!(
            cache.wave_revision, before,
            "the deck draws the revision the entry published"
        );
        cancel.cancel();
        host.close().await;
    }

    fn wave_of(height: u8) -> Waveform {
        let blob = [
            1, 0, 0, 0, 0, 0, 0, height, 0, 0, 0, height, 0, 0, 0, height,
        ];
        Waveform::try_from(blob.as_slice()).expect("hand-built blob is valid")
    }

    fn revision(revision: u64, wave: Waveform) -> TrackArtifacts {
        TrackAnalysis::builder()
            .token("fixture".into())
            .revision(revision)
            .source_sample_rate(NonZeroU32::new(44_100).expect("fixture rate is non-zero"))
            .waveform(wave)
            .build()
            .into()
    }

    #[kithara::test]
    fn the_deck_names_the_run_of_buckets_it_just_wrote() {
        let mut cache = DeckCache::default();
        let quiet = revision(1, wave_of(62));

        cache.refresh_wave(Some(&quiet));
        let named = cache.wave_revision;

        cache.refresh_wave(Some(&quiet));
        assert_eq!(cache.wave_revision, named, "nothing was written");

        cache.refresh_wave(Some(&revision(2, wave_of(63))));
        assert_ne!(
            cache.wave_revision, named,
            "a new run of buckets was written"
        );
    }

    #[kithara::test]
    fn the_deck_cache_follows_the_revision_not_the_bucket_address() {
        let mut cache = DeckCache::default();
        let wave = wave_of(62);

        cache.refresh_wave(Some(&revision(1, wave.clone())));
        let named = cache.wave_revision;

        cache.refresh_wave(Some(&revision(2, wave)));
        assert_ne!(
            cache.wave_revision, named,
            "the same bucket store under a new revision is a new run"
        );
    }

    #[kithara::test]
    fn dropping_the_track_names_the_empty_run() {
        let mut cache = DeckCache::default();
        cache.refresh_wave(Some(&revision(1, wave_of(63))));
        let named = cache.wave_revision;

        cache.refresh_wave(None);

        assert!(cache.wave.is_empty());
        assert_ne!(cache.wave_revision, named, "the empty run is a run too");
    }

    fn shown(ui: &UiState, settings: &DeckSettings) -> DeckSnapshot {
        DeckSnapshot::new(DeckId(0), ui, settings, DeckSync::default())
    }

    fn asked(on: bool) -> DeckSync {
        let mut sync = DeckSync::default();
        sync.request(on);
        sync
    }

    fn answered(mode: SyncMode, phase: SyncPhase, applied: Option<f64>) -> DeckSync {
        let mut sync = DeckSync::default();
        sync.reported = Some(SyncReport {
            mode,
            phase,
            applied_tempo: applied
                .map(|bpm| BeatsPerMinute::try_from(bpm).expect("fixture tempo is positive")),
        });
        sync
    }

    fn waiting_for_beats() -> DeckSync {
        let mut sync = DeckSync::default();
        sync.wish = Some(Wish {
            on: true,
            stage: WishStage::WaitsForBeats,
        });
        sync
    }

    fn refused() -> DeckSync {
        let mut sync = DeckSync::default();
        sync.is_refused = true;
        sync
    }

    #[kithara::test]
    #[case::no_answer_yet(DeckSync::default(), false, "")]
    #[case::asked_on_paused(asked(true), false, "WAITS FOR PLAY")]
    #[case::asked_on_playing(asked(true), true, "SYNCING")]
    #[case::asked_off_while_manual(asked(false), true, "")]
    #[case::refused(refused(), true, "REFUSED")]
    #[case::asked_on_without_beats(waiting_for_beats(), true, "WAITS FOR BEATS")]
    #[case::asked_on_without_beats_paused(waiting_for_beats(), false, "WAITS FOR BEATS")]
    #[case::waits_for_beats(
        answered(SyncMode::HostSync, SyncPhase::WaitingForGrid, None),
        true,
        "WAITS FOR BEATS"
    )]
    #[case::preparing(
        answered(SyncMode::HostSync, SyncPhase::Preparing, None),
        true,
        "SYNCING"
    )]
    #[case::converging(
        answered(SyncMode::HostSync, SyncPhase::Converging, Some(124.0)),
        true,
        "ALIGNING"
    )]
    #[case::locked(
        answered(SyncMode::HostSync, SyncPhase::Locked, Some(124.0)),
        true,
        "LOCKED"
    )]
    #[case::rejected(
        answered(SyncMode::Off, SyncPhase::Rejected(SyncExecutionReject::Late), None),
        true,
        "MISSED THE BEAT"
    )]
    #[case::released(
        answered(SyncMode::LocalSync, SyncPhase::Preparing, Some(124.0)),
        true,
        "RELEASING"
    )]
    #[case::on_its_own_timeline(
        answered(SyncMode::LocalSync, SyncPhase::Locked, Some(124.0)),
        true,
        ""
    )]
    #[case::manual(answered(SyncMode::Off, SyncPhase::Off, None), true, "")]
    fn the_deck_names_its_sync_by_the_pending_ask_then_the_host_answer(
        #[case] sync: DeckSync,
        #[case] playing: bool,
        #[case] word: &str,
    ) {
        assert_eq!(format_sync(&sync, playing), word);
    }

    #[kithara::test]
    fn a_pending_release_is_named_only_on_a_deck_the_host_holds() {
        let mut sync = answered(SyncMode::HostSync, SyncPhase::Locked, Some(124.0));
        sync.request(false);

        assert_eq!(format_sync(&sync, true), "RELEASING");
    }

    #[kithara::test]
    #[case::manual(DeckSync::default(), Some(120.0), "+0.0%", "120.0")]
    #[case::manual_under_a_map_the_host_dropped(
        answered(SyncMode::Off, SyncPhase::Off, Some(124.0)),
        Some(120.0),
        "+0.0%",
        "120.0"
    )]
    #[case::mapped(
        answered(SyncMode::HostSync, SyncPhase::Locked, Some(124.0)),
        Some(120.0),
        "+3.3%",
        "124.0"
    )]
    #[case::mapped_without_analysis(
        answered(SyncMode::HostSync, SyncPhase::Locked, Some(124.0)),
        None,
        "\u{2014}",
        "124.0"
    )]
    #[case::no_map_sounds_yet(
        answered(SyncMode::HostSync, SyncPhase::Preparing, None),
        Some(120.0),
        "\u{2014}",
        "\u{2014}"
    )]
    fn a_deck_on_a_timeline_shows_the_tempo_its_map_sounds_at(
        #[case] sync: DeckSync,
        #[case] analysed: Option<f32>,
        #[case] tempo: &str,
        #[case] bpm: &str,
    ) {
        let mut deck = shown(&UiState::empty(), &DeckSettings::new(0));
        deck.analysis.bpm = analysed;
        deck.sync = sync;

        assert_eq!(format_tempo(&deck), (tempo.to_owned(), bpm.to_owned()));
    }

    fn ladder() -> Vec<AbrVariant> {
        vec![
            AbrVariant {
                index: 0,
                label: "128k".to_owned(),
                detail: "128 kbps \u{b7} AAC".to_owned(),
            },
            AbrVariant {
                index: 1,
                label: "320k".to_owned(),
                detail: "320 kbps \u{b7} AAC".to_owned(),
            },
        ]
    }

    #[kithara::test]
    fn the_cell_marks_the_rung_the_ladder_chose_and_names_the_one_the_user_pinned() {
        let mut ui = UiState::empty();
        let settings = DeckSettings::new(0);
        ui.abr_variants = ladder();
        ui.current_variant = Some(1);

        assert_eq!(format_quality(&shown(&ui, &settings)), "AUTO\u{b7}320k");

        ui.abr_mode = Some(AbrMode::manual(0));
        ui.current_variant = Some(0);
        assert_eq!(format_quality(&shown(&ui, &settings)), "128k");
    }

    #[kithara::test]
    fn a_stream_with_no_rung_yet_still_reports_its_mode() {
        let ui = UiState::empty();

        assert_eq!(format_quality(&shown(&ui, &DeckSettings::new(0))), "AUTO");
    }

    #[kithara::test]
    fn a_drop_ends_the_drag_and_keeps_the_hover() {
        let mut cache = ViewCache::default();
        cache.set_hover_deck(1, true);
        cache.drag = Some(4);

        assert_eq!(cache.drag_target(), Some(1));
        assert_eq!(cache.take_drop(), Some((4, 1)));
        assert_eq!(cache.drag_target(), None, "the drag is over");

        cache.drag = Some(7);
        assert_eq!(cache.take_drop(), Some((7, 1)));
    }

    #[kithara::test]
    fn a_drop_outside_every_deck_lands_nowhere() {
        let mut cache = ViewCache::default();
        cache.set_hover_deck(0, true);
        cache.set_hover_deck(1, false);
        cache.drag = Some(2);

        assert_eq!(cache.hover_deck, Some(0), "another deck's exit is not ours");
        cache.set_hover_deck(0, false);
        assert_eq!(cache.take_drop(), None);
        assert_eq!(cache.drag, None, "a drop always ends the drag");
    }

    #[kithara::test]
    fn a_drop_focuses_the_deck_it_landed_on() {
        let mut cache = ViewCache::default();
        assert_eq!(cache.focus_deck(), 0);

        cache.set_hover_deck(1, true);
        cache.drag = Some(2);
        assert_eq!(cache.take_drop(), Some((2, 1)));
        assert_eq!(cache.focus_deck(), 1);

        cache.set_hover_deck(1, false);
        cache.drag = Some(5);
        assert_eq!(cache.take_drop(), None);
        assert_eq!(cache.focus_deck(), 1, "a drop on nothing focuses nothing");
    }

    #[kithara::test]
    fn a_layout_answers_to_the_deck_count_the_menu_names() {
        for layout in [DeckLayout::Single, DeckLayout::Dual] {
            assert_eq!(DeckLayout::from_decks(layout.decks()), Some(layout));
        }
        assert_eq!(DeckLayout::from_decks(3), None);
    }
}
