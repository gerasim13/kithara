//! A synthetic crate: one case per construct that splits execution into two
//! alike chains, and the two chain families each case must pair.

use super::{
    body::DecisionKind,
    chain::{Origin, Split},
};

/// What the report must hold for a case.
pub(super) enum Expect {
    /// Some row pairs the two families.
    Paired,
    /// A row of this origin and split pairs them.
    Row(Origin, Split),
    /// No row pairs them.
    Absent,
}

pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) expect: Expect,
    pub(super) source: String,
    /// Name prefixes of the functions on each side.
    pub(super) families: [&'static str; 2],
}

fn case(name: &'static str, families: [&'static str; 2], expect: Expect, parts: &[&str]) -> Case {
    Case {
        name,
        families,
        expect,
        source: parts.concat(),
    }
}

/// `n` alike methods `prefix_0 -> prefix_1 -> ..`.
fn chain(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|index| {
            let next = if index + 1 < n {
                format!("self.{prefix}_{}(scaled)", index + 1)
            } else {
                "self.items.first().copied()".to_owned()
            };
            format!(
                "
    fn {prefix}_{index}(&mut self, key: u64) -> Option<u64> {{
        let scaled = key.checked_mul({})?;
        if self.items.contains(&scaled) {{
            {next}
        }} else {{
            self.items.push(scaled);
            self.items.last().copied()
        }}
    }}",
                index + 2
            )
        })
        .collect()
}

/// `n` alike free functions `prefix_0 -> prefix_1 -> ..`.
fn free_chain(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|index| {
            let next = if index + 1 < n {
                format!("{prefix}_{}(scaled)", index + 1)
            } else {
                "Some(scaled)".to_owned()
            };
            format!(
                "
fn {prefix}_{index}(key: u64) -> Option<u64> {{
    let scaled = key.checked_mul({})?;
    if scaled % 5 == 0 {{
        {next}
    }} else {{
        let doubled = scaled.checked_add(scaled)?;
        Some(doubled)
    }}
}}",
                index + 2
            )
        })
        .collect()
}

fn strukt(name: &str, extra: &str) -> String {
    format!(
        "pub struct {name} {{\n    items: Vec<u64>,\n    spare: Vec<u64>,\n    limit: usize,{extra}\n}}\n"
    )
}

const fn decision(kind: DecisionKind) -> Expect {
    Expect::Row(Origin::Decision, Split::Decision(kind))
}

/// Every construct of the corpus, the entries it lacked at first, and the
/// alternatives that must not pair.
pub(super) fn cases() -> Vec<Case> {
    vec![
        user_example(),
        match_result(),
        guard(),
        let_else(),
        or_else(),
        state_machine(),
        strategy(),
        select(),
        retry_loop(),
        entry_points(),
        field_backends(),
        sequential(),
        deep_parallel(),
        try_propagation(),
        callback(),
        generic_strategy(),
        channel_command(),
        short_circuit(),
        else_if_ladder(),
        factory(),
        drop_guard(),
        payload_binding(),
        tuple_match(),
        let_else_binding(),
        locked_field(),
        imported_free(),
        entry_dissimilar_roots(),
        inline_arms(),
        fn_table(),
        neg_cfg(),
        neg_small(),
        neg_diff(),
    ]
}

