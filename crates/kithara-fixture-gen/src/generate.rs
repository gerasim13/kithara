use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    panic::resume_unwind,
    path::{Path, PathBuf},
    thread,
};

use kithara_platform::time::Instant;

use self::consts::{DISABLE_REMOTE_FIXTURES_ENV, WAIT_REPORTED};
use crate::{
    context::BuildContext,
    graph,
    registry::{AssetBuild, AssetDef},
    store,
};

mod consts {
    use kithara_platform::time::Duration;

    pub(super) const DISABLE_REMOTE_FIXTURES_ENV: &str = "KITHARA_DISABLE_REMOTE_FIXTURES";
    /// A wait on another build's fixture shorter than this is two builds
    /// starting together, not one waiting on the other. Cargo holds a build
    /// script's output until it ends, so the warning a longer wait prints is
    /// the one trace of it the job log gets.
    pub(super) const WAIT_REPORTED: Duration = Duration::from_secs(1);
}

/// Rejects two cases that would produce one accessor, before either is written.
fn resolve(defs: &[&'static AssetDef]) -> Vec<(String, String, &'static AssetDef)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut resolved = Vec::with_capacity(defs.len());
    for def in defs {
        let name = def.accessor_name();
        assert!(
            seen.insert(name.clone()),
            "kithara-test-fixtures: two asset cases share the accessor name `{name}`",
        );
        let id = def.format.map_or_else(
            || store::asset_id(def.func, def.case),
            |format| store::formatted_asset_id(def.func, def.case, &format()),
        );
        resolved.push((name, id, *def));
    }
    resolved.sort_by(|(left, _, _), (right, _, _)| left.cmp(right));
    resolved
}

