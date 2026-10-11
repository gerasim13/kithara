use std::{fs, ops::Range};

use anyhow::{Context as _, Result};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::{Meta, spanned::Spanned, visit::Visit};

use super::{Check, Context};
use crate::common::{
    exclude::{attr_without_test, attrs_are_test_only, collect_cfg_test_ranges, item_is_test_only},
    violation::Violation,
    walker::{compile_globs, matches_any, relative_to, workspace_rs_files_scoped},
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "cfg_density";

    pub(super) const EXPLANATION: &str = "\
Summary: Too many `#[cfg(...)]` gates scattered across individual items.

Why: Repeated cfg attributes are noisy, error-prone (easy to forget one
branch), and make the file hard to read. Grouping gated code into
dedicated modules with a single `#[cfg]` on the `mod` declaration is
cleaner and more maintainable.

Bad:
    #[cfg(not(target_arch = \"wasm32\"))]
    use std::env;
    #[cfg(not(target_arch = \"wasm32\"))]
    fn native_only() { ... }
    #[cfg(target_arch = \"wasm32\")]
    fn wasm_only() { ... }

Good:
    #[cfg(not(target_arch = \"wasm32\"))]
    mod native;
    #[cfg(target_arch = \"wasm32\")]
    mod wasm;

Resolve: move gated production items into dedicated platform or feature
modules and gate each module once. Test-only item ranges are excluded
automatically. Module boundaries and public reexports already express
this structure, so their gates are not counted. Conditional attributes
count only when they gate existence through `cfg`, not when they add
derives or binding metadata.";
}

pub(crate) struct CfgDensity;

impl Check for CfgDensity {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.cfg_density;
        let exclude = compile_globs(&cfg.exclude_globs);
        let exempt_crates: Vec<&str> = cfg.exempt_crates.iter().map(String::as_str).collect();
        let mut violations = Vec::new();

        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let rel = relative_to(ctx.workspace_root, &path);
            if matches_any(&exclude, rel) {
                continue;
            }
            if is_exempt_crate(rel, &exempt_crates) {
                continue;
            }

            let content = fs::read_to_string(&path)?;
            let count = count_cfg_attributes(&content)
                .with_context(|| format!("cfg_density: parse {}", path.display()))?;
            let key = rel.to_string_lossy().replace('\\', "/");

            if count >= cfg.deny {
                violations.push(
                    Violation::deny(
                        consts::ID,
                        &key,
                        format!("{count} #[cfg] attributes (deny threshold {})", cfg.deny),
                    )
                    .with_explanation(consts::EXPLANATION.into()),
                );
            } else if count >= cfg.warn {
                violations.push(
                    Violation::warn(
                        consts::ID,
                        &key,
                        format!("{count} #[cfg] attributes (warn threshold {})", cfg.warn),
                    )
                    .with_explanation(consts::EXPLANATION.into()),
                );
            }
        }
        Ok(violations)
    }
}

fn count_cfg_attributes(source: &str) -> syn::Result<usize> {
    let file = syn::parse_file(source)?;
    let mut counter = CfgCounter {
        count: 0,
        test_ranges: Vec::new(),
    };
    counter.visit_file(&file);
    Ok(counter.count)
}

struct CfgCounter {
    count: usize,
    test_ranges: Vec<Range<usize>>,
}

impl<'ast> Visit<'ast> for CfgCounter {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if !self.is_test_only(attribute.span().byte_range().start)
            && attr_without_test(&attribute.meta).is_none()
        {
            self.count += 1;
        }
    }

    fn visit_file(&mut self, file: &'ast syn::File) {
        if !attrs_are_test_only(&file.attrs) {
            collect_cfg_test_ranges(&file.items, &mut self.test_ranges);
            syn::visit::visit_file(self, file);
        }
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !item_is_test_only(item) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if let Some((_, items)) = &module.content {
            for item in items {
                self.visit_item(item);
            }
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if matches!(item.vis, syn::Visibility::Inherited) {
            syn::visit::visit_item_use(self, item);
        }
    }

    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        self.visit_macro_tokens(item.tokens.clone());
    }
}

impl CfgCounter {
    fn is_test_only(&self, offset: usize) -> bool {
        self.test_ranges.iter().any(|range| range.contains(&offset))
    }