fn user_example() -> Case {
    case(
        "user_example",
        ["good_path", "bad_path"],
        Expect::Paired,
        &[
            &strukt("Worker", ""),
            r"
impl Worker {
    pub fn some_work(&mut self, key: u64) -> Option<u64> {
        if self.something_terrible_happened(key) {
            self.bad_path(key)
        } else {
            self.good_path(key)
        }
    }
    fn something_terrible_happened(&self, key: u64) -> bool {
        self.items.len() > self.limit && key % 7 == 0
    }
    fn good_path(&mut self, key: u64) -> Option<u64> {
        let scaled = key.checked_mul(3)?;
        if self.foo(scaled) { self.bar(scaled) } else { self.baz(scaled) }
    }
    fn bad_path(&mut self, key: u64) -> Option<u64> {
        let scaled = key.checked_mul(3)?;
        if self.foo(scaled) { self.bar(scaled) } else { self.daz(scaled) }
    }
    fn daz(&mut self, key: u64) -> Option<u64> {
        let scaled = key.checked_mul(3)?;
        if self.foo(scaled) { self.baz(scaled) } else { self.zaz(scaled) }
    }
    fn foo(&self, key: u64) -> bool { self.items.iter().any(|item| *item == key) }
    fn bar(&mut self, key: u64) -> Option<u64> { self.items.push(key); self.items.last().copied() }
    fn baz(&mut self, key: u64) -> Option<u64> { self.items.retain(|item| *item != key); self.items.first().copied() }
    fn zaz(&mut self, key: u64) -> Option<u64> { self.items.clear(); self.items.push(key.wrapping_add(1)); self.items.first().copied() }
}
",
        ],
    )
}

fn match_result() -> Case {
    let close = "\n}\n";
    case(
        "match_result",
        ["primary", "secondary"],
        Expect::Paired,
        &[
            &strukt("MatchResult", ""),
            r"
impl MatchResult {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        match self.checked(key) {
            Ok(value) => self.primary_0(value),
            Err(_) => self.secondary_0(key),
        }
    }
    fn checked(&self, key: u64) -> Result<u64, ()> {
        if self.items.len() < self.limit { Ok(key) } else { Err(()) }
    }",
            &chain("primary", 3),
            &chain("secondary", 3),
            close,
        ],
    )
}

fn guard() -> Case {
    let close = "\n}\n";
    case(
        "guard",
        ["fast", "slow"],
        Expect::Paired,
        &[
            &strukt("Guard", ""),
            r"
impl Guard {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        if self.items.len() > self.limit {
            return self.slow_0(key);
        }
        self.fast_0(key)
    }",
            &chain("fast", 3),
            &chain("slow", 3),
            close,
        ],
    )
}

fn let_else() -> Case {
    let close = "\n}\n";
    case(
        "let_else",
        ["reuse", "rebuild"],
        Expect::Paired,
        &[
            &strukt("LetElse", ""),
            r"
impl LetElse {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        let Some(cached) = self.spare.iter().find(|item| **item == key).copied() else {
            return self.rebuild_0(key);
        };
        self.reuse_0(cached)
    }",
            &chain("reuse", 3),
            &chain("rebuild", 3),
            close,
        ],
    )
}

fn or_else() -> Case {
    let close = "\n}\n";
    case(
        "or_else",
        ["read_primary", "read_backup"],
        Expect::Paired,
        &[
            &strukt("OrElse", ""),
            r"
impl OrElse {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        self.read_primary_0(key).or_else(|| self.read_backup_0(key))
    }",
            &chain("read_primary", 3),
            &chain("read_backup", 3),
            close,
        ],
    )
}

