use std::io::Error;

use kithara_config::{Config, ConfigOwner, ConfigOwnerMut, Patch, UpdatableConfig};
use kithara_test_utils::kithara;

#[derive(Clone, Patch, Config)]
#[config(default, debug, update)]
struct Levels {
    #[config(value, update, builder(default = 2), get(copy))]
    level: u32,
    #[config(value, update, builder(required, default = Some(4)))]
    limit: Option<u32>,
}

#[derive(Clone, Patch, Config)]
#[config(default, update, owner_access, validate_builder, patch(validate = Self::validated, error = Error))]
struct Bounded {
    #[config(value, update, builder(default = 2), get(copy))]
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
    fn commit<C: UpdatableConfig>(config: &mut C, update: C::Update) -> Result<(), C::Error> {
        UpdatableConfig::apply_update(config, update)
    }
    assert!(
        commit(
            &mut bounded,
            BoundedUpdate {
                level: BoundedLevelUpdate::Set { value: 7 },
                ..BoundedUpdate::default()
            },
        )
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

#[derive(Clone, Config)]
#[config(update, patch(validate = Self::validated, error = Error))]
struct Mix {
    #[config(nested, update)]
    bounded: Bounded,
    #[config(value, update, get(copy))]
    gain: u32,
}

impl Mix {
    fn validated(self) -> Result<Self, Error> {
        if self.gain > self.bounded.level() {
            return Err(Error::other("gain exceeds the level"));
        }
        Ok(self)
    }
}

#[kithara::test]
fn a_nested_update_commits_through_both_checks_or_not_at_all() {
    let mut mix = Mix::builder().bounded(Bounded::default()).gain(2).build();
    let level = |value| BoundedUpdate {
        level: BoundedLevelUpdate::Set { value },
        ..BoundedUpdate::default()
    };

    let inner = mix.apply_update(MixUpdate {
        bounded: level(7),
        gain: MixGainUpdate::Set { value: 1 },
    });
    assert!(
        inner.is_err(),
        "the nested check refuses a level over its limit"
    );
    assert_eq!(
        (mix.bounded.level(), mix.gain()),
        (2, 2),
        "a nested refusal keeps the outer gain too"
    );

    let outer = mix.apply_update(MixUpdate {
        bounded: level(1),
        ..MixUpdate::default()
    });
    assert!(
        outer.is_err(),
        "the outer check refuses a level under the gain"
    );
    assert_eq!(
        mix.bounded.level(),
        2,
        "an outer refusal discards the change the nested check accepted"
    );

    mix.apply_update(MixUpdate {
        bounded: level(4),
        gain: MixGainUpdate::Set { value: 3 },
    })
    .expect("both checks accept");
    assert_eq!((mix.bounded.level(), mix.gain()), (4, 3));
    assert_eq!(
        mix.values().bounded.level,
        4,
        "the snapshot reads the nested change"
    );
}

#[derive(Config)]
#[config(update, builder(none))]
struct Stack {
    #[config(nested, update)]
    levels: Levels,
}

#[kithara::test]
fn an_unchecked_owner_takes_a_nested_update_in_place() {
    let mut stack = Stack {
        levels: Levels::default(),
    };
    stack.apply_update(StackUpdate {
        levels: LevelsUpdate {
            level: LevelsLevelUpdate::Set { value: 5 },
            ..LevelsUpdate::default()
        },
    });
    assert_eq!(stack.levels.level(), 5);
    assert_eq!(
        stack.values().levels.limit,
        Some(4),
        "an unchanged nested field keeps its value"
    );
}

#[derive(ConfigOwner)]
#[config_owner(config)]
#[config_owner_mut]
struct Owner {
    config: Bounded,
}

#[derive(ConfigOwner)]
#[config_owner(Bounded, inner.config)]
struct NestedOwner {
    inner: std::sync::Arc<Owner>,
}

#[derive(Config)]
#[config(owner_access, builder(none), fields(get(ref)))]
struct GenericResource<T>
where
    T: Send + Sync,
{
    #[config(skip = "borrowed by its runtime owner")]
    resource: T,
}

#[derive(ConfigOwner)]
#[config_owner(GenericResource<T>, inner.config)]
struct GenericOwner<T>
where
    T: Send + Sync,
{
    inner: Box<GenericInner<T>>,
}

struct GenericInner<T>
where
    T: Send + Sync,
{
    config: GenericResource<T>,
}

#[derive(Config)]
#[config(builder(none), fields(value, get(copy)))]
struct ValueFields {
    level: u32,
    #[config(skip = "runtime resource", get(ref))]
    resource: String,
}

#[derive(Config)]
#[config(builder(none), fields(nested, get(ref)))]
struct NestedFields {
    settings: ValueFields,
    #[config(value)]
    label: String,
}

#[derive(Config)]
#[config(construction)]
struct ConstructionInputs {
    resource: String,
    #[config(get(ref))]
    label: String,
}

#[kithara::test]
fn construction_inputs_need_no_field_exclusions() {
    let inputs = ConstructionInputs::builder()
        .resource(String::from("owned"))
        .label(String::from("label"))
        .build();
    assert_eq!(inputs.resource, "owned");
    assert_eq!(inputs.label(), "label");
}

#[kithara::test]
fn type_level_value_role_allows_explicit_resource_exclusion() {
    let config = ValueFields {
        level: 3,
        resource: String::from("owned"),
    };
    assert_eq!(config.level(), 3);
    assert_eq!(config.values().level, 3);
    assert_eq!(config.resource, "owned");
    assert!(std::ptr::eq(config.resource(), &config.resource));
}

#[kithara::test]
fn nested_field_defaults_preserve_value_overrides_and_borrowed_getters() {
    let config = NestedFields {
        settings: ValueFields {
            level: 5,
            resource: String::from("owned"),
        },
        label: String::from("nested"),
    };
    assert!(std::ptr::eq(config.settings(), &config.settings));
    assert!(std::ptr::eq(config.label(), &config.label));
    let values = config.values();
    assert_eq!(values.settings.level, 5);
    assert_eq!(values.label, "nested");
}

#[kithara::test]
fn derived_owners_borrow_the_same_updated_config_through_nested_fields() {
    let mut owner = Owner {
        config: Bounded::default(),
    };
    owner
        .apply_config_update(BoundedUpdate {
            level: BoundedLevelUpdate::Set { value: 3 },
            ..BoundedUpdate::default()
        })
        .expect("level stays within the limit");
    assert!(std::ptr::eq(owner.config(), &owner.config));
    assert_eq!(owner.level(), 3);
    assert!(
        owner
            .apply_config_update(BoundedUpdate {
                level: BoundedLevelUpdate::Set { value: 7 },
                ..BoundedUpdate::default()
            })
            .is_err()
    );
    assert_eq!(owner.level(), 3);

    let nested = NestedOwner {
        inner: std::sync::Arc::new(owner),
    };
    assert!(std::ptr::eq(nested.config(), &nested.inner.config));
    assert_eq!(nested.level(), 3);
    assert_eq!(nested.config().values().level, 3);
}

#[kithara::test]
fn generic_owner_access_borrows_the_original_resource() {
    let owner = GenericOwner {
        inner: Box::new(GenericInner {
            config: GenericResource {
                resource: String::from("owned"),
            },
        }),
    };
    assert!(std::ptr::eq(owner.resource(), &owner.inner.config.resource));
}

#[derive(Config)]
#[config(debug, fields(nested))]
struct Session<'a> {
    #[config(skip = "borrowed construction resource", debug(skip))]
    resource: &'a str,
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
#[config(fields(builder(default)))]
struct WrappedConfig {
    #[config(
        value(u32, self.level.0),
        wrap(default = 2, with = Wrapped::new),
        patch(wire = u32, from = Wrapped::new)
    )]
    level: Wrapped<u32>,
}

