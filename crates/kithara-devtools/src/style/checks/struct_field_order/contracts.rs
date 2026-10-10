use std::{fs, process::Command};

use cargo_metadata::MetadataCommand;
use regex::Regex;
use serde::{
    Deserialize,
    de::value::{Error, SeqDeserializer},
};

use super::{
    super::{Check, Context},
    StructFieldOrder,
    tests::{detect, detect_messages, run_fix},
};
use crate::{
    common::{scan::Scan, scope::Scope},
    style::config::StyleConfig,
};

fn assert_declined(source: &str) {
    let (fixed, _) = run_fix(source);
    assert_eq!(fixed, source, "an unproved contract was reordered");
    let messages = detect_messages(source);
    assert!(
        messages
            .iter()
            .all(|message| message.contains("autofix refused")),
        "an unproved order must explain its autofix refusal: {messages:?}"
    );
}

fn assert_refused(source: &str) {
    assert_declined(source);
    assert!(
        !detect_messages(source).is_empty(),
        "uncertainty must remain visible"
    );
}

fn execute(source: &str) -> String {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let input = fixture.path().join("fixture.rs");
    let executable = fixture.path().join("fixture");
    fs::write(&input, source).expect("write fixture");
    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&input)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile fixture");
    assert!(
        compiled.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(&executable).output().expect("execute fixture");
    assert!(output.status.success(), "fixture execution failed");
    String::from_utf8(output.stdout).expect("fixture UTF-8 output")
}

#[test]
fn fixed_source_preserves_observable_field_destruction_order() {
    let source = r#"
struct Trace(&'static str);

impl Drop for Trace {
    fn drop(&mut self) {
        let mut output = std::io::stdout();
        std::io::Write::write_all(&mut output, self.0.as_bytes()).unwrap();
        std::io::Write::write_all(&mut output, b";").unwrap();
    }
}

struct Owned {
    z: Trace,
    a: Trace,
}

fn main() {
    drop(Owned { z: Trace("first"), a: Trace("second") });
}
"#;
    assert_eq!(execute(source), "first;second;");
    let (fixed, skipped) = run_fix(source);
    assert_eq!(execute(&fixed), "first;second;");
    assert_eq!(fixed, source);
    assert!(
        skipped
            .iter()
            .any(|reason| reason.to_ascii_lowercase().contains("drop")),
        "unsafe destruction order needs a Drop reason: {skipped:?}"
    );
    assert_refused(source);
    assert!(
        detect_messages(source)
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("drop")),
        "diagnostic must preserve the Drop refusal reason"
    );
}

#[test]
fn fixed_source_preserves_derived_comparison_priority() {
    let source = r#"
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Ordered {
    z: i32,
    a: i32,
}

fn main() {
    let first = Ordered { z: 0, a: 1 };
    let second = Ordered { z: 1, a: 0 };
    let result = match first.cmp(&second) {
        std::cmp::Ordering::Less => "Less",
        std::cmp::Ordering::Equal => "Equal",
        std::cmp::Ordering::Greater => "Greater",
    };
    std::io::Write::write_all(&mut std::io::stdout(), result.as_bytes()).unwrap();
}
"#;
    assert_eq!(execute(source), "Less");
    let (fixed, _) = run_fix(source);
    assert_eq!(execute(&fixed), "Less");
    assert_refused(source);
}

#[test]
fn serialization_bytes_and_deserialization_sequence_are_field_order_contracts() {
    macro_rules! wire_fixture {
        ($($declaration:tt)*) => {
            $($declaration)*
            const SOURCE: &str = stringify!($($declaration)*);
        };
    }
    wire_fixture! {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wire {
            z: u32,
            a: u32,
        }
    }
    let wire = Wire { z: 1, a: 2 };
    assert_eq!(
        serde_json::to_string(&wire).expect("serialize"),
        r#"{"z":1,"a":2}"#
    );
    let sequence = SeqDeserializer::<_, Error>::new([1_u32, 2].into_iter());
    let decoded: Wire = Deserialize::deserialize(sequence).expect("deserialize sequence");
    assert_eq!((decoded.z, decoded.a), (1, 2));
    assert_refused(SOURCE);
}

#[test]
fn order_sensitive_derives_and_representations_decline_direct_and_nested_attrs() {
    let contracts = [
        "derive(serde::Serialize)",
        "derive(serde::Deserialize)",
        "derive(Serialize)",
        "derive(Deserialize)",
        "derive(Debug)",
        "derive(Hash)",
        "derive(Ord)",
        "derive(PartialOrd)",
        "derive(Clone)",
        "derive(Default)",
        "derive(Custom)",
        "repr(C)",
        "repr(packed)",
        "repr(transparent)",
    ];
    for contract in contracts {
        for attribute in [
            format!("#[{contract}]"),
            format!("#[cfg_attr(feature = \"contract\", {contract})]"),
            format!("#[cfg_attr(feature = \"outer\", cfg_attr(feature = \"inner\", {contract}))]"),
        ] {
            let source = format!("{attribute}\nstruct S {{\n    z: u32,\n    a: u32,\n}}\n");
            if contract.starts_with("repr") {
                assert_declined(&source);
            } else {
                assert_refused(&source);
            }
        }
    }
}

