use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Clone, kithara_config::Config)]
#[config(builder(none))]
pub(crate) struct ArchConfig {
    #[config(nested)]
    pub(crate) canonical_types: CanonicalTypesConfig,
    #[config(nested)]
    pub(crate) direction: DirectionConfig,
    #[config(nested)]
    pub(crate) module_layers: ModuleLayersConfig,
    #[config(nested)]
    pub(crate) thresholds: ThresholdsConfig,
}

impl ArchConfig {
    pub(crate) fn load(dir: &Path) -> Result<Self> {
        Ok(Self {
            direction: load_optional(&dir.join("direction.toml"))?,
            canonical_types: load_optional(&dir.join("canonical-types.toml"))?,
            thresholds: load_optional(&dir.join("thresholds.toml"))?,
            module_layers: load_optional(&dir.join("module-layers.toml"))?,
        })
    }
}

#[derive(Debug, Default, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct DirectionConfig {
    #[serde(default)]
    #[config(value)]
    pub(crate) exemptions: BTreeMap<String, String>,
    #[serde(default, rename = "layer")]
    #[config(value)]
    pub(crate) layers: Vec<Layer>,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct Layer {
    #[config(value)]
    pub(crate) name: String,
    #[config(value)]
    pub(crate) crates: Vec<String>,
    #[config(value)]
    pub(crate) index: u32,
}

#[derive(Debug, Default, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct CanonicalTypesConfig {
    #[serde(default, rename = "canonical")]
    #[config(value)]
    pub(crate) entries: Vec<CanonicalType>,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct CanonicalType {
    #[config(value)]
    pub(crate) kind: String,
    #[config(value)]
    pub(crate) name: String,
    #[config(value)]
    pub(crate) owner: String,
}

#[derive(Debug, Default, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ThresholdsConfig {
    #[serde(default)]
    #[config(nested)]
    pub(crate) arc_clone_hotspots: ArcCloneHotspotsThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) args_wrapper_struct: ArgsWrapperStructThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) cancel_root_sites: CancelRootSitesThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) cfg_density: CfgDensityThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) dead_exports: DeadExportsThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) field_always_constant: FieldAlwaysConstantThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) field_always_equals_other_field: FieldAlwaysEqualsOtherFieldThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) field_passthrough: FieldPassthroughThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) file_density: FileDensityThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) file_size: FileSizeThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) firewheel_dsp_facade: FirewheelDspFacadeThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) flat_directory: FlatDirectoryThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) fn_arg_count: FnArgCountThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) generic_param_count: GenericParamCountThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) god_module: GodModuleThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) god_struct: GodStructThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) god_trait: GodTraitThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) max_nesting: MaxNestingThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) mixed_entities: MixedEntitiesThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) module_fan_out: ModuleFanOutThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) multi_constructor: MultiConstructorThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) no_lib_statics: NoLibStaticsThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) platform_layer_hygiene: PlatformLayerHygieneThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) pub_struct_open_fields: PubStructOpenFieldsThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) readme_presence: ReadmePresenceThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) redundant_accessors: RedundantAccessorsThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) redundant_reexport: RedundantReexportThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) shared_state: SharedStateThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) single_impl_size: SingleImplSizeThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) single_word_filenames: SingleWordFilenamesThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) smoothing_primitive_sites: SmoothingPrimitiveSitesThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) tokio_dep_quarantine: TokioDepQuarantineThreshold,
    #[serde(default)]
    #[config(nested)]
    pub(crate) trait_impl_count: TraitImplCountThreshold,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct MultiConstructorThreshold {
    /// Names that are always considered the canonical constructor.
    #[serde(default = "default_canonical_ctor_names")]
    #[config(value)]
    pub(crate) canonical_names: Vec<String>,
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_files: Vec<String>,
}

