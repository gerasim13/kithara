use std::io::Error;

use kithara_config::{Config, ConfigOwner, ConfigOwnerMut, Patch, UpdatableConfig};
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
#[config(default, update, owner_access, validate_builder, patch(validate = Self::validated, error = Error))]
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
#[config(owner_access, builder(none))]
struct GenericResource<T>
where
    T: Send + Sync,
{
    #[config(skip = "borrowed by its runtime owner", field(get))]
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
#[config(builder(none), fields(value))]
struct ValueFields {
    #[config(field(get, copy))]
    level: u32,
    #[config(skip = "runtime resource")]
    resource: String,
}

#[derive(Config)]
#[config(construction)]
struct ConstructionInputs {
    resource: String,
    #[config(field(get))]
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
