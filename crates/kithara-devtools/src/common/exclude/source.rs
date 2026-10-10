use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::Path,
};

use glob::Pattern;
use syn::{
    ForeignItem, ImplItem, Item, Meta, TraitItem,
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
};

use crate::common::{
    violation::Report,
    walker::{compile_globs, matches_any},
};

/// Whether an item carries `#[test]` or a namespaced test attribute.
pub(crate) fn attrs_have_test_marker(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "test")
    })
}

/// Whether the attributes carry a cfg that holds in no build but a test one.
pub(crate) fn attrs_are_test_only(attrs: &[syn::Attribute]) -> bool {
    all_cfg(attrs.iter().map(|attr| attr_without_test(&attr.meta))) == Some(false)
}

/// Whether an item has no production context, including test functions.
pub(crate) fn item_is_test_only(item: &Item) -> bool {
    attrs_are_test_only(item_attrs(item))
        || matches!(item, Item::Fn(function) if attrs_have_test_marker(&function.attrs))
}

fn attr_without_test(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::List(list) if list.path.is_ident("cfg") => {
            cfg_without_test(&syn::parse2::<Meta>(list.tokens.clone()).ok()?)
        }
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            let nested = list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                .ok()?;
            let condition = cfg_without_test(nested.first()?);
            let applied = all_cfg(nested.iter().skip(1).map(attr_without_test));
            match condition {
                Some(true) => applied,
                Some(false) => Some(true),
                None if applied == Some(true) => Some(true),
                None => None,
            }
        }
        _ => Some(true),
    }
}

/// Unknown feature and platform predicates stay potentially production.
fn cfg_without_test(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Some(false),
        Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("any") => {
            let nested = list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                .ok()?;
            let values = nested.iter().map(cfg_without_test);
            if list.path.is_ident("all") {
                all_cfg(values)
            } else {
                all_cfg(values.map(|value| value.map(|value| !value))).map(|value| !value)
            }
        }
        Meta::List(list) if list.path.is_ident("not") => {
            let nested = list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                .ok()?;
            if nested.len() != 1 {
                return None;
            }
            cfg_without_test(nested.first()?).map(|value| !value)
        }
        _ => None,
    }
}

fn all_cfg(values: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut result = Some(true);
    for value in values {
        match value {
            Some(false) => return Some(false),
            None => result = None,
            Some(true) => {}
        }
    }
    result
}
/// Drop every violation the workspace's lint excludes cover: path globs,
/// `#[cfg(test)]` code, and excluded inline modules.
pub fn apply_lint_excludes(
    report: &mut Report,
    paths: &[String],
    modules: &[String],
    test_module_paths: &[String],
    workspace_root: &Path,
) {
    apply_path_excludes(report, paths);
    apply_path_excludes(report, test_module_paths);
    apply_cfg_test_exclusion(report, workspace_root);
    apply_module_excludes(report, modules, workspace_root);
}

/// Drop violations whose path portion matches any glob. A no-op when empty.
pub fn apply_path_excludes(report: &mut Report, patterns: &[String]) {
    if patterns.is_empty() {
        return;
    }
    let globs = compile_globs(patterns);
    report
        .violations
        .retain(|v| !matches_any(&globs, Path::new(key_path(&v.key))));
}

/// Drop violations that land on a line inside a `#[cfg(test)]`-predicated item
/// (`mod tests { ... }`, a test `fn`, a test `impl`, ...). Complements the
/// path-glob pass: inline test modules in production `src/*.rs` files are test
/// code but are not matched by path globs. Only `test`-keyed cfgs count —
/// `#[cfg(feature = ...)]` and other cfgs are left untouched. Files that fail
/// to parse are skipped (their violations are kept).
pub fn apply_cfg_test_exclusion(report: &mut Report, workspace_root: &Path) {
    let ranges = ranges_by_file(report, workspace_root, |items, out| {
        collect_cfg_test_ranges(items, out);
    });
    retain_outside_ranges(report, &ranges);
}

