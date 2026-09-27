use super::{
    ChainConfig, ChainReport,
    chain::{Chain, Origin, Split},
    corpus::{self, Case, Expect},
    detect,
};

const ROOT: &str = "crates/kithara-synth/src";

fn synth_crate(cases: &[Case]) -> Vec<(String, String)> {
    let lib: String = cases
        .iter()
        .map(|case| format!("pub mod {};\n", case.name))
        .collect();
    let modules = cases
        .iter()
        .map(|case| (format!("{ROOT}/{}.rs", case.name), case.source.clone()));
    std::iter::once((format!("{ROOT}/lib.rs"), lib))
        .chain(modules)
        .collect()
}

fn report(sources: &[(String, String)], min_side_lines: usize) -> ChainReport {
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
    let sources = [(format!("{ROOT}/lib.rs"), source.to_owned())];
    let coverage = report(&sources, 0).coverage;
    let listed = |label: &str| {
        coverage
            .unreached_private
            .iter()
            .any(|entry| entry.ends_with(&format!(" {label}")))
    };

    assert!(listed("Left::tick"), "{:?}", coverage.unreached_private);
    assert!(!listed("Right::peek"), "{:?}", coverage.unreached_private);
}