fn state_machine() -> Case {
    let close = "\n}\n";
    case(
        "state_machine",
        ["play", "recover"],
        Expect::Paired,
        &[
            r"
pub enum Phase {
    Playing(u64),
    Recovering(u64),
}
",
            &strukt("Machine", "\n    phase: Phase,"),
            r"
impl Machine {
    pub fn on_error(&mut self, key: u64) {
        self.phase = Phase::Recovering(key);
    }
    pub fn tick(&mut self) -> Option<u64> {
        match self.phase {
            Phase::Playing(key) => self.play_0(key),
            Phase::Recovering(key) => self.recover_0(key),
        }
    }",
            &chain("play", 3),
            &chain("recover", 3),
            close,
        ],
    )
}

fn strategy() -> Case {
    case(
        "strategy",
        ["fastf", "slowf"],
        Expect::Paired,
        &[
            r"
pub trait Fetch {
    fn fetch(&mut self, key: u64) -> Option<u64>;
}
",
            &strukt("FastFetch", ""),
            &strukt("SlowFetch", ""),
            r"
impl Fetch for FastFetch {
    fn fetch(&mut self, key: u64) -> Option<u64> { self.fastf_0(key) }
}
impl FastFetch {",
            &chain("fastf", 3),
            r"
}
impl Fetch for SlowFetch {
    fn fetch(&mut self, key: u64) -> Option<u64> { self.slowf_0(key) }
}
impl SlowFetch {",
            &chain("slowf", 3),
            r"
}
pub struct Picker {
    fast: FastFetch,
    slow: SlowFetch,
    broken: bool,
}
impl Picker {
    pub fn fetch(&mut self, key: u64) -> Option<u64> {
        let fetcher: &mut dyn Fetch = if self.broken { &mut self.slow } else { &mut self.fast };
        fetcher.fetch(key)
    }
}
",
        ],
    )
}

fn select() -> Case {
    let close = "\n}\n";
    case(
        "select",
        ["net", "disk"],
        Expect::Paired,
        &[
            &strukt("Selector", ""),
            r"
impl Selector {
    pub async fn load(&mut self, key: u64) -> Option<u64> {
        tokio::select! {
            value = self.net_0(key) => value,
            value = self.disk_0(key) => value,
        }
    }",
            &chain("net", 3),
            &chain("disk", 3),
            close,
        ],
    )
}

fn retry_loop() -> Case {
    let close = "\n}\n";
    case(
        "retry_loop",
        ["attempt", "last_resort"],
        Expect::Paired,
        &[
            &strukt("Retry", ""),
            r"
impl Retry {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        for attempt in 0..3 {
            match self.attempt_0(key) {
                Some(value) => return Some(value),
                None if attempt == 2 => return self.last_resort_0(key),
                None => continue,
            }
        }
        None
    }",
            &chain("attempt", 3),
            &chain("last_resort", 3),
            close,
        ],
    )
}

fn entry_points() -> Case {
    let close = "\n}\n";
    case(
        "entry_points",
        ["file", "stream"],
        Expect::Paired,
        &[
            &strukt("Entry", ""),
            r"
impl Entry {
    pub fn load_file(&mut self, key: u64) -> Option<u64> { self.file_0(key) }
    pub fn load_stream(&mut self, key: u64) -> Option<u64> { self.stream_0(key) }",
            &chain("file", 3),
            &chain("stream", 3),
            close,
        ],
    )
}

fn field_backends() -> Case {
    case(
        "field_backends",
        ["netb", "diskb"],
        Expect::Paired,
        &[
            &strukt("NetBackend", ""),
            &strukt("DiskBackend", ""),
            "\nimpl NetBackend {",
            &chain("netb", 3),
            "\n}\nimpl DiskBackend {",
            &chain("diskb", 3),
            r"
}
pub struct Router {
    net: NetBackend,
    disk: DiskBackend,
    offline: bool,
}
impl Router {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        if self.offline { self.disk.diskb_0(key) } else { self.net.netb_0(key) }
    }
}
",
        ],
    )
}

fn sequential() -> Case {
    let close = "\n}\n";
    case(
        "sequential",
        ["left", "right"],
        Expect::Paired,
        &[
            &strukt("Stereo", ""),
            r"
impl Stereo {
    pub fn process(&mut self, key: u64) -> Option<u64> {
        let left = self.left_0(key)?;
        let right = self.right_0(key)?;
        Some(left.max(right))
    }",
            &chain("left", 2),
            &chain("right", 2),
            close,
        ],
    )
}

fn deep_parallel() -> Case {
    let close = "\n}\n";
    case(
        "deep_parallel",
        ["simple", "guarded"],
        Expect::Paired,
        &[
            &strukt("Deep", ""),
            r"
impl Deep {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        if self.limit == 0 { self.enter_simple(key) } else { self.enter_guarded(key) }
    }
    fn enter_simple(&mut self, key: u64) -> Option<u64> { self.simple_0(key) }
    fn enter_guarded(&mut self, key: u64) -> Option<u64> {
        if self.items.len() > self.limit {
            self.items.clear();
            self.spare.clear();
            self.spare.push(key);
        }
        self.guarded_0(key)
    }",
            &chain("simple", 3),
            &chain("guarded", 3),
            close,
        ],
    )
}

fn try_propagation() -> Case {
    let close = "\n}\n";
    case(
        "try_propagation",
        ["normal", "degraded"],
        Expect::Paired,
        &[
            &strukt("Propagate", ""),
            r"
impl Propagate {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        if let Err(()) = self.validate(key) {
            return self.degraded_0(key);
        }
        self.normal_0(key)
    }
    fn validate(&self, key: u64) -> Result<(), ()> {
        let checked = key.checked_add(1).ok_or(())?;
        if checked as usize > self.limit { Err(()) } else { Ok(()) }
    }",
            &chain("normal", 3),
            &chain("degraded", 3),
            close,
        ],
    )
}

fn callback() -> Case {
    let close = "\n}\n";
    case(
        "callback",
        ["cb_fast", "cb_slow"],
        Expect::Paired,
        &[
            &strukt("Callback", ""),
            r"
impl Callback {
    pub fn load(&mut self, key: u64, fallback: bool) -> Option<u64> {
        let run: fn(&mut Self, u64) -> Option<u64> = if fallback { Self::cb_slow_0 } else { Self::cb_fast_0 };
        run(self, key)
    }",
            &chain("cb_fast", 3),
            &chain("cb_slow", 3),
            close,
        ],
    )
}

fn generic_strategy() -> Case {
    case(
        "generic_strategy",
        ["gfile", "gnet"],
        Expect::Paired,
        &[
            r"
pub trait Source {
    fn pull(&mut self, key: u64) -> Option<u64>;
}
",
            &strukt("FileSource", ""),
            &strukt("NetSource", ""),
            r"
impl Source for FileSource {
    fn pull(&mut self, key: u64) -> Option<u64> { self.gfile_0(key) }
}
impl FileSource {",
            &chain("gfile", 3),
            r"
}
impl Source for NetSource {
    fn pull(&mut self, key: u64) -> Option<u64> { self.gnet_0(key) }
}
impl NetSource {",
            &chain("gnet", 3),
            r"
}
pub struct Player {
    file: FileSource,
    net: NetSource,
    remote: bool,
}
fn drive<T: Source>(source: &mut T, key: u64) -> Option<u64> {
    source.pull(key)
}
impl Player {
    pub fn play(&mut self, key: u64) -> Option<u64> {
        if self.remote { drive(&mut self.net, key) } else { drive(&mut self.file, key) }
    }
}
",
        ],
    )
}

fn channel_command() -> Case {
    let close = "\n}\n";
    case(
        "channel_command",
        ["cfast", "cslow"],
        Expect::Paired,
        &[
            r"
pub enum Cmd {
    Fast(u64),
    Slow(u64),
}
pub struct Sender {
    queue: Vec<Cmd>,
    degraded: bool,
}
impl Sender {
    pub fn submit(&mut self, key: u64) {
        if self.degraded { self.queue.push(Cmd::Slow(key)); } else { self.queue.push(Cmd::Fast(key)); }
    }
}
",
            &strukt("CmdWorker", ""),
            r"
impl CmdWorker {
    pub fn handle(&mut self, cmd: Cmd) -> Option<u64> {
        match cmd {
            Cmd::Fast(key) => self.cfast_0(key),
            Cmd::Slow(key) => self.cslow_0(key),
        }
    }",
            &chain("cfast", 3),
            &chain("cslow", 3),
            close,
        ],
    )
}

fn short_circuit() -> Case {
    let close = "\n}\n";
    case(
        "short_circuit",
        ["quick", "thorough"],
        decision(DecisionKind::ShortCircuit),
        &[
            &strukt("Short", ""),
            r"
impl Short {
    pub fn load(&mut self, key: u64) -> bool {
        self.quick_0(key).is_some() || self.thorough_0(key).is_some()
    }",
            &chain("quick", 3),
            &chain("thorough", 3),
            close,
        ],
    )
}

fn else_if_ladder() -> Case {
    let close = "\n}\n";
    case(
        "else_if_ladder",
        ["cached", "fetched"],
        decision(DecisionKind::If),
        &[
            &strukt("Ladder", ""),
            r#"
impl Ladder {
    pub fn load(&mut self, key: u64) -> Option<u64> {
        if key % 3 == 0 {
            self.cached_0(key)
        } else if key % 3 == 1 {
            self.fetched_0(key)
        } else {
            self.render_report(key)
        }
    }
    fn render_report(&self, key: u64) -> Option<u64> {
        let lines: Vec<String> = self.spare.iter().map(|value| format!("{value}:{key}")).collect();
        tracing::info!(count = lines.len(), "report");
        lines.first().map(|line| line.len() as u64)
    }"#,
            &chain("cached", 3),
            &chain("fetched", 3),
            close,
        ],
    )
}

fn factory() -> Case {
    case(
        "factory",
        ["localp", "remotep"],
        Expect::Row(Origin::Dyn, Split::Dyn),
        &[
            r"
pub trait Source {
    fn pull(&mut self, key: u64) -> Option<u64>;
}
",
            &strukt("LocalSource", ""),
            &strukt("RemoteSource", ""),
            r"
impl LocalSource {
    pub fn new() -> Self { Self { items: Vec::new(), spare: Vec::new(), limit: 4 } }",
            &chain("localp", 3),
            r"
}
impl Source for LocalSource {
    fn pull(&mut self, key: u64) -> Option<u64> { self.localp_0(key) }
}
impl RemoteSource {
    pub fn new() -> Self { Self { items: Vec::new(), spare: Vec::new(), limit: 8 } }",
            &chain("remotep", 3),
            r"
}
impl Source for RemoteSource {
    fn pull(&mut self, key: u64) -> Option<u64> { self.remotep_0(key) }
}
pub fn open(offline: bool) -> Box<dyn Source> {
    if offline { Box::new(LocalSource::new()) } else { Box::new(RemoteSource::new()) }
}
pub struct Session {
    source: Box<dyn Source>,
}
impl Session {
    pub fn start(offline: bool) -> Self { Self { source: open(offline) } }
    pub fn step(&mut self, key: u64) -> Option<u64> { self.source.pull(key) }
}
",
        ],
    )
}

fn drop_guard() -> Case {
    let close = "\n}\n";
    case(
        "drop_guard",
        ["taskg", "streamg"],
        decision(DecisionKind::If),
        &[
            &strukt("Guarded", ""),
            r"
pub struct TaskGuard<'a> { owner: &'a mut Guarded }
pub struct StreamGuard<'a> { owner: &'a mut Guarded }
impl Drop for TaskGuard<'_> {
    fn drop(&mut self) { let _ = self.owner.taskg_0(1); }
}
impl Drop for StreamGuard<'_> {
    fn drop(&mut self) { let _ = self.owner.streamg_0(1); }
}
impl Guarded {
    pub fn run(&mut self, streaming: bool) {
        if streaming {
            let _guard = StreamGuard { owner: self };
        } else {
            let _guard = TaskGuard { owner: self };
        }
    }",
            &chain("taskg", 3),
            &chain("streamg", 3),
            close,
        ],
    )
}

fn payload_binding() -> Case {
    case(
        "payload_binding",
        ["leafc", "groupc"],
        decision(DecisionKind::Match),
        &[
            &strukt("LeafNode", ""),
            &strukt("GroupNode", ""),
            "\nimpl LeafNode {",
            &chain("leafc", 3),
            "\n}\nimpl GroupNode {",
            &chain("groupc", 3),
            r"
}
pub enum Layout {
    Leaf(LeafNode),
    Group { node: GroupNode },
}
pub struct Tree { layout: Layout }
impl Tree {
    pub fn ink(&mut self, key: u64) -> Option<u64> {
        match &mut self.layout {
            Layout::Leaf(leaf) => leaf.leafc_0(key),
            Layout::Group { node } => node.groupc_0(key),
        }
    }
}
",
        ],
    )
}

fn tuple_match() -> Case {
    case(
        "tuple_match",
        ["paira", "pairb"],
        decision(DecisionKind::Match),
        &[
            &strukt("PairA", ""),
            &strukt("PairB", ""),
            "\nimpl PairA {",
            &chain("paira", 3),
            "\n}\nimpl PairB {",
            &chain("pairb", 3),
            r"
}
pub struct Pair { a: Option<PairA>, b: Option<PairB> }
impl Pair {
    pub fn go(&mut self, key: u64) -> Option<u64> {
        match (self.a.as_mut(), self.b.as_mut()) {
            (Some(first), None) => first.paira_0(key),
            (None, Some(second)) => second.pairb_0(key),
            _ => None,
        }
    }
}
",
        ],
    )
}

fn let_else_binding() -> Case {
    case(
        "let_else_binding",
        ["primb", "backb"],
        decision(DecisionKind::LetElse),
        &[
            &strukt("Primary", ""),
            &strukt("Backup", ""),
            "\nimpl Primary {",
            &chain("primb", 3),
            "\n}\nimpl Backup {",
            &chain("backb", 3),
            r"
}
pub struct Holder { primary: Option<Primary>, backup: Backup }
impl Holder {
    pub fn go(&mut self, key: u64) -> Option<u64> {
        let Some(primary) = &mut self.primary else {
            let backup = &mut self.backup;
            return backup.backb_0(key);
        };
        primary.primb_0(key)
    }
}
",
        ],
    )
}

