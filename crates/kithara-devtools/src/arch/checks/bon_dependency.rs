use anyhow::Result;
use cargo_metadata::Dependency;

use super::{Check, Context};
use crate::common::{scope::packages_in_scope, violation::Violation};

const ID: &str = "bon_dependency";

pub(crate) struct BonDependency;

impl Check for BonDependency {
    fn id(&self) -> &'static str {
        ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        Ok(packages_in_scope(ctx.metadata, ctx.scope)
            .into_iter()
            .filter(|package| forbidden(package.name.as_str(), &package.dependencies))
            .map(|package| {
                Violation::deny(
                    ID,
                    format!("{}::bon", package.name),
                    "direct bon dependency; use kithara-config's Config or bon facade",
                )
            })
            .collect())
    }
}

fn forbidden(package: &str, dependencies: &[Dependency]) -> bool {
    package != "kithara-config"
        && dependencies
            .iter()
            .any(|dependency| dependency.name == "bon")
}

#[cfg(test)]
mod tests {
    use cargo_metadata::{DependencyKind, MetadataCommand};

    use super::forbidden;

    #[test]
    fn rejects_every_direct_kind_target_and_alias() {
        let metadata = MetadataCommand::new()
            .no_deps()
            .exec()
            .expect("workspace metadata");
        let config = metadata
            .packages
            .iter()
            .find(|package| package.name == "kithara-config")
            .expect("config package");
        let mut bon = config
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "bon")
            .expect("config owns bon")
            .clone();

        assert!(!forbidden("kithara-config", &[bon.clone()]));
        for kind in [
            DependencyKind::Normal,
            DependencyKind::Development,
            DependencyKind::Build,
        ] {
            bon.kind = kind;
            bon.rename = Some("builder_alias".to_owned());
            bon.target = Some("cfg(target_arch = \"wasm32\")".parse().expect("target"));
            assert!(forbidden("other-package", &[bon.clone()]));
        }
        assert!(!forbidden("other-package", &[]));
        assert!(!forbidden(
            "other-package",
            &config
                .dependencies
                .iter()
                .filter(|dependency| dependency.name != "bon")
                .cloned()
                .collect::<Vec<_>>(),
        ));
    }
}