#[test]
fn opaque_attributes_decline_at_every_enclosing_boundary() {
    let sources = [
        "#![opaque]\nstruct S {\n    z: u32,\n    a: u32,\n}\n",
        "#[opaque]\nmod inner {\nstruct S {\n    z: u32,\n    a: u32,\n}\n}\n",
        "#[opaque]\nstruct S {\n    z: u32,\n    a: u32,\n}\n",
        "struct S {\n    #[opaque]\n    z: u32,\n    a: u32,\n}\n",
        "struct S {\n    #[cfg_attr(feature = \"x\", cfg(feature = \"y\"))]\n    z: u32,\n    #[cfg_attr(feature = \"x\", cfg(feature = \"y\"))]\n    a: u32,\n}\n",
    ];
    for source in sources {
        assert_refused(source);
    }
}

#[test]
fn builder_derives_and_defaults_have_opaque_order_contracts() {
    let sources = [
        "#[derive(bon::Builder)]\nstruct S {\n    z: u32,\n    a: u32,\n}\n",
        "#[cfg_attr(feature = \"x\", cfg_attr(feature = \"y\", derive(bon::Builder)))]\nstruct S {\n    z: u32,\n    a: u32,\n}\n",
        "#[builder]\nstruct S {\n    z: u32,\n    a: u32,\n}\n",
        "struct S {\n    #[builder(default = record(1))]\n    z: u32,\n    #[builder(default = record(2))]\n    a: u32,\n}\n",
    ];
    for source in sources {
        assert_refused(source);
    }
}

#[test]
fn bare_primitive_spellings_require_unshadowed_type_scope() {
    let prefixes = [
        "type u32 = String;",
        "struct u32;",
        "use foreign::Value as u32;",
        "use foreign::*;",
        "declare_types!();",
        "#[opaque]\nfn generated() {}",
    ];
    for prefix in prefixes {
        let source = format!("{prefix}\nstruct S {{\n    z: u32,\n    a: u32,\n}}\n");
        assert_refused(&source);
    }
    assert_refused("struct S<u32> {\n    z: u32,\n    a: u32,\n}\n");
    assert_refused("struct S {\n    z: foreign::u32,\n    a: foreign::u32,\n}\n");
}

#[test]
fn unknown_fields_keep_their_relative_order_and_possible_unsized_tail() {
    for source in [
        "struct S {\n    z: Unknown,\n    a: Unknown,\n}\n",
        "type Tail = [u8];\nstruct S {\n    z: u32,\n    a: Tail,\n}\n",
        "struct S<T: ?Sized> {\n    z: u32,\n    a: T,\n}\n",
    ] {
        assert_refused(source);
        assert!(
            !run_fix(source).1.is_empty(),
            "declined permutation needs a reason"
        );
    }
    let source =
        "struct S<'a> {\n    z: Unknown,\n    borrowed: &'a Unknown,\n    zz: Unknown,\n}\n";
    let (fixed, skipped) = run_fix(source);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(fixed.find("borrowed:").expect("reference") < fixed.find("z:").expect("first owner"));
    assert!(fixed.find("z:").expect("first owner") < fixed.find("zz:").expect("second owner"));
    assert!(detect(&fixed).is_empty());
}

#[test]
fn one_potential_drop_field_can_move_past_references_and_pointers_with_its_comments() {
    let source = "\
struct S<'a> {
    /// Owned resource.
    z: Unknown,
    // Borrowed resource.
    pub a: &'a Unknown,
    // Raw resource.
    pub b: *const Unknown,
}
";
    assert_eq!(detect(source).len(), 1);
    let (fixed, skipped) = run_fix(source);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(fixed.contains("/// Owned resource.\n    z: Unknown,"));
    assert!(fixed.contains("// Borrowed resource.\n    pub a: &'a Unknown,"));
    assert!(fixed.contains("// Raw resource.\n    pub b: *const Unknown,"));
    assert!(fixed.find("pub a:").expect("reference") < fixed.find("pub b:").expect("pointer"));
    assert!(fixed.find("pub b:").expect("pointer") < fixed.find("z:").expect("owner"));
    assert!(detect(&fixed).is_empty());
    assert_eq!(run_fix(&fixed).0, fixed, "eligible fix must be idempotent");
}