    /// shortcut: opaque macro grammar keeps token counts; extend when real macro ownership needs resolution.
    fn visit_macro_tokens(&mut self, tokens: TokenStream) {
        let mut attribute = false;
        for token in tokens {
            match token {
                TokenTree::Group(group) => {
                    if attribute
                        && group.delimiter() == Delimiter::Bracket
                        && !self.is_test_only(group.span().byte_range().start)
                        && syn::parse2::<Meta>(group.stream())
                            .is_ok_and(|meta| attr_without_test(&meta).is_none())
                    {
                        self.count += 1;
                    }
                    if group.delimiter() == Delimiter::Brace
                        && let Ok(file) = syn::parse2::<syn::File>(group.stream())
                    {
                        self.visit_file(&file);
                    } else {
                        self.visit_macro_tokens(group.stream());
                    }
                    attribute = false;
                }
                TokenTree::Punct(punct) => attribute = punct.as_char() == '#',
                _ => attribute = false,
            }
        }
    }
}

fn is_exempt_crate(rel: &std::path::Path, exempt: &[&str]) -> bool {
    let mut components = rel.components();
    if components.next().and_then(|c| c.as_os_str().to_str()) != Some("crates") {
        return false;
    }
    let Some(crate_dir) = components.next().and_then(|c| c.as_os_str().to_str()) else {
        return false;
    };
    exempt.contains(&crate_dir)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cargo_metadata::MetadataCommand;

    use super::{CfgDensity, Check, Context, count_cfg_attributes};
    use crate::{
        arch::config::ArchConfig,
        common::{scope::Scope, violation::Severity},
    };

    #[test]
    fn cfg_density_reports_scattered_gates_without_boundary_or_metadata_noise() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let src = root.join("crates/fixture/src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/fixture\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::write(
            src.parent().unwrap().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let boundaries = (0..12)
            .map(|index| format!("#[cfg(feature = \"part-{index}\")] mod part_{index};\n"))
            .collect::<String>();
        fs::write(src.join("lib.rs"), boundaries).unwrap();
        let metadata = (0..12)
            .map(|index| {
                format!(
                    "#[cfg_attr(feature = \"ffi\", derive(uniffi::Record))] struct Record{index};\n"
                )
            })
            .collect::<String>();
        fs::write(src.join("records.rs"), metadata).unwrap();
        let scattered = (0..10)
            .map(|index| format!("#[cfg(unix)] pub fn function_{index}() {{}}\n"))
            .collect::<String>();
        fs::write(
            src.join("runtime.rs"),
            format!("#[cfg(feature = \"runtime\")] mod runtime {{\n{scattered}}}\n"),
        )
        .unwrap();

        let metadata = MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()
            .unwrap();
        let config = ArchConfig::default();
        let scope = Scope::default();
        let ctx = Context::new(&config, &metadata, root, &scope);
        let violations = CfgDensity.run(&ctx).unwrap();

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].key, "crates/fixture/src/runtime.rs");
        assert_eq!(violations[0].severity, Severity::Deny);
        assert_eq!(
            violations[0].message,
            "10 #[cfg] attributes (deny threshold 10)"
        );
    }

    #[test]
    fn cfg_density_ignores_test_only_item_ranges() {
        let source = r#"
#[cfg(target_arch = "wasm32")]
fn production() {}

#[cfg(test)]
mod tests {
    #[cfg(feature = "fixture-a")]
    fn fixture_a() {}

    #[cfg_attr(feature = "fixture-b", ignore)]
    fn fixture_b() {}
}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_counts_unknown_production_predicates() {
        let source = r#"
#[cfg(not(test))]
fn non_test_build() {}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native() {}

#[cfg_attr(feature = "trace", derive(Debug))]
struct Trace;
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_rejects_invalid_rust() {
        let source = "#[cfg(unix)]\nfn broken( {\n#[cfg_attr(test, ignore)]\n";

        assert!(count_cfg_attributes(source).is_err());
    }

    #[test]
    fn cfg_density_counts_nested_conditional_existence() {
        let source = r#"
#[cfg_attr(feature = "bindings", derive(uniffi::Record))]
struct Record;
#[cfg_attr(unix, cfg(feature = "native"))]
fn native() {}
#[cfg_attr(unix, cfg_attr(feature = "native", cfg(feature = "codec")))]
fn codec() {}
#[cfg_attr(feature = "bindings", derive(Debug), cfg(feature = "record"))]
struct OptionalRecord;
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 3);
    }

    #[test]
    fn cfg_density_respects_production_and_nested_test_predicates() {
        let source = r#"
#[cfg(any(test, feature = "mock"))]
fn production_mock() {}
#[cfg_attr(all(), cfg(test))]
fn test_fixture() {
    #[cfg(unix)]
    let _native_fixture = true;
}
#[cfg(not(not(test)))]
fn double_negated_test() {}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_ignores_test_only_attribute_effects() {
        let source = r#"
#[cfg_attr(test, cfg(unix))]
fn test_only_attribute() {}
#[cfg_attr(test, cfg_attr(feature = "native", cfg(unix)))]
fn nested_test_only_attribute() {}
#[cfg_attr(any(test, feature = "native"), cfg(unix))]
fn production_attribute() {}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_excludes_boundaries_but_keeps_scattered_gates() {
        let source = r#"
#[cfg(feature = "file")]
mod file;
#[cfg_attr(unix, cfg(feature = "file"))]
pub use crate::file::File;
#[cfg(feature = "file")]
pub(crate) use crate::file::FileInner;
#[cfg(feature = "audio")]
pub mod audio {
    #[cfg(unix)]
    use std::env;
    #[cfg(feature = "audio")]
    pub use kithara_audio::Audio;
    #[cfg(unix)]
    pub fn open() {
        #[cfg(feature = "trace")]
        let _trace = true;
    }
    struct State {
        #[cfg(unix)]
        descriptor: u64,
    }
}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 4);
    }

    #[test]
    fn cfg_density_counts_all_attributes_on_the_same_line() {
        let source = "#[cfg(unix)] #[cfg(feature = \"native\")] fn native() {}";

        assert_eq!(count_cfg_attributes(source).unwrap(), 2);
    }

    #[test]
    fn cfg_density_keeps_production_gates_beside_test_items() {
        let source = "#[cfg(test)] fn fixture() {} #[cfg(unix)] fn native() {}";

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_keeps_macro_gates_but_ignores_quoted_text() {
        let source = r##"
macro_rules! dispatch {
    ($resource:expr) => {
        match $resource {
            #[cfg(unix)]
            Resource::Disk(resource) => resource,
            Resource::Memory(resource) => resource,
        }
    };
}
const DOCUMENTATION: &str = "#[cfg(unix)]";
"##;

        assert_eq!(count_cfg_attributes(source).unwrap(), 1);
    }

    #[test]
    fn cfg_density_ignores_test_only_rust_items_inside_macros() {
        let source = r#"
macro_rules! declarations {
    () => {
        #[cfg(test)]
        fn fixture() {
            #[cfg(unix)]
            let _native_fixture = true;
        }
        #[cfg_attr(all(), cfg(test))]
        fn nested_fixture() {
            #[cfg(unix)]
            let _native_fixture = true;
        }
        #[cfg(any(test, feature = "mock"))]
        fn production_mock() {
            #[cfg(unix)]
            let _native = true;
        }
    };
}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 2);
    }

    #[test]
    fn cfg_density_uses_test_ownership_inside_macro_impl_trait_and_foreign_items() {
        let source = r#"
macro_rules! declarations {
    () => {
        impl State {
            #[cfg(test)]
            fn fixture() {
                #[cfg(unix)]
                let _native_fixture = true;
            }
            #[cfg(unix)]
            fn production() {}
        }
        trait Fixture {
            #[cfg_attr(all(), cfg(test))]
            fn fixture() {
                #[cfg(unix)]
                let _native_fixture = true;
            }
            #[cfg(any(test, feature = "mock"))]
            fn production_mock() {}
        }
        unsafe extern "C" {
            #[cfg(test)]
            #[cfg(unix)]
            fn fixture();
            #[cfg(unix)]
            fn production();
        }
    };
}
"#;

        assert_eq!(count_cfg_attributes(source).unwrap(), 3);
    }
}
