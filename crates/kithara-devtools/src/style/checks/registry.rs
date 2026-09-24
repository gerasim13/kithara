use std::path::Path;

use anyhow::Result;
use cargo_metadata::Metadata;

use super::{
    super::config::StyleConfig, comment_hygiene, const_locality, dead_doc_refs,
    declaration_spacing, doc_size, doc_staleness, non_english_text, qualified_path_depth,
    readme_shape, split_module, struct_field_order, struct_init_order, thin_module_dir,
    trait_item_order,
};
use crate::common::{fix::FixOutcome, scan::Scan, scope::Scope, violation::Violation};

pub(crate) struct Context<'a> {
    pub(crate) workspace_root: &'a Path,
    pub(crate) metadata: &'a Metadata,
    pub(crate) scan: &'a Scan,
    pub(crate) scope: &'a Scope,
    pub(crate) config: &'a StyleConfig,
}

pub(crate) trait Check: Sync {
    /// Apply the check's autofix in place. Default: no autofix; the
    /// violation stays in the report and the user resolves it manually.
    /// Implementations must uphold the four invariants from
    /// `xtask/src/common/fix/README.md` (I1-I4).
    fn fix(&self, _ctx: &Context<'_>) -> Result<FixOutcome> {
        Ok(FixOutcome {
            writes: 0,
            skipped: vec![format!("check '{}' has no autofix", self.id())],
            changes: Vec::new(),
        })
    }
    fn id(&self) -> &'static str;
    fn run(&self, ctx: &Context<'_>) -> Result<Vec<Violation>>;

    fn uses_global_lint_excludes(&self) -> bool {
        true
    }
}

pub(crate) fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(comment_hygiene::CommentHygiene),
        Box::new(const_locality::ConstLocality),
        Box::new(dead_doc_refs::DeadDocRefs),
        Box::new(declaration_spacing::DeclarationSpacing),
        Box::new(doc_size::DocSize),
        Box::new(doc_staleness::DocStaleness),
        Box::new(non_english_text::NonEnglishText),
        Box::new(qualified_path_depth::QualifiedPathDepth),
        Box::new(readme_shape::ReadmeShape),
        Box::new(split_module::SplitModule),
        Box::new(struct_field_order::StructFieldOrder),
        Box::new(trait_item_order::TraitItemOrder),
        Box::new(struct_init_order::StructInitOrder),
        Box::new(thin_module_dir::ThinModuleDir),
    ]
}
