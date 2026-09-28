use std::io::Error;

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

#[config(default, update)]
#[derive(Clone, Patch)]
#[patch(validate = Self::validated, error = Error)]
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

#[config]
struct Session<'a> {
    #[config(skip = "borrowed construction resource")]
    resource: &'a str,
    #[config(nested)]
    levels: Levels,
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