#[derive(Config, Patch)]
#[config(
    default,
    debug,
    update,
    fields(
        value,
        get(copy),
        builder(default = 2),
        update,
        patch(skip),
        debug(skip)
    )
)]
struct SharedOptions {
    first: u32,
    #[config(builder(default = 3), patch(attribute(serde(rename = "level"))))]
    second: u32,
    #[config(
        skip = "owned runtime resource",
        get(skip),
        builder(default),
        update(false)
    )]
    resource: String,
}

#[kithara::test]
fn shared_field_options_keep_builders_updates_and_patch_exclusions_independent() {
    let mut config = SharedOptions::default();
    assert_eq!(config.first(), 2);
    assert_eq!(config.second(), 3);
    assert_eq!(format!("{config:?}"), "SharedOptions { .. }");
    config.apply(SharedOptionsPatch { second: Some(9) });
    assert_eq!(config.second(), 9);
    config.apply_update(SharedOptionsUpdate {
        first: SharedOptionsFirstUpdate::Set { value: 8 },
        ..SharedOptionsUpdate::default()
    });
    assert_eq!(config.first(), 8);
    config.apply_update(SharedOptionsUpdate {
        first: SharedOptionsFirstUpdate::Reset,
        second: SharedOptionsSecondUpdate::Reset,
    });
    assert_eq!(config.first(), 2);
    assert_eq!(config.second(), 3);
    assert!(config.resource.is_empty());
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