fn locked_field() -> Case {
    case(
        "locked_field",
        ["lockfast", "lockslow"],
        decision(DecisionKind::If),
        &[
            &strukt("Inner", ""),
            r"
pub struct Mutex<T> { value: T }
pub struct MutexGuard<'a, T> { value: &'a mut T }
impl<T> Mutex<T> {
    pub fn lock(&mut self) -> MutexGuard<'_, T> { MutexGuard { value: &mut self.value } }
}
impl Inner {",
            &chain("lockfast", 3),
            &chain("lockslow", 3),
            r"
}
pub struct Shared { inner: Mutex<Inner>, degraded: bool }
impl Shared {
    pub fn go(&mut self, key: u64) -> Option<u64> {
        if self.degraded { self.inner.lock().lockslow_0(key) } else { self.inner.lock().lockfast_0(key) }
    }
}
",
        ],
    )
}

fn imported_free() -> Case {
    case(
        "imported_free",
        ["fastfree", "slowfree"],
        decision(DecisionKind::If),
        &[
            r"
mod fast {
    pub fn run(key: u64) -> Option<u64> { super::fastfree_0(key) }
}
mod slow {
    pub fn run(key: u64) -> Option<u64> { super::slowfree_0(key) }
}
mod other {
    pub fn run(key: u64) -> Option<u64> { Some(key) }
}
mod more {
    pub fn run(key: u64) -> Option<u64> { Some(key + 1) }
}
use fast::run as fast_run;
pub fn go(key: u64, slow_path: bool) -> Option<u64> {
    if slow_path { slow::run(key) } else { fast_run(key) }
}
",
            &free_chain("fastfree", 3),
            &free_chain("slowfree", 3),
            "\n",
        ],
    )
}

