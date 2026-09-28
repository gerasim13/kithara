use kithara_config::{Config as _, Patch, config};
use kithara_test_utils::kithara;

#[config(default, update)]
#[derive(Clone, Patch)]
struct Levels {
    #[config(value, update, builder(default = 2), field(get, copy))]
    level: u32,
    #[config(value, update, builder(required, default = Some(4)))]
    limit: Option<u32>,
}

#[config]
struct Session<'a> {
    #[config(skip = "borrowed construction resource")]
    resource: &'a str,
    #[config(nested)]
    levels: Levels,
}

struct Wrapped<T>(T);

impl<T> Wrapped<T> {
    fn new(value: T) -> Self {
        Self(value)
    }
}

#[config]
#[derive(Patch)]
struct WrappedConfig {
    #[config(value(u32, self.level.0), wrap(default = 2, with = Wrapped::new, patch))]
    level: Wrapped<u32>,
}

#[kithara::test]
fn retained_values_are_owned_and_resources_stay_private() {
    let mut levels = Levels::default();
    assert_eq!(levels.level(), 2);
    levels.apply_update(LevelsUpdate {
        level: LevelsLevelUpdate::Set { value: 7 },
        limit: LevelsLimitUpdate::Clear,
    });
    let session = Session::builder()
        .resource("injected")
        .levels(levels)
        .build();
    let values = session.values();
    assert_eq!(values.levels.level, 7);
    assert_eq!(values.levels.limit, None);
    assert_eq!(session.resource, "injected");
}

#[kithara::test]
fn wrapped_fields_keep_builder_defaults_setters_and_patch_conversion() {
    let mut config = WrappedConfig::builder().build();
    assert_eq!(config.values().level, 2);

    let configured = WrappedConfig::builder().level(7).build();
    assert_eq!(configured.values().level, 7);

    config.apply(WrappedConfigPatch { level: Some(9) });
    assert_eq!(config.values().level, 9);
}
