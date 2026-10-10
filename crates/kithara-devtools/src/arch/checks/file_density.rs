use anyhow::{Context as _, Result};

use super::{Check, Context};
use crate::common::{
    parse::count_items,
    violation::Violation,
    walker::{relative_to, workspace_rs_files_scoped},
};

pub(crate) mod consts {
    pub(crate) const ID: &str = "file_density";
}

pub(crate) struct FileDensity;

impl Check for FileDensity {
    fn id(&self) -> &'static str {
        consts::ID
    }

    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>> {
        let cfg = &ctx.config.thresholds.file_density;
        let mut violations = Vec::new();

        for path in workspace_rs_files_scoped(ctx.workspace_root, ctx.scope)? {
            let Some(file) = ctx.parsed_file(&path)? else {
                continue;
            };
            let stats = count_items(file);
            if stats.fns < cfg.min_fns_to_evaluate {
                continue;
            }
            let denom = u32::try_from(stats.types.max(1))
                .context("file_density: types count overflows u32")?;
            let numer =
                u32::try_from(stats.fns).context("file_density: fns count overflows u32")?;
            let ratio = f64::from(numer) / f64::from(denom);
            let key = relative_to(ctx.workspace_root, &path)
                .to_string_lossy()
                .replace('\\', "/");
            let msg = format!(
                "{} fns / {} types (ratio {:.1})",
                stats.fns, stats.types, ratio
            );
            if ratio >= cfg.deny_fns_per_type {
                violations.push(Violation::deny(consts::ID, key, msg));
            } else if ratio >= cfg.warn_fns_per_type {
                violations.push(Violation::warn(consts::ID, key, msg));
            }
        }
        Ok(violations)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cargo_metadata::MetadataCommand;

    use super::*;
    use crate::{
        arch::{
            checks::{fn_arg_count::FnArgCount, no_lib_statics::NoLibStatics},
            config::ArchConfig,
        },
        common::scope::Scope,
    };

    #[test]
    fn checks_measure_only_production_items_before_aggregation() {
        let dir = tempfile::tempdir().expect("temporary workspace");
        let crate_root = dir.path().join("crates/fixture");
        let src = crate_root.join("src");
        fs::create_dir_all(&src).expect("create source directory");
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/fixture\"]\nresolver = \"2\"\n",
        )
        .expect("write workspace manifest");
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write crate manifest");
        fs::write(
            src.join("lib.rs"),
            r#"
struct Real;
impl Real { fn production(&self, a: u8) {} }
#[cfg(test)]
mod fixtures {
    fn helper(a: u8, b: u8) {}
    fn second() {}
    static FIXTURE: u8 = 0;
}
"#,
        )
        .expect("write crate source");
        fs::write(
            src.join("fixture.rs"),
            "#![cfg(test)]\nstatic FIXTURE: u8 = 0;\nfn helper(a: u8, b: u8) {}\nfn second() {}",
        )
        .expect("write test-only source");
        let metadata = MetadataCommand::new()
            .manifest_path(dir.path().join("Cargo.toml"))
            .no_deps()
            .exec()
            .expect("fixture cargo metadata");
        let mut config = ArchConfig::default();
        config.thresholds.file_density.min_fns_to_evaluate = 2;
        config.thresholds.file_density.warn_fns_per_type = 2.0;
        config.thresholds.fn_arg_count.warn = 2;
        let scope = Scope::default();
        let ctx = Context::new(&config, &metadata, dir.path(), &scope);

        assert!(FileDensity.run(&ctx).expect("density findings").is_empty());
        assert!(FnArgCount.run(&ctx).expect("argument findings").is_empty());
        assert!(NoLibStatics.run(&ctx).expect("global findings").is_empty());
    }
}