fn entry_dissimilar_roots() -> Case {
    let close = "\n}\n";
    case(
        "entry_dissimilar_roots",
        ["filer", "streamer"],
        Expect::Row(Origin::Region, Split::Entry),
        &[
            &strukt("Api", ""),
            r#"
impl Api {
    pub fn load_file(&mut self, key: u64) -> Option<u64> { self.filer_0(key) }
    pub fn load_stream(&mut self, key: u64, retries: u32, label: &str) -> Option<u64> {
        tracing::debug!(retries, label, "stream load");
        let attempts = usize::try_from(retries).ok()?;
        self.limit = self.limit.max(attempts);
        self.streamer_0(key)
    }"#,
            &chain("filer", 5),
            &chain("streamer", 5),
            close,
        ],
    )
}

fn inline_arms() -> Case {
    case(
        "inline_arms",
        ["inl_fast", "inl_slow"],
        decision(DecisionKind::If),
        &[
            &strukt("Inline", ""),
            r"
impl Inline {
    pub fn go(&mut self, key: u64, fast: bool) -> Option<u64> {
        if fast {
            let scaled = key.checked_mul(3)?;
            if self.items.contains(&scaled) { return self.items.first().copied(); }
            self.items.push(scaled);
            self.items.retain(|item| *item < 1_000);
            self.items.sort_unstable();
            self.items.dedup();
            let total: u64 = self.items.iter().sum();
            self.spare.push(total);
            self.inl_fast_a(total)?;
            self.inl_fast_b(total)
        } else {
            let scaled = key.checked_mul(5)?;
            if self.items.contains(&scaled) { return self.items.last().copied(); }
            self.items.push(scaled);
            self.items.retain(|item| *item < 2_000);
            self.items.sort_unstable();
            self.items.dedup();
            let total: u64 = self.items.iter().sum();
            self.spare.push(total);
            self.inl_slow_a(total)?;
            self.inl_slow_b(total)
        }
    }
    fn inl_fast_a(&mut self, key: u64) -> Option<u64> { self.items.push(key); self.items.last().copied() }
    fn inl_fast_b(&mut self, key: u64) -> Option<u64> { self.spare.push(key); self.spare.last().copied() }
    fn inl_slow_a(&mut self, key: u64) -> Option<u64> { self.items.insert(0, key); self.items.first().copied() }
    fn inl_slow_b(&mut self, key: u64) -> Option<u64> { self.spare.insert(0, key); self.spare.first().copied() }
}
",
        ],
    )
}

fn fn_table() -> Case {
    let close = "\n}\n";
    case(
        "fn_table",
        ["tablea", "tableb"],
        decision(DecisionKind::Table),
        &[
            &strukt("Table", ""),
            r"
impl Table {
    pub fn go(&mut self, key: u64) -> Option<u64> {
        let strategies: [fn(&mut Self, u64) -> Option<u64>; 2] = [Self::tablea_0, Self::tableb_0];
        strategies.iter().find_map(|strategy| strategy(self, key))
    }",
            &chain("tablea", 3),
            &chain("tableb", 3),
            close,
        ],
    )
}

fn neg_cfg() -> Case {
    let close = "\n}\n";
    case(
        "neg_cfg",
        ["mac", "other"],
        Expect::Absent,
        &[
            &strukt("Platform", ""),
            r#"
impl Platform {
    pub fn decode(&mut self, key: u64) -> Option<u64> { self.native(key) }
    #[cfg(target_os = "macos")]
    fn native(&mut self, key: u64) -> Option<u64> { self.mac_0(key) }
    #[cfg(not(target_os = "macos"))]
    fn native(&mut self, key: u64) -> Option<u64> { self.other_0(key) }"#,
            &chain("mac", 3),
            &chain("other", 3),
            close,
        ],
    )
}

fn neg_small() -> Case {
    let close = "\n}\n";
    case(
        "neg_small",
        ["up", "down"],
        Expect::Absent,
        &[
            &strukt("Small", ""),
            r"
impl Small {
    pub fn step(&mut self, key: u64, up: bool) -> Option<u64> {
        if up { self.up_0(key) } else { self.down_0(key) }
    }",
            &chain("up", 1),
            &chain("down", 2),
            close,
        ],
    )
}

fn neg_diff() -> Case {
    case(
        "neg_diff",
        ["seek_to", "set_rate"],
        Expect::Absent,
        &[
            &strukt("Commands", ""),
            r"
impl Commands {
    pub fn run(&mut self, key: u64, seek: bool) -> Option<u64> {
        if seek { self.seek_to(key) } else { self.set_rate(key) }
    }
    fn seek_to(&mut self, key: u64) -> Option<u64> {
        self.items.retain(|item| *item < key);
        self.spare.extend(self.items.drain(..));
        self.flush_all()
    }
    fn flush_all(&mut self) -> Option<u64> {
        let total: u64 = self.spare.iter().sum();
        self.spare.clear();
        Some(total)
    }
    fn set_rate(&mut self, key: u64) -> Option<u64> {
        self.limit = usize::try_from(key).ok()?;
        self.apply_limit()
    }
    fn apply_limit(&mut self) -> Option<u64> {
        while self.items.len() > self.limit { self.items.remove(0); }
        self.items.first().copied()
    }
}
",
        ],
    )
}