#[test]
fn recursive_inert_types_allow_a_style_permutation() {
    let source = "\
struct S {
    z: (u32, [u8; 2]),
    pub callback: fn(Unknown) -> Unknown,
    pub empty: (),
}
";
    assert_eq!(detect(source).len(), 1);
    let (fixed, skipped) = run_fix(source);
    assert_ne!(fixed, source);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(detect(&fixed).is_empty());
    assert_eq!(run_fix(&fixed).0, fixed);
}

#[test]
fn cargo_target_root_is_eligible_but_scoped_external_module_has_unresolved_lineage() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let manifest = fixture.path().join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"field-order-contract\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[lib]\npath = \"engine.rs\"\n",
    )
    .expect("write manifest");
    let root_path = fixture.path().join("engine.rs");
    let root_source = "struct Root {\n    z: u32,\n    a: u32,\n}\n";
    fs::write(&root_path, root_source).expect("write target root");
    let metadata = MetadataCommand::new()
        .manifest_path(&manifest)
        .no_deps()
        .exec()
        .expect("fixture metadata");
    assert_eq!(metadata.packages.len(), 1);
    assert_eq!(metadata.packages[0].targets.len(), 1);
    assert_eq!(
        metadata.packages[0].targets[0].src_path.as_std_path(),
        root_path.as_path()
    );
    let config = StyleConfig::default();
    let root_scan = Scan::new(fixture.path());
    let root_scope = Scope::new(Vec::new(), vec!["engine.rs".into()]);
    let root_context = Context {
        workspace_root: fixture.path(),
        metadata: &metadata,
        scan: &root_scan,
        scope: &root_scope,
        config: &config,
    };
    assert_eq!(
        StructFieldOrder
            .run(&root_context)
            .expect("root detection")
            .len(),
        1
    );
    let root_outcome = StructFieldOrder.fix(&root_context).expect("root fix");
    assert_eq!(root_outcome.writes, 1);
    assert!(
        root_outcome.skipped.is_empty(),
        "{:?}",
        root_outcome.skipped
    );
    let fixed_root = fs::read_to_string(&root_path).expect("read root");
    assert_eq!(fixed_root, "struct Root {\n    a: u32,\n    z: u32,\n}\n");

    let opaque_root = format!("{fixed_root}\n#[opaque]\nmod child;\n");
    fs::write(&root_path, &opaque_root).expect("write external module declaration");
    let child_path = fixture.path().join("child.rs");
    let child_source = "struct Child {\n    z: u32,\n    a: u32,\n}\n";
    fs::write(&child_path, child_source).expect("write external module");
    let child_scan = Scan::new(fixture.path());
    let child_scope = Scope::new(Vec::new(), vec!["child.rs".into()]);
    let child_context = Context {
        scan: &child_scan,
        scope: &child_scope,
        ..root_context
    };
    let child_diagnostics = StructFieldOrder
        .run(&child_context)
        .expect("child detection");
    assert_eq!(child_diagnostics.len(), 1);
    assert!(child_diagnostics[0].message.contains("autofix refused"));
    assert!(child_diagnostics[0].message.contains("lineage"));
    let child_outcome = StructFieldOrder.fix(&child_context).expect("child fix");
    assert_eq!(child_outcome.writes, 0);
    assert!(
        child_outcome
            .skipped
            .iter()
            .any(|reason| reason.contains("lineage")),
        "unresolved external module lineage needs a reason: {:?}",
        child_outcome.skipped
    );
    assert_eq!(
        fs::read_to_string(&child_path).expect("read child"),
        child_source
    );
    assert_eq!(
        fs::read_to_string(&root_path).expect("read root"),
        opaque_root
    );
}

