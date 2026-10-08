use super::{
    super::{
        ChainConfig, ChainReport,
        chain::{Chain, Origin, Split},
        corpus::{self, Case, Expect},
        detect,
    },
    consts,
};

fn synth_crate(cases: &[Case]) -> Vec<(String, String)> {
    let root = consts::ROOT;
    let lib: String = cases
        .iter()
        .map(|case| format!("pub mod {};\n", case.name))
        .collect();
    let modules = cases
        .iter()
        .map(|case| (format!("{root}/{}.rs", case.name), case.source.clone()));
    std::iter::once((format!("{root}/lib.rs"), lib))
        .chain(modules)
        .collect()
}

pub(super) fn report(sources: &[(String, String)], min_side_lines: usize) -> ChainReport {
    let config = ChainConfig {
        min_side_lines,
        ..ChainConfig::default()
    };
    detect(sources, &config).expect("chain report")
}

/// A member of the family: a path segment that is the prefix itself or
/// continues it after `_`.
fn in_family(members: &[String], prefix: &str) -> bool {
    members.iter().any(|member| {
        member.split("::").any(|segment| {
            segment
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
        })
    })
}

/// Rows in the case's file whose sides hold its two families.
fn rows<'r>(report: &'r ChainReport, case: &Case) -> Vec<&'r Chain> {
    let file = format!("/{}.rs:", case.name);
    let [a, b] = case.families;
    report
        .chains
        .iter()
        .filter(|chain| {
            let [x, y] = &chain.sides;
            (x.location.contains(&file) || y.location.contains(&file))
                && ((in_family(&x.members, a) && in_family(&y.members, b))
                    || (in_family(&x.members, b) && in_family(&y.members, a)))
        })
        .collect()
}

/// Private functions of a one-file crate that no resolved call reaches.
pub(super) fn unreached(source: &str) -> Vec<String> {
    let sources = [(format!("{}/lib.rs", consts::ROOT), source.to_owned())];
    report(&sources, 0).coverage.unreached_private
}

/// The coverage list names the function `label`.
pub(super) fn lists(unreached: &[String], label: &str) -> bool {
    unreached
        .iter()
        .any(|entry| entry.ends_with(&format!(" {label}")))
}

#[test]
fn every_synthetic_case_pairs_its_two_chain_families() {
    let cases = corpus::cases();
    let report = report(&synth_crate(&cases), 0);
    let missed: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let kinds: Vec<(Origin, Split)> = rows(&report, case)
                .into_iter()
                .map(|chain| (chain.origin, chain.split))
                .collect();
            let found = match case.expect {
                Expect::Paired => !kinds.is_empty(),
                Expect::Row(origin, split) => kinds.contains(&(origin, split)),
                Expect::Absent => kinds.is_empty(),
            };
            (!found).then(|| format!("{}: {kinds:?}", case.name))
        })
        .collect();
    assert!(missed.is_empty(), "cases missed: {missed:#?}");
}

#[test]
fn sides_shorter_than_the_line_floor_drop_the_row() {
    let cases: Vec<Case> = corpus::cases()
        .into_iter()
        .filter(|case| case.name == "user_example")
        .collect();
    let sources = synth_crate(&cases);
    let case = cases.first().expect("user_example case");

    assert!(!rows(&report(&sources, 0), case).is_empty());
    assert!(rows(&report(&sources, 40), case).is_empty());
}

#[test]
fn alike_decisions_in_two_modules_stay_two_rows() {
    let case = corpus::cases()
        .into_iter()
        .find(|case| case.name == "imported_free")
        .expect("imported_free case");
    let root = consts::ROOT;
    let sources = [
        (
            format!("{root}/lib.rs"),
            "pub mod left;\npub mod right;\n".to_owned(),
        ),
        (format!("{root}/left.rs"), case.source.clone()),
        (format!("{root}/right.rs"), case.source),
    ];
    let report = report(&sources, 0);
    let forked_in = |module: &str| {
        report.chains.iter().any(|chain| {
            chain.origin == Origin::Decision
                && chain
                    .fork
                    .as_ref()
                    .is_some_and(|fork| fork.location.contains(&format!("/{module}.rs:")))
        })
    };

    assert!(forked_in("left"), "{:#?}", report.chains);
    assert!(forked_in("right"), "{:#?}", report.chains);
}

