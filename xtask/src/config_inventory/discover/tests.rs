use super::discover;

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