fn default_canonical_ctor_names() -> Vec<String> {
    ["new", "default"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for MultiConstructorThreshold {
    fn default() -> Self {
        Self {
            canonical_names: default_canonical_ctor_names(),
            exempt_files: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FieldPassthroughThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_files: Vec<String>,
    /// Types a field may be wrapped in without the wrapper counting as a level
    /// of its own.
    #[serde(default = "default_transparent_wrappers")]
    #[config(value)]
    pub(crate) transparent_wrappers: Vec<String>,
    /// How far a passthrough chain is followed before the check gives up.
    #[serde(default = "default_field_passthrough_max_depth")]
    #[config(value)]
    pub(crate) max_depth: usize,
}

const fn default_field_passthrough_max_depth() -> usize {
    8
}

fn default_transparent_wrappers() -> Vec<String> {
    ["Arc", "Rc", "Box", "RefCell", "Cell", "Mutex", "RwLock"]
        .map(String::from)
        .to_vec()
}

impl Default for FieldPassthroughThreshold {
    fn default() -> Self {
        Self {
            exempt_files: Vec::new(),
            max_depth: default_field_passthrough_max_depth(),
            transparent_wrappers: default_transparent_wrappers(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ArgsWrapperStructThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_files: Vec<String>,
    #[serde(default = "default_args_wrapper_min_call_sites")]
    #[config(value)]
    pub(crate) min_call_sites: usize,
    #[serde(default = "default_args_wrapper_min_fields")]
    #[config(value)]
    pub(crate) min_fields: usize,
}

impl Default for ArgsWrapperStructThreshold {
    fn default() -> Self {
        Self {
            min_fields: default_args_wrapper_min_fields(),
            min_call_sites: default_args_wrapper_min_call_sites(),
            exempt_files: Vec::new(),
        }
    }
}

const fn default_args_wrapper_min_fields() -> usize {
    5
}

const fn default_args_wrapper_min_call_sites() -> usize {
    2
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FieldAlwaysConstantThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_files: Vec<String>,
    #[serde(default = "default_field_always_min_call_sites")]
    #[config(value)]
    pub(crate) min_call_sites: usize,
}

impl Default for FieldAlwaysConstantThreshold {
    fn default() -> Self {
        Self {
            min_call_sites: default_field_always_min_call_sites(),
            exempt_files: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FieldAlwaysEqualsOtherFieldThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_files: Vec<String>,
    #[serde(default = "default_field_always_min_call_sites")]
    #[config(value)]
    pub(crate) min_call_sites: usize,
}

impl Default for FieldAlwaysEqualsOtherFieldThreshold {
    fn default() -> Self {
        Self {
            min_call_sites: default_field_always_min_call_sites(),
            exempt_files: Vec::new(),
        }
    }
}

const fn default_field_always_min_call_sites() -> usize {
    3
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct RedundantReexportThreshold {
    /// Sub-checks to run: `explicit_duplicate` (R1) and `associated_type_leak` (R2).
    #[serde(default = "default_redundant_reexport_detect")]
    #[config(value)]
    pub(crate) detect: Vec<String>,
    /// Type names to exempt (canonical without crate prefix, e.g. `FileConfig`).
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt: Vec<String>,
}

impl Default for RedundantReexportThreshold {
    fn default() -> Self {
        Self {
            detect: default_redundant_reexport_detect(),
            exempt: Vec::new(),
        }
    }
}

fn default_redundant_reexport_detect() -> Vec<String> {
    ["explicit_duplicate", "associated_type_leak"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct CfgDensityThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exclude_globs: Vec<String>,
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
    #[config(value)]
    pub(crate) deny: usize,
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for CfgDensityThreshold {
    fn default() -> Self {
        Self {
            warn: 5,
            deny: 10,
            exempt_crates: Vec::new(),
            exclude_globs: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FileSizeThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exclude_globs: Vec<String>,
    #[config(value)]
    pub(crate) deny: usize,
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for FileSizeThreshold {
    fn default() -> Self {
        Self {
            warn: 400,
            deny: 1000,
            exclude_globs: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FileDensityThreshold {
    #[config(value)]
    pub(crate) deny_fns_per_type: f64,
    #[config(value)]
    pub(crate) warn_fns_per_type: f64,
    #[config(value)]
    pub(crate) min_fns_to_evaluate: usize,
}

impl Default for FileDensityThreshold {
    fn default() -> Self {
        Self {
            warn_fns_per_type: 25.0,
            deny_fns_per_type: 40.0,
            min_fns_to_evaluate: 20,
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct SharedStateThreshold {
    #[serde(default = "default_shared_state_patterns")]
    #[config(value)]
    pub(crate) patterns: Vec<String>,
    #[config(value)]
    pub(crate) deny: usize,
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for SharedStateThreshold {
    fn default() -> Self {
        Self {
            warn: 3,
            deny: 5,
            patterns: default_shared_state_patterns(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ArcCloneHotspotsThreshold {
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for ArcCloneHotspotsThreshold {
    fn default() -> Self {
        Self { warn: 3 }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct GodModuleThreshold {
    /// Per-crate override map: crate-name → custom warn threshold. Lets
    /// app/test/macro crates relax the default without baselining each file.
    #[serde(default)]
    #[config(value)]
    pub(crate) overrides: BTreeMap<String, usize>,
    /// Default warn threshold = number of `pub`/`pub(crate)` items in one
    /// module file that triggers a violation.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for GodModuleThreshold {
    fn default() -> Self {
        Self {
            warn: 8,
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct GodStructThreshold {
    /// Trait names whose impl methods are standard conformance (idiomatic
    /// plumbing), not accreted responsibility — `Drop`, `Default`, `From`,
    /// arithmetic, ordering, IO adapters, ... Matched on the trait's last
    /// path segment. Methods from these impls do not count toward the total;
    /// domain-trait methods still do.
    #[serde(default = "default_std_traits")]
    #[config(value)]
    pub(crate) std_traits: Vec<String>,
    /// Substantial methods per type, aggregated across every `impl` block of
    /// the owning crate. "Substantial" excludes thin forwarders/accessors
    /// (short, branch-free bodies — idiomatic facade plumbing), `#[cfg(test)]`
    /// items, and methods that only satisfy a standard-library / language
    /// trait contract (see `std_traits`). Field count is reported for context
    /// but is owned by `pub_struct_open_fields`, not summed in here — so this
    /// check measures behaviour concentration, not state size or API breadth.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for GodStructThreshold {
    fn default() -> Self {
        Self {
            warn: 15,
            std_traits: default_std_traits(),
        }
    }
}

fn default_std_traits() -> Vec<String> {
    [
        "Drop",
        "Default",
        "Clone",
        "Copy",
        "Debug",
        "Display",
        "PartialEq",
        "Eq",
        "PartialOrd",
        "Ord",
        "Hash",
        "From",
        "Into",
        "TryFrom",
        "TryInto",
        "AsRef",
        "AsMut",
        "Borrow",
        "BorrowMut",
        "Deref",
        "DerefMut",
        "Add",
        "Sub",
        "Mul",
        "Div",
        "Rem",
        "Neg",
        "AddAssign",
        "SubAssign",
        "MulAssign",
        "DivAssign",
        "RemAssign",
        "Not",
        "BitAnd",
        "BitOr",
        "BitXor",
        "Shl",
        "Shr",
        "BitAndAssign",
        "BitOrAssign",
        "BitXorAssign",
        "ShlAssign",
        "ShrAssign",
        "Index",
        "IndexMut",
        "Read",
        "Write",
        "Seek",
        "BufRead",
        "Iterator",
        "IntoIterator",
        "DoubleEndedIterator",
        "ExactSizeIterator",
        "FromIterator",
        "Extend",
        "FromStr",
        "ToString",
        "ToOwned",
        "Send",
        "Sync",
        "Unpin",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct GodTraitThreshold {
    /// Number of method (`fn`) items in a single trait definition.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for GodTraitThreshold {
    fn default() -> Self {
        Self { warn: 7 }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct PubStructOpenFieldsThreshold {
    /// `pub` structs with at least this many `pub` fields are flagged. Signals
    /// missing invariants / direct mutation. Candidate for a builder or
    /// encapsulated setter API.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for PubStructOpenFieldsThreshold {
    fn default() -> Self {
        Self { warn: 3 }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FnArgCountThreshold {
    /// Functions with this many arguments (excluding `self`) trigger a warn.
    /// Tighter than clippy's `too_many_arguments` default (7) — used as a
    /// map of the codebase, not an enforced gate.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for FnArgCountThreshold {
    fn default() -> Self {
        Self { warn: 5 }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ModuleFanOutThreshold {
    /// Per-crate override map: crate-name → custom warn threshold.
    #[serde(default)]
    #[config(value)]
    pub(crate) overrides: BTreeMap<String, usize>,
    /// Default warn threshold = number of distinct intra-crate sibling
    /// modules a single file imports from before being flagged as an
    /// orchestrator candidate.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for ModuleFanOutThreshold {
    fn default() -> Self {
        Self {
            warn: 4,
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct NoLibStaticsThreshold {
    /// Crate names exempt from the rule. App / FFI / wasm / xtask / test
    /// support crates legitimately own singletons; lib crates do not.
    /// Project-specific — supplied via config.
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct PlatformLayerHygieneThreshold {
    /// Source root whose files this check governs, workspace-relative and
    /// trailing-slashed.
    #[serde(default = "default_platform_root")]
    #[config(value)]
    pub(crate) root: String,
    /// Files sanctioned to name `std::sync::Arc` directly: the ownership
    /// primitive itself and the wasm shim that has no platform Arc to reach
    /// for.
    #[serde(default = "default_arc_owner_files")]
    #[config(value)]
    pub(crate) arc_owner_files: Vec<String>,
    /// Sub-directories of the root the check does not read: the backends and
    /// the platform-specific trees whose job is to name the primitives the
    /// rest of the crate must not.
    #[serde(default = "default_platform_excluded_subtrees")]
    #[config(value)]
    pub(crate) excluded_subtrees: Vec<String>,
    /// Sub-paths that implement the abstraction rather than consume it.
    #[serde(default = "default_platform_impl_subtrees")]
    #[config(value)]
    pub(crate) impl_subtrees: Vec<String>,
}

fn default_arc_owner_files() -> Vec<String> {
    [
        "crates/kithara-platform/src/system/ownership.rs",
        "crates/kithara-platform/src/wasm/sync/mod.rs",
    ]
    .map(String::from)
    .to_vec()
}

fn default_platform_excluded_subtrees() -> Vec<String> {
    ["backend/", "system/", "loom/", "wasm/"]
        .map(String::from)
        .to_vec()
}

fn default_platform_impl_subtrees() -> Vec<String> {
    [
        "flash/sync/",
        "flash/tokio/",
        "flash/system/",
        "common/cancel/",
        "common/time.rs",
    ]
    .map(String::from)
    .to_vec()
}

fn default_platform_root() -> String {
    "crates/kithara-platform/src/".to_owned()
}

impl Default for PlatformLayerHygieneThreshold {
    fn default() -> Self {
        Self {
            arc_owner_files: default_arc_owner_files(),
            excluded_subtrees: default_platform_excluded_subtrees(),
            impl_subtrees: default_platform_impl_subtrees(),
            root: default_platform_root(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct CancelRootSitesThreshold {
    /// Relative file paths where minting a fresh cancel root
    /// (`CancelToken::root` / `CancelToken::never`) is sanctioned: consumer-crate
    /// owner tops, FFI bridges, `CancelScope`, and the dedicated sentinel / latch
    /// sites.
    #[serde(default)]
    #[config(value)]
    pub(crate) allowed_files: Vec<String>,
    /// Crates whose production *is* test scaffolding (helpers, mocks). Their
    /// hard-coded `CancelToken::root()` / `CancelToken::never()` calls are
    /// indistinguishable from test fixtures and don't root an orphan tree.
    /// Project-specific — supplied via config.
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
    /// Fresh-root minting calls denied outside the allowlist: the owning-master
    /// `CancelToken::root` and the never-cancelled sentinel
    /// `CancelToken::never`. Both root a new cancel tree; `.child()` — the
    /// sanctioned derivation — is not one of these.
    #[serde(default = "default_cancel_root_patterns")]
    #[config(value)]
    pub(crate) patterns: Vec<String>,
}

fn default_cancel_root_patterns() -> Vec<String> {
    ["CancelToken::root", "CancelToken::never"]
        .map(String::from)
        .to_vec()
}

impl Default for CancelRootSitesThreshold {
    fn default() -> Self {
        Self {
            allowed_files: Vec::new(),
            exempt_crates: Vec::new(),
            patterns: default_cancel_root_patterns(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct SmoothingPrimitiveSitesThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) allowed_files: Vec<String>,
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
    #[serde(default = "default_smoothing_primitive_patterns")]
    #[config(value)]
    pub(crate) patterns: Vec<String>,
}

fn default_smoothing_primitive_patterns() -> Vec<String> {
    ["SmoothingFilter", "one_pole", "OnePole", "smoothing_coeff"]
        .map(String::from)
        .to_vec()
}

impl Default for SmoothingPrimitiveSitesThreshold {
    fn default() -> Self {
        Self {
            allowed_files: Vec::new(),
            exempt_crates: Vec::new(),
            patterns: default_smoothing_primitive_patterns(),
        }
    }
}

/// Files allowed to name firewheel's DSP helpers directly, and the helper
/// modules the facade owns.
#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FirewheelDspFacadeThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) allowed_files: Vec<String>,
    #[serde(default = "default_firewheel_facade_modules")]
    #[config(value)]
    pub(crate) modules: Vec<String>,
}

fn default_firewheel_facade_modules() -> Vec<String> {
    [
        "dsp::fade",
        "dsp::mix",
        "dsp::filter::smoothing_filter",
        "param::smoother",
    ]
    .map(String::from)
    .to_vec()
}

impl Default for FirewheelDspFacadeThreshold {
    fn default() -> Self {
        Self {
            allowed_files: Vec::new(),
            modules: default_firewheel_facade_modules(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct TokioDepQuarantineThreshold {
    /// Crates whose *production* tokio coupling is not yet migrated to the
    /// platform re-exports (W6 quarantine debt). Entries here are tracked work
    /// to remove, not a standing exemption — adding a NEW crate with direct
    /// production tokio still fails the gate.
    #[serde(default)]
    #[config(value)]
    pub(crate) allowed_crates: Vec<String>,
    /// Crates whose direct tokio dependency is never flagged: the
    /// workspace-hack feature-unification shim (must name every transitive
    /// dep) and the test-support crates (their tokio is test scaffolding).
    /// Project-specific — supplied via config.
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
    /// Crate names quarantined: a *production* (non-dev, non-build) dependency
    /// on any of these is forbidden outside the platform owner / exemptions.
    #[serde(default = "default_quarantined_crates")]
    #[config(value)]
    pub(crate) quarantined: Vec<String>,
}

fn default_quarantined_crates() -> Vec<String> {
    ["tokio", "tokio-util", "tokio-stream"]
        .map(String::from)
        .to_vec()
}

impl Default for TokioDepQuarantineThreshold {
    fn default() -> Self {
        Self {
            allowed_crates: Vec::new(),
            exempt_crates: Vec::new(),
            quarantined: default_quarantined_crates(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct DeadExportsThreshold {
    /// Symbol names that are never flagged (e.g. FFI entry points the scanner
    /// cannot see called from non-Rust callers).
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt: Vec<String>,
    /// Attribute path segments that mark a definition as an external entry
    /// point with no in-tree Rust caller (`no_mangle`, `wasm_bindgen`, ...).
    /// Configured in `.config/arch/thresholds.toml` (`[dead_exports]`).
    #[serde(default)]
    #[config(value)]
    pub(crate) export_attrs: Vec<String>,
    /// Workspace-relative path fragments whose definitions `--fix` must never
    /// auto-delete: platform-gated module directories (e.g. `android/`) whose
    /// callers live in build configurations or non-Rust code this scan cannot
    /// see. Reporting still flags them; only autofix is held back.
    #[serde(default)]
    #[config(value)]
    pub(crate) fix_protect_paths: Vec<String>,
    /// Crates skipped entirely (neither defs collected nor refs counted):
    /// build tooling and workspace-hack shims. Configured in
    /// `.config/arch/thresholds.toml` (`[dead_exports]`).
    #[serde(default)]
    #[config(value)]
    pub(crate) ignore_crates: Vec<String>,
    /// Definition kinds to evaluate: `fn`, `method`, `const`, `static`,
    /// `type`, `struct`, `enum`, `trait`.
    #[serde(default = "default_dead_exports_kinds")]
    #[config(value)]
    pub(crate) kinds: Vec<String>,
    /// Crates treated as test-only scaffolding (integration harness, test-util
    /// and test-macro crates). Any reference originating in one of these counts
    /// as a test reference. Configured in
    /// `.config/arch/thresholds.toml` (`[dead_exports]`).
    #[serde(default)]
    #[config(value)]
    pub(crate) test_crates: Vec<String>,
}

impl Default for DeadExportsThreshold {
    fn default() -> Self {
        Self {
            kinds: default_dead_exports_kinds(),
            test_crates: Vec::new(),
            ignore_crates: Vec::new(),
            exempt: Vec::new(),
            fix_protect_paths: Vec::new(),
            export_attrs: Vec::new(),
        }
    }
}

fn default_dead_exports_kinds() -> Vec<String> {
    [
        "fn", "method", "const", "static", "type", "struct", "enum", "trait",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct GenericParamCountThreshold {
    /// Items with this many generic params (type+lifetime+const) trigger a warn.
    #[config(value)]
    pub(crate) warn_params: usize,
    /// Items with this many where-clause predicates trigger a warn.
    #[config(value)]
    pub(crate) warn_where: usize,
}

impl Default for GenericParamCountThreshold {
    fn default() -> Self {
        Self {
            warn_params: 3,
            warn_where: 4,
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct TraitImplCountThreshold {
    /// Number of `impl Trait for X` blocks targeting one local type
    /// (per file, since the AST view is per-file). High count → god-type
    /// implementing too many roles.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for TraitImplCountThreshold {
    fn default() -> Self {
        Self { warn: 6 }
    }
}

fn default_shared_state_patterns() -> Vec<String> {
    vec![
        "Arc<Mutex<".to_string(),
        "Arc<RwLock<".to_string(),
        "Arc<parking_lot::Mutex<".to_string(),
        "Arc<parking_lot::RwLock<".to_string(),
        "Arc<tokio::sync::Mutex<".to_string(),
        "Arc<tokio::sync::RwLock<".to_string(),
    ]
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct FlatDirectoryThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) ignore_globs: Vec<String>,
    #[config(value)]
    pub(crate) deny: usize,
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for FlatDirectoryThreshold {
    fn default() -> Self {
        Self {
            warn: 7,
            deny: 12,
            ignore_globs: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct MaxNestingThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_crates: Vec<String>,
    #[config(value)]
    pub(crate) max_depth: usize,
}

impl Default for MaxNestingThreshold {
    fn default() -> Self {
        Self {
            max_depth: 2,
            exempt_crates: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ReadmePresenceThreshold {
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt: Vec<String>,
    #[config(value)]
    pub(crate) min_bytes: u64,
}

impl Default for ReadmePresenceThreshold {
    fn default() -> Self {
        Self {
            exempt: Vec::new(),
            min_bytes: 200,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AccessorSeverity {
    Off,
    Warn,
    Deny,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct SingleImplSizeThreshold {
    /// Hard threshold: deny at this many lines.
    #[config(value)]
    pub(crate) deny_lines: usize,
    /// Soft threshold: warn at this many lines spanned by a single `impl`
    /// block (own or trait impl).
    #[config(value)]
    pub(crate) warn_lines: usize,
}

impl Default for SingleImplSizeThreshold {
    fn default() -> Self {
        Self {
            warn_lines: 200,
            deny_lines: 400,
        }
    }
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct SingleWordFilenamesThreshold {
    /// `warn` (track via baseline) or `deny` (fail on new offenders) or `off`.
    #[config(value)]
    pub(crate) severity: AccessorSeverity,
    /// Filenames that always pass regardless of word count
    /// (`mod.rs`, `lib.rs`, `main.rs`, `build.rs`).
    #[config(value)]
    pub(crate) exempt_filenames: Vec<String>,
    /// Glob patterns of paths to skip entirely (e.g. `**/tests/**`).
    #[serde(default)]
    #[config(value)]
    pub(crate) exempt_globs: Vec<String>,
    /// Maximum number of `_`-separated tokens in the filename stem.
    /// Default 1 = filenames must be a single word (`peer.rs`, `source.rs`).
    /// `stream_type.rs` has 2 tokens; `a_b_c.rs` has 3.
    #[config(value)]
    pub(crate) max_words: usize,
}

impl Default for SingleWordFilenamesThreshold {
    fn default() -> Self {
        Self {
            max_words: 1,
            exempt_filenames: default_single_word_exempt_filenames(),
            exempt_globs: Vec::new(),
            severity: AccessorSeverity::Warn,
        }
    }
}

fn default_single_word_exempt_filenames() -> Vec<String> {
    ["mod.rs", "lib.rs", "main.rs", "build.rs"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct RedundantAccessorsThreshold {
    #[config(value)]
    pub(crate) p1_severity: AccessorSeverity,
    #[config(value)]
    pub(crate) p2_severity: AccessorSeverity,
    #[config(value)]
    pub(crate) p3_severity: AccessorSeverity,
    #[config(value)]
    pub(crate) p4_severity: AccessorSeverity,
    /// 0-arg methods that expose internal data (`as_ref`, `as_str`, `lock`,
    /// `borrow`, `read`, ...). The receiver chain is treated as the data path.
    /// `_mut` variants (`as_mut`, `borrow_mut`, `deref_mut`, `get_mut`,
    /// `write`) are emitted with `RefMut` access kind.
    #[serde(default = "crate::common::parse::default_expose_methods")]
    #[config(value)]
    pub(crate) expose_methods: Vec<String>,
    /// Types whose presence in a return type signals interior mutability
    /// (`AtomicU32`, `Mutex`, `RwLock`, ...). Detection recurses into generic
    /// arguments, so `Arc<AtomicUsize>` / `Option<&Mutex<T>>` are caught.
    #[serde(default = "default_mutable_handle_types")]
    #[config(value)]
    pub(crate) mutable_handle_types: Vec<String>,
    /// Single-arg constructor calls treated as transparent over their argument
    /// (`Some(&self.x)`, `Box::new(...)`, `Cow::Borrowed(...)`, `Arc::new(...)`,
    /// ...). Patterns are matched as plain (`Some`) or path suffix (`Box::new`).
    #[serde(default = "crate::common::parse::default_wrapper_ctors")]
    #[config(value)]
    pub(crate) wrapper_ctors: Vec<String>,
    /// Method names that mutate when applied to `self.<field>`: `store`, `set`,
    /// `swap`, `fetch_add`, ... Used by P3 to find a setter that targets the
    /// same field as a `*_handle()` getter.
    #[serde(default = "default_writer_methods")]
    #[config(value)]
    pub(crate) writer_methods: Vec<String>,
    #[config(value)]
    pub(crate) detect_delegate_passthrough: bool,
    #[config(value)]
    pub(crate) detect_field_passthrough: bool,
    #[config(value)]
    pub(crate) detect_mutation_handle: bool,
    #[config(value)]
    pub(crate) detect_nested_shorthand: bool,
    #[config(value)]
    pub(crate) ignore_deref: bool,
    #[config(value)]
    pub(crate) public_only: bool,
}

impl Default for RedundantAccessorsThreshold {
    fn default() -> Self {
        Self {
            detect_field_passthrough: true,
            detect_nested_shorthand: true,
            detect_mutation_handle: true,
            detect_delegate_passthrough: true,
            p1_severity: AccessorSeverity::Warn,
            p2_severity: AccessorSeverity::Warn,
            p3_severity: AccessorSeverity::Deny,
            p4_severity: AccessorSeverity::Warn,
            public_only: true,
            ignore_deref: true,
            mutable_handle_types: default_mutable_handle_types(),
            writer_methods: default_writer_methods(),
            wrapper_ctors: crate::common::parse::default_wrapper_ctors(),
            expose_methods: crate::common::parse::default_expose_methods(),
        }
    }
}

fn default_mutable_handle_types() -> Vec<String> {
    [
        "AtomicBool",
        "AtomicI8",
        "AtomicI16",
        "AtomicI32",
        "AtomicI64",
        "AtomicIsize",
        "AtomicU8",
        "AtomicU16",
        "AtomicU32",
        "AtomicU64",
        "AtomicUsize",
        "AtomicPtr",
        "Cell",
        "RefCell",
        "OnceCell",
        "LazyLock",
        "Mutex",
        "RwLock",
        "Notify",
        "Semaphore",
        "Condvar",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn default_writer_methods() -> Vec<String> {
    [
        "store",
        "swap",
        "fetch_add",
        "fetch_sub",
        "fetch_or",
        "fetch_and",
        "fetch_xor",
        "fetch_max",
        "fetch_min",
        "fetch_update",
        "try_update",
        "fetch_nand",
        "compare_exchange",
        "compare_exchange_weak",
        "set",
        "replace",
        "replace_with",
        "take",
        "swap",
        "write",
        "send",
        "send_replace",
        "send_modify",
        "notify_one",
        "notify_all",
        "notify_waiters",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct MixedEntitiesThreshold {
    /// Files with this many sizable types → deny.
    #[config(value)]
    pub(crate) deny: usize,
    /// A type is "sizable" if its `impl` surface has at least this many `fn`s
    /// across `impl X` and `impl Trait for X` blocks combined.
    #[config(value)]
    pub(crate) min_fns_per_type: usize,
    /// A type also counts as "sizable" if it has at least this many `impl`
    /// blocks (own + trait impls). Catches structural types whose surface is
    /// spread across many trait implementations rather than methods.
    #[config(value)]
    pub(crate) min_impl_blocks: usize,
    /// Files with this many sizable types → warn.
    #[config(value)]
    pub(crate) warn: usize,
}

impl Default for MixedEntitiesThreshold {
    fn default() -> Self {
        Self {
            min_fns_per_type: 5,
            min_impl_blocks: 3,
            warn: 2,
            deny: 3,
        }
    }
}

#[derive(Debug, Default, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ModuleLayersConfig {
    #[serde(default, rename = "crate")]
    #[config(value)]
    pub(crate) crates: Vec<CrateLayers>,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct CrateLayers {
    #[config(value)]
    pub(crate) name: String,
    #[serde(default, rename = "layer")]
    #[config(value)]
    pub(crate) layers: Vec<ModuleLayer>,
}

#[derive(Debug, Deserialize, Clone, kithara_config::Config)]
#[serde(deny_unknown_fields)]
#[config(builder(none))]
pub(crate) struct ModuleLayer {
    #[config(value)]
    pub(crate) name: String,
    #[config(value)]
    pub(crate) paths: Vec<String>,
    #[config(value)]
    pub(crate) index: u32,
}

fn load_optional<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("read config: {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse config: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A namespace with no `thresholds.toml` section must still know what it
    /// looks for. A derived `Default` hands the check an empty list and the
    /// ratchet goes quiet instead of failing.
    #[test]
    fn a_threshold_built_from_default_keeps_its_subjects() {
        let thresholds = ThresholdsConfig::default();

        assert_eq!(
            thresholds.cancel_root_sites.patterns,
            ["CancelToken::root", "CancelToken::never"]
        );
        assert_eq!(
            thresholds.smoothing_primitive_sites.patterns,
            ["SmoothingFilter", "one_pole", "OnePole", "smoothing_coeff"]
        );
        assert_eq!(
            thresholds.firewheel_dsp_facade.modules,
            [
                "dsp::fade",
                "dsp::mix",
                "dsp::filter::smoothing_filter",
                "param::smoother"
            ]
        );
        assert_eq!(
            thresholds.tokio_dep_quarantine.quarantined,
            ["tokio", "tokio-util", "tokio-stream"]
        );
        assert_eq!(thresholds.field_passthrough.max_depth, 8);
        assert_eq!(
            thresholds.platform_layer_hygiene.root,
            "crates/kithara-platform/src/"
        );
        assert_eq!(thresholds.platform_layer_hygiene.arc_owner_files.len(), 2);
    }

    /// The other half of the same trap. A present table takes the per-field
    /// defaults, never the written `Default`, so a project that sets one key of
    /// a threshold must keep the rest of that threshold's subjects.
    #[test]
    fn a_partly_configured_threshold_keeps_the_keys_it_left_alone() {
        let thresholds: ThresholdsConfig = toml::from_str(
            r#"
[field_passthrough]
exempt_files = ["crates/demo/src/lib.rs"]

[cancel_root_sites]
exempt_crates = ["kithara-demo"]

[smoothing_primitive_sites]
allowed_files = ["crates/demo/src/gain.rs"]

[firewheel_dsp_facade]
allowed_files = ["crates/demo/src/param.rs"]

[tokio_dep_quarantine]
allowed_crates = ["kithara-demo"]

[platform_layer_hygiene]
excluded_subtrees = ["demo/"]
"#,
        )
        .expect("a partial thresholds document");

        assert_eq!(thresholds.field_passthrough.max_depth, 8);
        assert_eq!(
            thresholds.field_passthrough.transparent_wrappers,
            ["Arc", "Rc", "Box", "RefCell", "Cell", "Mutex", "RwLock"]
        );
        assert_eq!(
            thresholds.cancel_root_sites.patterns,
            ["CancelToken::root", "CancelToken::never"]
        );
        assert_eq!(
            thresholds.smoothing_primitive_sites.patterns,
            ["SmoothingFilter", "one_pole", "OnePole", "smoothing_coeff"]
        );
        assert_eq!(
            thresholds.firewheel_dsp_facade.modules,
            [
                "dsp::fade",
                "dsp::mix",
                "dsp::filter::smoothing_filter",
                "param::smoother"
            ]
        );
        assert_eq!(
            thresholds.tokio_dep_quarantine.quarantined,
            ["tokio", "tokio-util", "tokio-stream"]
        );
        assert_eq!(
            thresholds.platform_layer_hygiene.root,
            "crates/kithara-platform/src/"
        );
        assert_eq!(thresholds.platform_layer_hygiene.arc_owner_files.len(), 2);
    }
}