#[test]
fn coverage_lists_private_functions_no_resolved_call_reaches() {
    let source = r"
pub struct Left { value: u64 }
pub struct Right { value: u64 }
impl Left {
    fn tick(&mut self) -> u64 { self.value += 1; self.value }
}
impl Right {
    fn tick(&mut self) -> u64 { self.value += 2; self.value }
    fn peek(&self) -> u64 { self.value }
}
pub fn drive() -> u64 {
    vendor::left().tick()
}
pub fn total(right: &Right) -> u64 {
    right.peek()
}
";
    let unreached = unreached(source);

    assert!(lists(&unreached, "Left::tick"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::peek"), "{unreached:?}");
}

#[test]
fn a_call_the_resolver_cannot_place_reaches_nothing() {
    let unreached = unreached(
        r"
pub struct Store { items: Vec<u64> }
impl Store {
    fn flush(&mut self) { self.items.clear(); }
}
fn decode(value: u64) -> u64 { value + 1 }
pub fn run(value: u64, sinks: &mut [vendor::Sink]) -> u64 {
    sinks.iter_mut().for_each(|sink| sink.flush());
    vendor::decode(value)
}
",
    );

    assert!(lists(&unreached, "decode"), "{unreached:?}");
    assert!(lists(&unreached, "Store::flush"), "{unreached:?}");
}

#[test]
fn a_binding_types_only_the_calls_in_its_scope() {
    let unreached = unreached(
        r"
pub struct Left { value: u64 }
pub struct Right { value: u64 }
impl Left {
    fn tick(&mut self) -> u64 { self.value += 1; self.value }
}
impl Right {
    fn tick(&mut self) -> u64 { self.value += 2; self.value }
}
pub struct Up { value: u64 }
pub struct Down { value: u64 }
impl Up {
    fn step(&mut self) -> u64 { self.value += 1; self.value }
}
impl Down {
    fn step(&mut self) -> u64 { self.value -= 1; self.value }
}
pub enum Either { Up(Up), Down(Down) }
pub fn shadow(mut item: Left, other: Right) -> u64 {
    let first = item.tick();
    let mut item = other;
    first + item.tick()
}
pub fn arms(either: Either) -> u64 {
    match either {
        Either::Up(mut moved) => moved.step(),
        Either::Down(mut moved) => moved.step(),
    }
}
",
    );

    for label in ["Left::tick", "Right::tick", "Up::step", "Down::step"] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
}

#[test]
fn a_member_the_type_lacks_resolves_through_its_deref_target() {
    let unreached = unreached(
        r"
use std::ops::Deref;
pub struct Loader { count: u64 }
impl Loader {
    fn load(&self) -> u64 { self.count }
}
pub struct Runtime { loader: Loader, count: u64 }
impl Runtime {
    fn tick(&self) -> u64 { self.count + 1 }
    fn peek(&self) -> u64 { self.count }
}
pub struct Control { runtime: Runtime, count: u64 }
impl Deref for Control {
    type Target = Runtime;
    fn deref(&self) -> &Runtime { &self.runtime }
}
impl Control {
    fn peek(&self) -> u64 { self.count }
    pub fn drive(&self) -> u64 {
        self.loader.load() + self.tick() + self.peek()
    }
}
",
    );

    for label in ["Loader::load", "Runtime::tick", "Control::peek"] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
    assert!(lists(&unreached, "Runtime::peek"), "{unreached:?}");
}