#[test]
fn multiple_cargo_contexts_cannot_rewrite_a_shared_source() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let manifest = fixture.path().join("Cargo.toml");
    let manifest_prefix = "[package]\nname = \"shared-field-order\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[lib]\npath = \"engine.rs\"\n";
    let root_path = fixture.path().join("engine.rs");
    let root_source = "struct Root {\n    z: u32,\n    a: u32,\n}\n";
    fs::write(&root_path, root_source).expect("write shared source");
    let config = StyleConfig::default();
    let scope = Scope::new(Vec::new(), vec!["engine.rs".into()]);
    for bin_path in ["engine.rs", "consumer.rs"] {
        fs::write(
            &manifest,
            format!("{manifest_prefix}\n[[bin]]\nname = \"consumer\"\npath = \"{bin_path}\"\n"),
        )
        .expect("write manifest");
        if bin_path == "consumer.rs" {
            fs::write(
                fixture.path().join(bin_path),
                "type u32 = Trace;\nstruct Trace;\nimpl Drop for Trace { fn drop(&mut self) {} }\ninclude!(\"engine.rs\");\nfn main() {}\n",
            )
            .expect("write source inclusion context");
        }
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest)
            .no_deps()
            .exec()
            .expect("fixture metadata");
        assert_eq!(metadata.packages[0].targets.len(), 2);
        let root_occurrences = metadata.packages[0]
            .targets
            .iter()
            .filter(|target| target.src_path.as_std_path() == root_path.as_path())
            .count();
        assert_eq!(
            root_occurrences,
            if bin_path == "engine.rs" { 2 } else { 1 }
        );
        let scan = Scan::new(fixture.path());
        let context = Context {
            workspace_root: fixture.path(),
            metadata: &metadata,
            scan: &scan,
            scope: &scope,
            config: &config,
        };
        let diagnostics = StructFieldOrder
            .run(&context)
            .expect("shared source detection");
        assert_eq!(
            diagnostics.len(),
            1,
            "shared-source candidate must remain visible"
        );
        assert!(diagnostics[0].message.contains("autofix refused"));
        let outcome = StructFieldOrder.fix(&context).expect("shared source fix");
        assert_eq!(outcome.writes, 0);
        assert!(!outcome.skipped.is_empty());
        assert_eq!(
            fs::read_to_string(&root_path).expect("read shared source"),
            root_source
        );
    }
}

#[test]
fn public_context_refuses_external_modules_and_expression_macros() {
    for (suffix, has_external_child) in [
        ("mod child;\n", true),
        ("fn operation() { assert!(true); }\n", false),
    ] {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"closed-source-contract\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[lib]\npath = \"engine.rs\"\n",
        )
        .expect("write manifest");
        let root_path = fixture.path().join("engine.rs");
        let root_source = format!("struct Root {{\n    z: u32,\n    a: u32,\n}}\n{suffix}");
        fs::write(&root_path, &root_source).expect("write target root");
        let child_path = fixture.path().join("child.rs");
        if has_external_child {
            fs::write(&child_path, "").expect("write readable external module");
        }
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest)
            .no_deps()
            .exec()
            .expect("fixture metadata");
        assert_eq!(metadata.packages.len(), 1);
        assert_eq!(metadata.packages[0].targets.len(), 1);
        assert_eq!(
            metadata.packages[0].targets[0].src_path.as_std_path(),
            root_path.as_path()
        );
        let config = StyleConfig::default();
        let scan = Scan::new(fixture.path());
        let scope = Scope::new(Vec::new(), vec!["engine.rs".into()]);
        let context = Context {
            workspace_root: fixture.path(),
            metadata: &metadata,
            scan: &scan,
            scope: &scope,
            config: &config,
        };
        let diagnostics = StructFieldOrder.run(&context).expect("root detection");
        assert_eq!(
            diagnostics.len(),
            1,
            "unproved root candidate must remain visible"
        );
        assert!(diagnostics[0].message.contains("autofix refused"));
        assert!(diagnostics[0].message.contains("lineage"));
        let outcome = StructFieldOrder.fix(&context).expect("root fix");
        assert_eq!(outcome.writes, 0);
        assert!(
            outcome
                .skipped
                .iter()
                .any(|reason| reason.contains("lineage")),
            "closed-source refusal needs a lineage reason: {:?}",
            outcome.skipped
        );
        assert_eq!(
            fs::read_to_string(&root_path).expect("read target root"),
            root_source
        );
        if has_external_child {
            assert_eq!(
                fs::read_to_string(&child_path).expect("read external module"),
                ""
            );
        }
    }
}

#[test]
fn field_punctuation_stays_valid_when_comments_contain_or_precede_commas() {
    let pattern = Regex::new(r"(?s:/\*.*?\*/)|//[^\n]*").expect("comment pattern");
    let comments = |source: &str| {
        let mut found = pattern
            .find_iter(source)
            .map(|comment| comment.as_str().to_string())
            .collect::<Vec<_>>();
        found.sort();
        found
    };
    for (source, has_trailing_comma) in [
        ("struct S {\n    z: u32,\n    a: u32 // comma,\n}\n", false),
        (
            "struct S {\n    z: u32 /* note */ ,\n    a: u32,\n}\n",
            true,
        ),
    ] {
        syn::parse_file(source).expect("valid punctuation fixture");
        let (fixed, skipped) = run_fix(source);
        syn::parse_file(&fixed).expect("autofix must preserve valid punctuation");
        assert_eq!(
            comments(&fixed),
            comments(source),
            "autofix must preserve the fixture's complete comment multiset"
        );
        if !has_trailing_comma || fixed == source {
            assert_eq!(fixed, source);
            assert!(!skipped.is_empty(), "refused punctuation needs a reason");
        }
        assert_eq!(
            run_fix(&fixed).0,
            fixed,
            "punctuation fix must be idempotent"
        );
    }
}
