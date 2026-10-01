use std::io::Error;

use kithara_config::{Config, Patch};
use kithara_test_utils::kithara;

#[derive(Clone, Patch, Config)]
#[config(default, debug, update)]
struct Levels {
    #[config(value, update, builder(default = 2), field(get, copy))]
    level: u32,
    #[config(value, update, builder(required, default = Some(4)))]
    limit: Option<u32>,
}

#[derive(Clone, Patch, Config)]
#[config(default, update, validate_builder, patch(validate = Self::validated, error = Error))]
struct Bounded {
    #[config(value, update, builder(default = 2), field(get, copy))]
    level: u32,
    #[config(value, update, builder(required, default = Some(4)))]
    limit: Option<u32>,
}

impl Bounded {
    fn validated(self) -> Result<Self, Error> {
        if self.limit.is_some_and(|limit| self.level > limit) {
            return Err(Error::other("level exceeds the limit"));
        }
        Ok(self)
    }
}

#[kithara::test]
fn a_judged_update_commits_whole_or_not_at_all() {
    let mut bounded = Bounded::default();
    assert!(
        bounded
            .apply_update(BoundedUpdate {
                level: BoundedLevelUpdate::Set { value: 7 },
                ..BoundedUpdate::default()
            })
            .is_err()
    );
    assert_eq!(bounded.level(), 2);
    assert_eq!(bounded.values().limit, Some(4));

    bounded
        .apply_update(BoundedUpdate {
            level: BoundedLevelUpdate::Set { value: 7 },
            limit: BoundedLimitUpdate::Clear,
        })
        .expect("a cleared limit admits any level");
    assert_eq!(bounded.level(), 7);
    assert_eq!(bounded.values().limit, None);
}

#[kithara::test]
fn a_judged_builder_uses_the_same_check_as_updates() {
    assert!(Bounded::builder().level(5).build().is_err());
    let mut bounded = Bounded::builder()
        .level(4)
        .build()
        .expect("boundary value is valid");
    bounded
        .apply_update(BoundedUpdate {
            level: BoundedLevelUpdate::Reset,
            ..BoundedUpdate::default()
        })
        .expect("declared default is valid");
    assert_eq!(bounded.level(), 2);
}

#[derive(Config)]
#[config(debug)]
struct Session<'a> {
    #[config(skip = "borrowed construction resource", debug(skip))]
    resource: &'a str,
    #[config(nested)]
    levels: Levels,
    #[config(skip = "derived from the levels it opens with", builder(skip = levels.level()))]
    opened: u32,
}

struct Wrapped<T>(T);

impl<T> Wrapped<T> {
    fn new(value: T) -> Self {
        Self(value)
    }
}

#[derive(Patch, Config)]
struct WrappedConfig {
    #[config(
        value(u32, self.level.0),
        wrap(default = 2, with = Wrapped::new),
        patch(wire = u32, from = Wrapped::new)
    )]
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
    assert_eq!(
        session.opened, 7,
        "a skipped field reads the builder's arguments"
    );
}

#[kithara::test]
fn debug_prints_every_field_but_the_skipped_ones() {
    let session = Session::builder()
        .resource("secret")
        .levels(Levels::default())
        .build();
    assert_eq!(
        format!("{session:?}"),
        "Session { levels: Levels { level: 2, limit: Some(4) }, opened: 2, .. }"
    );
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
