use super::{discover, registrations};

#[test]
fn registrations_separate_retained_values_from_delegated_operations() {
    let entries = registrations(
        "crates/player/src/control.rs",
        r#"
        /// A retained recipe.
        #[kithara_config::config(builder = false)]
        struct EqConfig<S> {
            /// Smoothing policy.
            #[config(value)] smoothing: SmootherConfig,
            #[config(skip = "injected pool")] pools: S,
        }
        impl<S> PlayerControl<S> {
            /// Replace the live layout.
            #[kithara_config::config(delegate = "eq_layout", sdk)]
            fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), Error> { todo!() }
        }
        "#,
    )
    .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].package, "player");
    assert_eq!(entries[0].module_path, "control");
    assert_eq!(entries[0].kind, "retained");
    assert_eq!(entries[0].fields[0].role, "value");
    assert_eq!(entries[0].fields[1].role, "skip");
    assert_eq!(
        entries[0].fields[1].exclusion_reason.as_deref(),
        Some("injected pool")
    );
    assert_eq!(entries[1].kind, "delegate");
    assert_eq!(entries[1].owner, "PlayerControl < S >");
    assert_eq!(entries[1].property.as_deref(), Some("eq_layout"));
    assert_eq!(entries[1].hook.as_deref(), Some("set_eq_layout"));
    assert!(entries[1].sdk);
    assert_eq!(entries[1].fields[0].rust_type, "Vec < EqBandConfig >");
    assert!(
        entries[1]
            .docs
            .iter()
            .any(|line| line.contains("live layout"))
    );
}

#[test]
fn manifest_reads_composed_builder_field_and_patch_groups() {
    let entries = registrations(
        "crates/kithara-play/src/player/config.rs",
        r#"
        #[kithara_config::config(builder = false)]
        struct PlayerConfig {
            #[config(value, builder(default = Consts::MAX_BAR_RATIO), field(get, copy))]
            rate: u32,
            #[config(skip = "injected resource", builder(default), patch(skip))]
            resource: Option<u32>,
            #[cfg(feature = "web")]
            #[config(value)]
            #[builder(default = 4)]
            count: u32,
            #[config(value(Option<u32>, self.optional), builder(default))]
            optional: Option<u32>,
        }
        "#,
    )
    .unwrap();
    assert_eq!(entries[0].fields[0].role, "value");
    assert_eq!(
        entries[0].fields[0].builder_default.as_deref(),
        Some("Consts :: MAX_BAR_RATIO")
    );
    assert_eq!(entries[0].fields[1].role, "skip");
    assert_eq!(
        entries[0].fields[1].builder_default.as_deref(),
        Some("default")
    );
    assert_eq!(
        entries[0].fields[1].exclusion_reason.as_deref(),
        Some("injected resource")
    );
    assert_eq!(entries[0].fields[2].builder_default.as_deref(), Some("4"));
    assert_eq!(
        entries[0].fields[2].conditions,
        ["# [cfg (feature = \"web\")]"]
    );
    assert_eq!(entries[0].fields[3].role, "value");
    assert_eq!(
        entries[0].fields[3].builder_default.as_deref(),
        Some("default")
    );
}

#[test]
fn schema_modules_and_patch_derives_do_not_require_config_suffixes() {
    let source = "struct Ordinary; mod config { struct Limits; } #[derive(Patch)] struct Recipe; mod other { struct Hidden; }";
    let entries = discover("src/lib.rs", source).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["Limits", "Recipe"]
    );
    for path in [
        "src/config.rs",
        "src/config/host.rs",
        "src/document/schema.rs",
    ] {
        assert_eq!(
            discover(path, "enum Mode { Fast } type Configuration = Mode;")
                .unwrap()
                .len(),
            2
        );
    }
    assert!(
        discover("src/configuration_cache.rs", "struct Ordinary;")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn config_bearing_inputs_are_detected_without_bon() {
    let entries = discover("src/lib.rs", "impl Player { fn new(config: &AudioConfig, mode: Mode) -> Self { todo!() } fn ordinary(value: usize) {} } fn prepare(input: Option<Settings>) {} type Configuration = AudioConfig;").unwrap();
    assert_eq!(
        entries.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
        ["config_inputs", "config_inputs", "alias"]
    );
    assert_eq!(
        entries[0].members,
        ["config : & AudioConfig", "mode : Mode"]
    );
}

#[test]
fn file_conditions_and_associated_aliases_are_not_lost() {
    let entries = discover("src/native.rs", "#![cfg(unix)] impl Service for Audio { type Config = Options; } fn test() { struct LocalConfig; }").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].kind, "associated_alias");
    assert_eq!(entries[0].members, ["Options"]);
    assert_eq!(entries[0].conditions, ["# ! [cfg (unix)]"]);
    assert_eq!(entries[1].conditions, entries[0].conditions);
    assert_eq!(entries[1].scope, ["test"]);
}

#[test]
fn additions_are_detected_without_export_registration() {
    let before = discover("src/lib.rs", "struct Config { value: u32 }").unwrap();
    let after = discover(
        "src/lib.rs",
        "struct Config { value: u32, extra: Option<u64> } struct NewConfig;",
    )
    .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 2);
    assert_ne!(before[0].members, after[0].members);
    assert_eq!(after[1].name, "NewConfig");
}

#[test]
fn scopes_conditions_and_constructor_only_inputs_survive_discovery() {
    let entries = discover(
        "src/lib.rs",
        r#"
        #[cfg(feature = "audio")]
        mod audio {
            struct Config<T> { #[cfg(unix)] value: T }
            #[bon]
            impl<T> Config<T> {
                #[builder]
                fn new(value: T, ephemeral: usize) -> Self { todo!() }
            }
        }
        mod other { struct Config; }
        const TEXT: &str = "struct FakeConfig;";
    "#,
    )
    .unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].scope, ["audio"]);
    assert_eq!(entries[2].scope, ["other"]);
    assert_eq!(entries[0].members, ["# [cfg (unix)] value : T"]);
    assert_eq!(entries[1].kind, "builder_inputs");
    assert_eq!(entries[1].members, ["value : T", "ephemeral : usize"]);
    assert_eq!(entries[0].conditions, entries[1].conditions);
    assert!(!entries[0].conditions.is_empty());
    assert!(entries[2].conditions.is_empty());
}

#[test]
fn enums_aliases_and_builder_names_are_candidates() {
    let entries = discover("src/lib.rs", "enum InputConfig { Value { count: u32 }, Empty } type AliasConfig = InputConfig; #[derive(bon::Builder)] struct Recipe { count: u32 }").unwrap();
    assert_eq!(
        entries.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
        ["enum", "alias", "struct"]
    );
    assert_eq!(entries[0].members, ["Value { count : u32 }", "Empty"]);
    assert_eq!(entries[1].members, ["InputConfig"]);
    assert!(discover("broken.rs", "struct BadConfig {").is_err());
}