/// Drop violations inside an inline `mod` whose leaf name or file-relative
/// `::`-path matches a glob. Complements the path-glob and `#[cfg(test)]`
/// passes with sub-file, module-scoped exclusion (e.g. scope out a whole
/// `mod legacy {}` without listing files). A no-op when `patterns` is empty.
pub fn apply_module_excludes(report: &mut Report, patterns: &[String], workspace_root: &Path) {
    if patterns.is_empty() {
        return;
    }
    let globs = compile_globs(patterns);
    let ranges = ranges_by_file(report, workspace_root, |items, out| {
        collect_module_ranges(items, &mut Vec::new(), &globs, out);
    });
    retain_outside_ranges(report, &ranges);
}

/// Parse every file referenced by a violation once and collect its excluded
/// line ranges via `collect`. Files that fail to parse contribute no ranges
/// (their violations are kept).
struct FileExclusion {
    lines: BTreeSet<usize>,
    whole_file: bool,
}

fn ranges_by_file(
    report: &Report,
    workspace_root: &Path,
    collect: impl Fn(&[Item], &mut Vec<Range<usize>>),
) -> BTreeMap<String, FileExclusion> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    for v in &report.violations {
        files.insert(key_path(&v.key).to_string());
    }
    files
        .into_iter()
        .filter_map(|rel| {
            let source = fs::read_to_string(workspace_root.join(&rel)).ok()?;
            let file = syn::parse_file(&source).ok()?;
            let mut ranges = Vec::new();
            collect(&file.items, &mut ranges);
            let lines = fully_excluded_lines(&source, &mut ranges);
            let whole_file = attrs_are_test_only(&file.attrs);
            (whole_file || !lines.is_empty()).then_some((rel, FileExclusion { lines, whole_file }))
        })
        .collect()
}

/// Keep a violation unless its line falls inside one of its file's ranges.
/// Violations whose key carries no line (e.g. `file.rs::Name`) are kept.
fn retain_outside_ranges(report: &mut Report, ranges: &BTreeMap<String, FileExclusion>) {
    report.violations.retain(|v| {
        let Some(excluded) = ranges.get(key_path(&v.key)) else {
            return true;
        };
        !excluded.whole_file && key_line(&v.key).is_none_or(|line| !excluded.lines.contains(&line))
    });
}

/// Extract the workspace-relative path portion from a violation key. Keys look
/// like `crates/<crate>/src/.../file.rs:line:col` or `file.rs::Name`; the path
/// always ends at the `.rs` extension, so we slice up to and including it.
fn key_path(key: &str) -> &str {
    key.find(".rs").map_or(key, |i| &key[..i + 3])
}

/// Extract the 1-based source line a violation key points at. The line is the
/// first `:`-separated field after the `.rs` path portion, e.g. `561` in
/// `crates/x/src/foo.rs:561:for_body`. Returns `None` for keys that carry no
/// line (e.g. `file.rs::Name`).
fn key_line(key: &str) -> Option<usize> {
    let path = key_path(key);
    let rest = key.get(path.len()..)?.strip_prefix(':')?;
    let field = rest.split(':').next()?;
    field.parse::<usize>().ok()
}

/// Count lines in `src` that do not fall inside a `#[cfg(test)]` item range.
/// File-keyed checks (e.g. `file_size`) carry no line in their violation key,
/// so the line-based [`apply_cfg_test_exclusion`] pass cannot reach them — they
/// fold the same test-code exclusion in here instead, measuring only the
/// production surface. Returns the raw line count when `src` fails to parse.
#[must_use]
pub fn non_test_line_count(src: &str) -> usize {
    let total = src.lines().count();
    let Some(excluded) = cfg_test_lines(src) else {
        return total;
    };
    total.saturating_sub(excluded.len())
}

/// Lines containing only test code and whitespace; mixed production lines stay.
pub(crate) fn cfg_test_lines(src: &str) -> Option<BTreeSet<usize>> {
    let mut ranges = cfg_test_byte_ranges(src)?;
    Some(fully_excluded_lines(src, &mut ranges))
}

/// Exact byte ranges belonging to test-only items, including nested block items.
pub(crate) fn cfg_test_byte_ranges(src: &str) -> Option<Vec<Range<usize>>> {
    let file = syn::parse_file(src).ok()?;
    if attrs_are_test_only(&file.attrs) {
        return Some(std::iter::once(0..src.len()).collect());
    }
    let mut ranges = Vec::new();
    collect_cfg_test_ranges(&file.items, &mut ranges);
    Some(ranges)
}