/// The accessor names this build must produce again, with every asset built
/// from one of them. A derived asset holds bytes of its dependency, so
/// refreshing a source without its dependents would leave the two disagreeing.
/// Unknown selections are nonfatal because enabled families register different
/// subsets. The small acyclic graph makes repeated dependency closure finite.
fn refreshed(resolved: &[(String, String, &'static AssetDef)]) -> HashSet<String> {
    let selection = store::Refresh::requested();
    let mut names: HashSet<String> = resolved
        .iter()
        .filter(|(name, _, def)| selection.selects(def.func, name))
        .map(|(name, _, _)| name.clone())
        .collect();
    if let store::Refresh::Named(requested) = &selection {
        for requested in requested {
            if !resolved
                .iter()
                .any(|(name, _, def)| name == requested || def.func == requested)
            {
                println!(
                    "cargo:warning={} names `{requested}`, which no enabled family registers",
                    store::REFRESH_ENV,
                );
            }
        }
    }
    loop {
        let grown: Vec<String> = resolved
            .iter()
            .filter(|(name, _, def)| {
                !names.contains(name)
                    && def
                        .dependencies
                        .iter()
                        .any(|dependency| names.contains(*dependency))
            })
            .map(|(name, _, _)| name.clone())
            .collect();
        if grown.is_empty() {
            return names;
        }
        names.extend(grown);
    }
}

fn materialize(
    namespace: &Path,
    resolved: &[(String, String, &'static AssetDef)],
    refresh: &HashSet<String>,
) -> HashMap<String, String> {
    let mut unavailable = HashMap::new();
    let nodes: Vec<_> = resolved
        .iter()
        .map(|(name, _, def)| graph::Node {
            name,
            dependencies: def.dependencies,
        })
        .collect();
    let levels =
        graph::levels(&nodes).unwrap_or_else(|error| panic!("kithara-test-fixtures: {error}"));
    let parallelism = thread::available_parallelism().map_or(1, usize::from);

    for level in levels {
        for batch in level.chunks(parallelism) {
            let unavailable_snapshot = &unavailable;
            let results = thread::scope(|scope| {
                batch
                    .iter()
                    .map(|&index| {
                        scope.spawn(move || {
                            materialize_one(
                                namespace,
                                resolved,
                                index,
                                unavailable_snapshot,
                                refresh,
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or_else(|panic| resume_unwind(panic)))
                    .collect::<Vec<_>>()
            });
            unavailable.extend(results.into_iter().flatten());
        }
    }
    unavailable
}

/// Only optional sources without dependencies fetch remotely; optional derived
/// assets are produced locally whenever their dependencies exist. Without hydration,
/// fetching families cannot regenerate, so refresh retains their cached entries.
fn materialize_one(
    namespace: &Path,
    resolved: &[(String, String, &'static AssetDef)],
    index: usize,
    unavailable: &HashMap<String, String>,
    refresh: &HashSet<String>,
) -> Option<(String, String)> {
    let (name, id, def) = &resolved[index];
    let fetches = def.optional && def.dependencies.is_empty();
    let hydration_off = fetches
        && std::env::var_os(DISABLE_REMOTE_FIXTURES_ENV).is_some_and(|value| !value.is_empty());
    let reuse = hydration_off || !refresh.contains(name);
    if reuse && store::has_entry(namespace, id, def.ext) {
        return None;
    }
    if hydration_off {
        return Some((
            name.clone(),
            format!("remote hydration disabled by {DISABLE_REMOTE_FIXTURES_ENV}"),
        ));
    }
    let waiting = Instant::now();
    let _lock = store::lock_entry(namespace, id)
        .unwrap_or_else(|error| panic!("kithara-test-fixtures: lock for `{name}`: {error}"));
    let waited = waiting.elapsed();
    if waited >= WAIT_REPORTED {
        println!(
            "cargo:warning=waited {} s for fixture `{name}`, which another build was producing",
            waited.as_secs()
        );
    }
    if reuse && store::has_entry(namespace, id, def.ext) {
        return None;
    }
    if let Some((dependency, reason)) = def.dependencies.iter().find_map(|dependency| {
        unavailable
            .get(*dependency)
            .map(|reason| (dependency, reason))
    }) {
        if def.optional {
            return Some((
                name.clone(),
                format!("dependency `{dependency}` unavailable: {reason}"),
            ));
        }
        panic!(
            "kithara-test-fixtures: required fixture `{name}` depends on unavailable `{dependency}`: {reason}"
        );
    }
    let dependency_bytes: Vec<_> = def
        .dependencies
        .iter()
        .map(|dependency| {
            let (_, dependency_id, dependency_def) = resolved
                .iter()
                .find(|(candidate, _, _)| candidate == dependency)
                .unwrap_or_else(|| {
                    panic!(
                        "kithara-test-fixtures: graph accepted missing dependency `{dependency}`"
                    )
                });
            store::read_entry(namespace, dependency_id, dependency_def.ext).unwrap_or_else(|| {
                panic!(
                    "kithara-test-fixtures: dependency `{dependency}` of `{name}` was not materialized"
                )
            })
        })
        .collect();
    let inputs: Vec<_> = dependency_bytes.iter().map(Vec::as_slice).collect();
    let context = BuildContext::new(namespace, id);
    #[cfg(feature = "remote")]
    let bytes = match (def.build)(context, &inputs) {
        AssetBuild::Ready(bytes) => bytes,
        AssetBuild::Unavailable(reason) if def.optional => {
            println!("cargo:warning=optional fixture `{name}` unavailable: {reason}");
            return Some((name.clone(), reason));
        }
        AssetBuild::Unavailable(reason) => {
            panic!("kithara-test-fixtures: required fixture `{name}` unavailable: {reason}")
        }
    };
    #[cfg(not(feature = "remote"))]
    let AssetBuild::Ready(bytes) = (def.build)(context, &inputs);
    assert!(
        !bytes.is_empty(),
        "kithara-test-fixtures: `{name}` produced no bytes"
    );
    store::write_entry(namespace, id, def.ext, &bytes)
        .unwrap_or_else(|error| panic!("kithara-test-fixtures: write `{name}`: {error}"));
    None
}

/// Filesystem-free WASM accessors embed bytes at compile time; native accessors
/// retain store metadata and read entries at runtime.
fn codegen(
    namespace: &Path,
    resolved: &[(String, String, &'static AssetDef)],
    unavailable: &HashMap<String, String>,
    embed_assets: bool,
) -> String {
    let mut out = String::new();
    let mut manifest = String::new();
    let mut by_name = String::new();
    for (name, id, def) in resolved {
        let entry = format!("ENTRY_{}", name.to_uppercase());
        let path = store::entry_path(namespace, id, def.ext);
        let path = path.to_str().unwrap_or_else(|| {
            panic!("kithara-test-fixtures: the store path for `{name}` is not valid UTF-8")
        });
        let relative_path = format!("{}/{id}.{}", store::CACHE_VERSION.trim(), def.ext);
        let content_type = def.content_type;
        let unavailable = unavailable
            .get(name)
            .map_or_else(|| "None".to_owned(), |reason| format!("Some({reason:?})"));
        let (cfg, body) = if def.embed && embed_assets {
            (
                "",
                format!("    crate::asset::Asset::embedded(&{entry}, include_bytes!({path:?}))"),
            )
        } else {
            (
                "#[cfg(not(target_arch = \"wasm32\"))]\n",
                format!(
                    "    static BYTES: ::std::sync::OnceLock<Vec<u8>> = \
                     ::std::sync::OnceLock::new();\n    \
                     crate::asset::Asset::on_disk(&{entry}, &BYTES)"
                ),
            )
        };
        let _ = write!(
            out,
            "static {entry}: crate::asset::AssetEntry = crate::asset::AssetEntry {{\n    \
             name: {name:?},\n    id: {id:?},\n    path: {relative_path:?},\n    \
             content_type: {content_type:?},\n    unavailable: {unavailable},\n}};\n\n\
             {cfg}#[must_use]\n\
             pub fn {name}() -> crate::asset::Asset {{\n{body}\n}}\n\n",
        );
        let _ = writeln!(manifest, "    &{entry},");
        let _ = writeln!(
            by_name,
            "    {}({name:?}, {name}),",
            cfg.replace('\n', "\n        "),
        );
    }
    let _ = write!(
        out,
        "/// Every asset this build materialized.\n\
         pub static MANIFEST: &[&crate::asset::AssetEntry] = &[\n{manifest}];\n\n\
         type AssetLookup = (&'static str, fn() -> crate::asset::Asset);\n\n\
         static BY_NAME: &[AssetLookup] = &[\n{by_name}];\n\n\
         /// The asset this build registered under `name`.\n\
         #[must_use]\n\
         pub fn by_name(name: &str) -> Option<crate::asset::Asset> {{\n    \
         BY_NAME\n        .iter()\n        .find(|(candidate, _)| *candidate == name)\n        \
         .map(|(_, asset)| asset())\n}}\n",
    );
    out
}

/// Materialize every registered asset into the shared store and write one
/// accessor per case into `OUT_DIR/assets.rs`. Called from the build script of
/// the crate the accessors compile into.
/// Only available entries are watched: Cargo treats absent watched files as changed
/// on every build, while hydration environment changes rerun this script. The stamp
/// detects namespace removal; individual watches detect changed entries.
///
/// # Panics
///
/// Panics when a required asset cannot be produced or the store cannot be
/// written: a build script has no other way to fail.
pub fn generate() {
    println!("cargo:rerun-if-env-changed={}", store::STORE_ENV);
    println!("cargo:rerun-if-env-changed={DISABLE_REMOTE_FIXTURES_ENV}");
    println!("cargo:rerun-if-env-changed={}", store::REFRESH_ENV);

    let defs: Vec<&AssetDef> = inventory::iter::<AssetDef>.into_iter().collect();
    assert!(
        !defs.is_empty(),
        "kithara-test-fixtures: the asset registry is empty; either `src/defs` declares no \
         `#[kithara::asset]`, or the registrations were dropped at link time",
    );
    for name in defs.iter().flat_map(|def| def.env) {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let resolved = resolve(&defs);

    let fingerprint = store::CACHE_VERSION.trim();
    let root =
        store::root_from_env().unwrap_or_else(|error| panic!("kithara-test-fixtures: {error}"));
    let namespace = store::namespace(&root, fingerprint);
    let unavailable = materialize(&namespace, &resolved, &refreshed(&resolved));
    for (_, id, def) in resolved
        .iter()
        .filter(|(name, _, _)| !unavailable.contains_key(name))
    {
        println!(
            "cargo:rerun-if-changed={}",
            store::entry_path(&namespace, id, def.ext).display()
        );
    }

    let stamp = store::write_stamp(&namespace, fingerprint)
        .unwrap_or_else(|error| panic!("kithara-test-fixtures: write stamp: {error}"));
    println!("cargo:rerun-if-changed={}", stamp.display());

    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("invariant: cargo always sets OUT_DIR"));
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH")
        .expect("invariant: cargo always sets CARGO_CFG_TARGET_ARCH");
    fs::write(
        out_dir.join("assets.rs"),
        codegen(&namespace, &resolved, &unavailable, target_arch == "wasm32"),
    )
    .unwrap_or_else(|error| panic!("kithara-test-fixtures: write assets.rs: {error}"));
}