fn fully_excluded_lines(source: &str, ranges: &mut [Range<usize>]) -> BTreeSet<usize> {
    ranges.sort_by_key(|range| range.start);
    let mut out = BTreeSet::new();
    let mut offset = 0;
    for (index, line) in source.split_inclusive('\n').enumerate() {
        let end = offset + line.len();
        let mut cursor = offset;
        let mut covered = false;
        let mut production = false;
        for range in ranges
            .iter()
            .filter(|range| range.start < end && range.end > offset)
        {
            covered = true;
            let start = range.start.max(cursor).min(end);
            production |= !source[cursor..start].trim().is_empty();
            cursor = cursor.max(range.end.min(end));
        }
        if covered && !production && source[cursor..end].trim().is_empty() {
            out.insert(index + 1);
        }
        offset = end;
    }
    out
}

fn collect_cfg_test_ranges(items: &[Item], out: &mut Vec<Range<usize>>) {
    let mut visitor = TestRanges { out };
    for item in items {
        visitor.visit_item(item);
    }
}

struct TestRanges<'a> {
    out: &'a mut Vec<Range<usize>>,
}

impl<'ast> Visit<'ast> for TestRanges<'_> {
    fn visit_foreign_item(&mut self, item: &'ast ForeignItem) {
        let attrs = match item {
            ForeignItem::Fn(value) => &value.attrs,
            ForeignItem::Static(value) => &value.attrs,
            ForeignItem::Type(value) => &value.attrs,
            ForeignItem::Macro(value) => &value.attrs,
            _ => return,
        };
        if attrs_are_test_only(attrs) {
            self.out.push(item.span().byte_range());
        } else {
            visit::visit_foreign_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let attrs = match item {
            ImplItem::Const(value) => &value.attrs,
            ImplItem::Fn(value) => &value.attrs,
            ImplItem::Type(value) => &value.attrs,
            ImplItem::Macro(value) => &value.attrs,
            _ => return,
        };
        if attrs_are_test_only(attrs)
            || matches!(item, ImplItem::Fn(_) if attrs_have_test_marker(attrs))
        {
            self.out.push(item.span().byte_range());
        } else {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_item(&mut self, item: &'ast Item) {
        if item_is_test_only(item) {
            self.out.push(item.span().byte_range());
        } else {
            visit::visit_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        let attrs = match item {
            TraitItem::Const(value) => &value.attrs,
            TraitItem::Fn(value) => &value.attrs,
            TraitItem::Type(value) => &value.attrs,
            TraitItem::Macro(value) => &value.attrs,
            _ => return,
        };
        if attrs_are_test_only(attrs)
            || matches!(item, TraitItem::Fn(_) if attrs_have_test_marker(attrs))
        {
            self.out.push(item.span().byte_range());
        } else {
            visit::visit_trait_item(self, item);
        }
    }
}

/// Record the line range of every inline `mod` whose leaf name or `::`-path
/// (joined from the file root) matches one of the compiled globs. Recurses so
/// nested modules match at any depth.
fn collect_module_ranges(
    items: &[Item],
    path: &mut Vec<String>,
    globs: &[Pattern],
    out: &mut Vec<Range<usize>>,
) {
    for item in items {
        let Item::Mod(m) = item else {
            continue;
        };
        let leaf = m.ident.to_string();
        path.push(leaf.clone());
        let full = path.join("::");
        if matches_any(globs, Path::new(&leaf)) || matches_any(globs, Path::new(&full)) {
            out.push(m.span().byte_range());
        }
        if let Some((_, inner)) = &m.content {
            collect_module_ranges(inner, path, globs, out);
        }
        path.pop();
    }
}

pub(crate) fn item_attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Whether cfg attributes make compilation without `test` impossible.
/// Unknown feature/platform predicates and malformed cfg remain checked.
#[must_use]
pub fn attrs_have_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs_are_test_only(attrs)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::common::{exclude::cfg_test_module_globs, violation::Violation};

    fn attrs_of(source: &str) -> Vec<syn::Attribute> {
        let file = syn::parse_file(source).expect("the fixture parses");
        item_attrs(&file.items[0]).to_vec()
    }

    fn workspace_with(file: &str, source: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a temporary workspace");
        write_fixture(root.path(), file, source);
        root
    }

    fn write_fixture(root: &Path, file: &str, source: &str) {
        let path = root.join(file);
        fs::create_dir_all(path.parent().expect("the file sits in a directory"))
            .expect("the directory is created");
        fs::write(&path, source).expect("the fixture is written");
    }

    #[test]
    fn a_plain_cfg_test_module_is_test_only() {
        assert!(attrs_are_test_only(&attrs_of("#[cfg(test)]\nmod probe;")));
    }

    #[test]
    fn a_cfg_all_carrying_test_is_test_only() {
        assert!(attrs_are_test_only(&attrs_of(
            "#[cfg(all(test, feature = \"masonry\"))]\nmod probe;"
        )));
    }

    #[test]
    fn a_module_a_feature_can_also_turn_on_is_not_test_only() {
        assert!(!attrs_are_test_only(&attrs_of(
            "#[cfg(any(test, feature = \"mock\"))]\npub mod mock;"
        )));
    }

    #[test]
    fn a_declared_test_module_excludes_only_resolved_existing_files() {
        let root = workspace_with(
            "crates/x/src/backends/mod.rs",
            "#[cfg(all(test, feature = \"masonry\"))]\nmod conformance;\nmod vello;\n",
        );
        write_fixture(
            root.path(),
            "crates/x/src/backends/conformance.rs",
            "mod nested;",
        );
        write_fixture(
            root.path(),
            "crates/x/src/backends/conformance/nested.rs",
            "fn probe() {}",
        );
        write_fixture(
            root.path(),
            "crates/x/src/backends/conformance/unused.rs",
            "fn prod() {}",
        );
        assert_eq!(
            cfg_test_module_globs(root.path()),
            vec![
                "crates/x/src/backends/conformance.rs".to_owned(),
                "crates/x/src/backends/conformance/nested.rs".to_owned(),
            ]
        );
    }

    #[test]
    fn a_production_module_is_left_in_scope() {
        let root = workspace_with("crates/x/src/lib.rs", "mod vello;\n");
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    fn exclude_globs() -> Vec<String> {
        [
            "**/tests/**",
            "**/tests.rs",
            "**/*_test.rs",
            "**/test_*.rs",
            "crates/kithara-test-utils/**",
            "crates/kithara-test-macros/**",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    #[test]
    fn path_excludes_drop_test_code_keep_production() {
        let mut report = Report::default();
        report.extend([
            Violation::warn("loop_allocation", "crates/x/tests.rs:10:4", "test"),
            Violation::warn(
                "fat_loop_body",
                "crates/x/tests/helper.rs:5:loop_body",
                "test",
            ),
            Violation::warn(
                "box_concrete_type",
                "crates/kithara-test-utils/src/a.rs:1:1",
                "test",
            ),
            Violation::warn("loop_allocation", "crates/x/src/foo.rs:7:8", "prod"),
        ]);

        apply_path_excludes(&mut report, &exclude_globs());

        let keys: Vec<&str> = report.violations.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, ["crates/x/src/foo.rs:7:8"]);
    }

    #[test]
    fn path_excludes_empty_is_noop() {
        let mut report = Report::default();
        report.extend([Violation::warn(
            "loop_allocation",
            "crates/x/tests.rs:10:4",
            "test",
        )]);
        apply_path_excludes(&mut report, &[]);
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn key_path_strips_line_col_and_name_suffix() {
        assert_eq!(key_path("crates/x/src/foo.rs:7:8"), "crates/x/src/foo.rs");
        assert_eq!(
            key_path("crates/x/src/foo.rs:173:48::outcome"),
            "crates/x/src/foo.rs"
        );
        assert_eq!(key_path("kithara-foo"), "kithara-foo");
    }

    #[test]
    fn key_line_reads_first_field_after_path() {
        assert_eq!(key_line("crates/x/src/foo.rs:561:for_body"), Some(561));
        assert_eq!(key_line("crates/x/src/foo.rs:7:8"), Some(7));
        assert_eq!(key_line("crates/x/src/foo.rs:173:48::outcome"), Some(173));
        assert_eq!(key_line("crates/x/src/foo.rs::Name"), None);
        assert_eq!(key_line("kithara-foo"), None);
    }

    fn cfg_ranges(src: &str) -> Vec<(usize, usize)> {
        line_intervals(cfg_test_lines(src).expect("valid Rust source"))
    }

    fn line_intervals(lines: BTreeSet<usize>) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for line in lines {
            if let Some((_, end)) = out.last_mut()
                && line == *end + 1
            {
                *end = line;
            } else {
                out.push((line, line));
            }
        }
        out
    }

    #[test]
    fn cfg_test_mod_range_covers_inner_lines() {
        let src = "fn prod() {}\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                       fn unit() {}\n\
                   }\n";
        // The `#[cfg(test)] mod tests` spans its attribute line through close.
        assert_eq!(cfg_ranges(src), [(2, 5)]);
    }

    #[test]
    fn cfg_any_test_retains_production_reachability() {
        let any_test = "#[cfg(any(test, feature = \"x\"))]\nmod m { fn f() {} }\n";
        assert!(cfg_ranges(any_test).is_empty());
        let feature_only = "#[cfg(feature = \"test\")]\nmod m { fn f() {} }\n";
        assert!(cfg_ranges(feature_only).is_empty());
    }

    #[test]
    fn cfg_predicates_exclude_only_provably_nonproduction_items() {
        for predicate in [
            "test",
            "all(feature = \"x\", test)",
            "any(test, all(test, target_os = \"linux\"))",
            "not(not(test))",
            "not(any(feature = \"x\", not(test)))",
            "all(\n feature = \"x\",\n any(test, all(test, feature = \"y\")),\n)",
        ] {
            let attrs = attrs_of(&format!("#[cfg({predicate})]\nfn f() {{}}"));
            assert!(attrs_have_cfg_test(&attrs), "{predicate}");
            assert!(attrs_are_test_only(&attrs), "{predicate}");
        }
        for predicate in [
            "not(test)",
            "any(test, feature = \"x\")",
            "not(all(test, feature = \"x\"))",
            "feature = \"test\"",
            "unknown(test)",
        ] {
            let attrs = attrs_of(&format!("#[cfg({predicate})]\nfn f() {{}}"));
            assert!(!attrs_have_cfg_test(&attrs), "{predicate}");
            assert!(!attrs_are_test_only(&attrs), "{predicate}");
        }
    }

    #[test]
    fn cfg_attr_is_an_implication_not_a_test_marker() {
        for source in [
            "#[cfg_attr(test, derive(Debug))]\nstruct S;",
            "#[cfg_attr(feature = \"x\", cfg(test))]\nstruct S;",
            "#[cfg_attr(test, cfg(not(test)))]\nstruct S;",
            "#[cfg(test extra)]\nstruct S;",
        ] {
            assert!(!attrs_have_cfg_test(&attrs_of(source)), "{source}");
        }
        let attrs = attrs_of("#[cfg_attr(not(test), cfg(test))]\nstruct S;");
        assert!(attrs_have_cfg_test(&attrs));
        assert!(attrs_are_test_only(&attrs));
    }

    #[test]
    fn file_inner_cfg_and_associated_member_ranges_are_test_only() {
        let source = "#![cfg(all(test, feature = \"x\"))]\nfn helper() {}\n";
        assert_eq!(non_test_line_count(source), 0);
        assert_eq!(cfg_test_lines(source), Some(BTreeSet::from([1, 2])));

        let source = "trait T {\n#[cfg(test)]\nfn unit();\n#[cfg(test)]\nconst C: usize;\n#[cfg(test)]\ntype Value;\n}\nimpl T for S {\n#[cfg(test)]\nconst C: usize = 1;\n#[cfg(test)]\ntype Value = u8;\n}\n";
        let lines = cfg_test_lines(source).expect("valid fixture");
        for line in [2, 3, 4, 5, 6, 7, 10, 11, 12, 13] {
            assert!(lines.contains(&line), "associated member line {line}");
        }
        assert!(!lines.contains(&1));
        assert!(!lines.contains(&9));
    }

    #[test]
    fn test_only_inline_parents_and_paths_cover_external_descendants() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(all(\n test,\n feature = \"x\",\n))]\nmod fixture {\n#[path = \"cases.rs\"] mod examples;\n}\n#[cfg(test)]\n#[path = \"support/fixture.rs\"] mod renamed;\n",
        );
        write_fixture(
            root.path(),
            "crates/x/src/fixture/cases.rs",
            "fn helper() {}",
        );
        write_fixture(
            root.path(),
            "crates/x/src/support/fixture.rs",
            "mod nested;",
        );
        write_fixture(
            root.path(),
            "crates/x/src/support/fixture/nested.rs",
            "fn helper() {}",
        );
        assert_eq!(
            cfg_test_module_globs(root.path()),
            vec![
                "crates/x/src/fixture/cases.rs".to_owned(),
                "crates/x/src/support/fixture.rs".to_owned(),
                "crates/x/src/support/fixture/nested.rs".to_owned(),
            ]
        );
    }

    #[test]
    fn production_reuse_keeps_a_physical_file_and_its_children_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[path = \"shared.rs\"] mod regular;\n#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;\n",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn external_test_context_is_refreshed_after_parent_edit() {
        let parent = "crates/x/src/lib.rs";
        let child = "crates/x/src/fixture.rs";
        let root = workspace_with(parent, "mod fixture;");
        write_fixture(root.path(), child, "fn helper() {}");
        let report = || {
            let mut report = Report::default();
            report.extend([Violation::warn("file_density", child, "finding")]);
            report
        };
        let mut before = report();
        apply_lint_excludes(
            &mut before,
            &[],
            &[],
            &cfg_test_module_globs(root.path()),
            root.path(),
        );
        assert_eq!(before.violations.len(), 1);
        write_fixture(root.path(), parent, "#[cfg(test)]\nmod fixture;");
        let mut after = report();
        apply_lint_excludes(
            &mut after,
            &[],
            &[],
            &cfg_test_module_globs(root.path()),
            root.path(),
        );
        assert!(after.violations.is_empty());
    }

    #[test]
    fn file_inner_cfg_filters_keys_without_source_lines() {
        let rel = "crates/x/src/fixture.rs";
        let root = workspace_with(rel, "#![cfg(test)]\nfn helper() {}");
        let mut report = Report::default();
        report.extend([
            Violation::warn("file_density", rel, "finding"),
            Violation::warn("no_lib_statics", format!("{rel}::static VALUE"), "finding"),
        ]);
        apply_cfg_test_exclusion(&mut report, root.path());
        assert!(report.violations.is_empty());
    }

    #[test]
    fn test_markers_apply_to_functions_without_hiding_other_item_kinds() {
        let source = "#[tokio::test]\nasync fn unit() {}\n#[custom::test]\nstruct Production;\n";
        let file = syn::parse_file(source).expect("valid fixture");
        assert!(item_is_test_only(&file.items[0]));
        assert!(!item_is_test_only(&file.items[1]));
        assert_eq!(cfg_test_lines(source), Some(BTreeSet::from([1, 2])));
    }

    #[test]
    fn an_explicit_cargo_target_keeps_a_test_reused_source_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"../custom.rs\"] mod fixture;",
        );
        write_fixture(root.path(), "crates/x/custom.rs", "fn helper() {}");
        write_fixture(
            root.path(),
            "crates/x/Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n[lib]\npath = \"custom.rs\"\n",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn a_malformed_cargo_manifest_keeps_the_crate_checked() {
        let root = workspace_with("crates/x/src/lib.rs", "#[cfg(test)]\nmod fixture;");
        write_fixture(root.path(), "crates/x/src/fixture.rs", "fn helper() {}");
        write_fixture(root.path(), "crates/x/Cargo.toml", "[package");
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn cargo_roots_and_module_reuse_resolve_children_in_their_own_context() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[path = \"../custom.rs\"] mod reused;\n#[cfg(test)]\n#[path = \"../custom/child.rs\"] mod fixture;",
        );
        write_fixture(root.path(), "crates/x/custom.rs", "mod child;");
        write_fixture(root.path(), "crates/x/child.rs", "fn helper() {}");
        write_fixture(root.path(), "crates/x/custom/child.rs", "fn helper() {}");
        write_fixture(
            root.path(),
            "crates/x/Cargo.toml",
            "[lib]\npath = \"custom.rs\"\n",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn ambiguous_and_conditional_module_paths_stay_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\nmod ambiguous;\n#[cfg(test)]\n#[cfg_attr(feature = \"x\", path = \"changed.rs\")]\nmod conditional;",
        );
        for file in [
            "ambiguous.rs",
            "ambiguous/mod.rs",
            "conditional.rs",
            "changed.rs",
        ] {
            write_fixture(
                root.path(),
                &format!("crates/x/src/{file}"),
                "fn helper() {}",
            );
        }
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn conditional_production_reuse_keeps_a_test_source_and_descendants_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;\n#[cfg_attr(feature = \"x\", path = \"shared.rs\")]\nmod regular;",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        write_fixture(root.path(), "crates/x/src/regular.rs", "fn helper() {}");
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn production_block_local_reuse_keeps_a_test_source_and_descendants_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;\nfn prod() { { #[path = \"shared.rs\"] mod regular; } }",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn block_local_module_in_a_test_function_does_not_create_production_reuse() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;\n#[cfg(all(test, feature = \"x\"))]\nfn unit() { #[path = \"shared.rs\"] mod local; }",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        assert_eq!(
            cfg_test_module_globs(root.path()),
            vec![
                "crates/x/src/shared.rs".to_owned(),
                "crates/x/src/shared/child.rs".to_owned(),
            ]
        );
    }

    #[test]
    fn ambiguous_production_reuse_keeps_all_source_candidates_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;\nmod shared;",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(root.path(), "crates/x/src/shared/mod.rs", "fn helper() {}");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn same_line_production_is_retained_by_lines_and_precise_in_bytes() {
        let source =
            "#[cfg(test)] fn unit() { let value = 4000; } fn prod() { let value = 5000; }\n";
        let ranges = cfg_test_byte_ranges(source).expect("valid fixture");
        assert!(
            ranges
                .iter()
                .any(|range| range.contains(&source.find("4000").expect("test value")))
        );
        assert!(
            !ranges
                .iter()
                .any(|range| range.contains(&source.find("5000").expect("production value")))
        );
        assert!(cfg_test_lines(source).expect("valid fixture").is_empty());
        assert_eq!(non_test_line_count(source), 1);
        let rel = "crates/x/src/lib.rs";
        let root = workspace_with(rel, source);
        let mut report = Report::default();
        report.extend([Violation::warn(
            "magic_number",
            format!("{rel}:1:4"),
            "production",
        )]);
        apply_cfg_test_exclusion(&mut report, root.path());
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn nested_block_items_use_the_same_test_ranges_as_module_items() {
        let source =
            "fn prod() {\n#[cfg(test)]\nfn unit() { let value = 4000; }\nlet value = 5000;\n}\n";
        let ranges = cfg_test_byte_ranges(source).expect("valid fixture");
        assert!(
            ranges
                .iter()
                .any(|range| range.contains(&source.find("4000").expect("test value")))
        );
        assert!(
            !ranges
                .iter()
                .any(|range| range.contains(&source.find("5000").expect("production value")))
        );
        assert_eq!(cfg_test_lines(source), Some(BTreeSet::from([2, 3])));
    }

    #[test]
    fn unknown_parent_context_keeps_a_test_reused_source_checked() {
        let root = workspace_with(
            "crates/x/src/lib.rs",
            "#[cfg(test)]\n#[path = \"shared.rs\"] mod fixture;",
        );
        write_fixture(root.path(), "crates/x/src/shared.rs", "mod child;");
        write_fixture(
            root.path(),
            "crates/x/src/shared/child.rs",
            "fn helper() {}",
        );
        write_fixture(
            root.path(),
            "crates/x/src/unknown.rs",
            "#[path = \"shared.rs\"] mod reused;\nfn invalid( {",
        );
        assert!(cfg_test_module_globs(root.path()).is_empty());
        fs::write(root.path().join("crates/x/src/unknown.rs"), [0xff])
            .expect("unreadable UTF-8 fixture");
        assert!(cfg_test_module_globs(root.path()).is_empty());
    }

    #[test]
    fn malformed_or_missing_module_context_keeps_findings() {
        let root = workspace_with("crates/x/src/lib.rs", "mod invalid {");
        write_fixture(root.path(), "crates/x/src/fixture.rs", "fn helper() {}");
        assert!(cfg_test_module_globs(root.path()).is_empty());
        let mut report = Report::default();
        report.extend([Violation::warn(
            "file_density",
            "crates/x/src/missing.rs",
            "finding",
        )]);
        apply_lint_excludes(
            &mut report,
            &[],
            &[],
            &cfg_test_module_globs(root.path()),
            root.path(),
        );
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn apply_cfg_test_exclusion_filters_inline_test_module() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rel = "crates/x/src/foo.rs";
        let src = "fn prod() {\n\
                   \x20\x20\x20\x20for _ in 0..1 {}\n\
                   }\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                       fn unit() {\n\
                       \x20\x20\x20\x20for _ in 0..1 {}\n\
                       }\n\
                   }\n";
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, src).expect("write fixture");

        let mut report = Report::default();
        report.extend([
            Violation::warn("fat_loop_body", format!("{rel}:2:for_body"), "prod"),
            Violation::warn("fat_loop_body", format!("{rel}:7:for_body"), "test"),
        ]);

        apply_cfg_test_exclusion(&mut report, tmp.path());

        let keys: Vec<&str> = report.violations.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, [format!("{rel}:2:for_body")]);
    }

    #[test]
    fn apply_cfg_test_exclusion_keeps_unparsable_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rel = "crates/x/src/broken.rs";
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "this is not valid rust ::: {{{").expect("write fixture");

        let mut report = Report::default();
        report.extend([Violation::warn(
            "fat_loop_body",
            format!("{rel}:3:for_body"),
            "kept",
        )]);

        apply_cfg_test_exclusion(&mut report, tmp.path());
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn non_test_line_count_subtracts_cfg_test_modules() {
        let src = "fn a() {}\n\
                   fn b() {}\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                       fn t1() {}\n\
                       fn t2() {}\n\
                   }\n";
        // 7 lines total, the `#[cfg(test)] mod` spans lines 3..=7 (5 lines).
        assert_eq!(non_test_line_count(src), 2);
    }

    #[test]
    fn non_test_line_count_matches_cfg_all_test_feature() {
        let src = "fn a() {}\n\
                   #[cfg(all(test, feature = \"x\"))]\n\
                   mod tests {\n\
                       fn t() {}\n\
                   }\n";
        // `all(test, feature)` is a test cfg: lines 2..=5 drop, leaving 1.
        assert_eq!(non_test_line_count(src), 1);
    }

    #[test]
    fn non_test_line_count_keeps_unparsable_and_test_free() {
        assert_eq!(non_test_line_count("not ::: valid {{{"), 1);
        assert_eq!(non_test_line_count("fn a() {}\nfn b() {}\n"), 2);
    }

    fn module_ranges(src: &str, patterns: &[&str]) -> Vec<(usize, usize)> {
        let file: syn::File = syn::parse_str(src).expect("valid Rust source");
        let globs = compile_globs(
            &patterns
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
        );
        let mut out = Vec::new();
        collect_module_ranges(&file.items, &mut Vec::new(), &globs, &mut out);
        line_intervals(fully_excluded_lines(src, &mut out))
    }

    #[test]
    fn collect_module_ranges_matches_leaf_and_nested_path() {
        let src = "mod keep {\n\
                   \x20\x20\x20\x20fn a() {}\n\
                   }\n\
                   mod outer {\n\
                       mod legacy {\n\
                       \x20\x20\x20\x20fn b() {}\n\
                       }\n\
                   }\n";
        assert_eq!(module_ranges(src, &["legacy"]), [(5, 7)]);
        assert_eq!(module_ranges(src, &["outer::legacy"]), [(5, 7)]);
        assert!(module_ranges(src, &["missing"]).is_empty());
    }

    #[test]
    fn apply_module_excludes_filters_named_module() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rel = "crates/x/src/foo.rs";
        let src = "fn prod() {\n\
                   \x20\x20\x20\x20for _ in 0..1 {}\n\
                   }\n\
                   mod legacy {\n\
                       fn old() {\n\
                       \x20\x20\x20\x20for _ in 0..1 {}\n\
                       }\n\
                   }\n";
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, src).expect("write fixture");

        let mut report = Report::default();
        report.extend([
            Violation::warn("fat_loop_body", format!("{rel}:2:for_body"), "prod"),
            Violation::warn("fat_loop_body", format!("{rel}:6:for_body"), "legacy"),
        ]);

        apply_module_excludes(&mut report, &["legacy".to_string()], tmp.path());

        let keys: Vec<&str> = report.violations.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, [format!("{rel}:2:for_body")]);
    }

    #[test]
    fn apply_module_excludes_empty_is_noop() {
        let mut report = Report::default();
        report.extend([Violation::warn(
            "fat_loop_body",
            "crates/x/src/foo.rs:6:for_body",
            "kept",
        )]);
        apply_module_excludes(&mut report, &[], Path::new("/nonexistent"));
        assert_eq!(report.violations.len(), 1);
    }
}
