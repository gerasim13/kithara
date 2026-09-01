use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use kithara::{
    assets::AssetLayoutRegistry, hls::SizeProbeMethod, net::Compression,
    play::policy::DomainKeyPolicy,
};
use serde_yaml_ng::Value;

use super::{
    env::{MissingEnv, expand},
    layouts::asset_layouts,
    merge::merge,
    policy::{PolicyError, drm_policy},
    schema::Document,
};
use crate::baked::{BAKED_DOCUMENT, baked_env};

/// Path the baked document is reported under in parse errors.
const BAKED_PATH: &str = "<baked app.yaml>";

/// The configuration this process runs on, and the document it came from.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    document: Document,
    /// The merged document before expansion. Kept so a dump can print
    /// references rather than the secrets behind them.
    source: Value,
}

/// Why a document could not be turned into a configuration.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// A path the operator named does not exist.
    Missing(PathBuf),
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_yaml_ng::Error,
    },
    Env(MissingEnv),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "configuration file not found: {}", path.display()),
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::Env(missing) => write!(f, "{missing}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Env(missing) => Some(missing),
            Self::Missing(_) => None,
        }
    }
}

impl Config {
    /// Read the configuration: the baked document, an overlay laid on top, then
    /// environment references expanded over the result.
    ///
    /// `explicit` is a path the operator named and must exist; `beside` is the
    /// conventional file next to the executable and may be absent.
    ///
    /// # Errors
    /// Returns [`LoadError`] when a named file is missing or unreadable, a
    /// document does not match the schema, or a reference resolves nowhere.
    pub fn load(explicit: Option<&Path>, beside: Option<&Path>) -> Result<Self, LoadError> {
        let mut source: Value =
            serde_yaml_ng::from_str(BAKED_DOCUMENT).map_err(|source| LoadError::Parse {
                path: PathBuf::from(BAKED_PATH),
                source,
            })?;

        let overlay_path = Self::overlay_path(explicit, beside)?;
        if let Some(path) = overlay_path.as_deref() {
            merge(&mut source, Self::read(path)?);
        }

        let mut expanded = source.clone();
        expand(&mut expanded, &|name| {
            std::env::var(name).ok().or_else(|| baked_env(name))
        })
        .map_err(LoadError::Env)?;

        let reported = overlay_path.unwrap_or_else(|| PathBuf::from(BAKED_PATH));
        let document = serde_yaml_ng::from_value(expanded).map_err(|source| LoadError::Parse {
            path: reported,
            source,
        })?;

        Ok(Self { document, source })
    }

    fn overlay_path(
        explicit: Option<&Path>,
        beside: Option<&Path>,
    ) -> Result<Option<PathBuf>, LoadError> {
        if let Some(path) = explicit {
            return if path.exists() {
                Ok(Some(path.to_path_buf()))
            } else {
                Err(LoadError::Missing(path.to_path_buf()))
            };
        }
        Ok(beside.filter(|path| path.exists()).map(Path::to_path_buf))
    }

    fn read(path: &Path) -> Result<Value, LoadError> {
        let text = fs::read_to_string(path).map_err(|source| LoadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        serde_yaml_ng::from_str(&text).map_err(|source| LoadError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Tracks the document opens with.
    #[must_use]
    pub fn tracks(&self) -> &[String] {
        &self.document.playlist.tracks
    }

    /// Whether the HTTP client accepts invalid certificates. Test servers only.
    #[must_use]
    pub fn should_accept_invalid_certs(&self) -> bool {
        self.document
            .network
            .should_accept_invalid_certs
            .unwrap_or_default()
    }

    /// `Accept-Encoding` algorithms the HTTP client offers.
    #[must_use]
    pub fn compression(&self) -> Compression {
        self.document.network.compression()
    }

    /// HLS size-probe strategy.
    #[must_use]
    pub fn size_probe_method(&self) -> SizeProbeMethod {
        self.document.network.size_probe_method
    }

    /// Crossfade length, when the document sets one.
    #[must_use]
    pub fn crossfade_seconds(&self) -> Option<f32> {
        self.document.playback.crossfade_seconds
    }

    /// The media-identity registry the asset store reads.
    #[must_use]
    pub fn asset_layouts(&self) -> AssetLayoutRegistry {
        asset_layouts(&self.document.assets)
    }

    /// The DRM policy the key registry resolves through.
    ///
    /// # Errors
    /// Returns an error when a provider declares a policy that cannot be
    /// honoured -- a reserved header, or a hex salt of odd length.
    pub fn drm_policy(&self) -> Result<DomainKeyPolicy, PolicyError> {
        drm_policy(&self.document.drm)
    }

    /// The effective configuration as a document. Printed before expansion, so
    /// a dump names `$KITHARA_...` rather than handing out the secret behind it.
    #[must_use]
    pub fn dump(&self) -> String {
        serde_yaml_ng::to_string(&self.source)
            .unwrap_or_else(|e| format!("cannot render the configuration: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use kithara::hls::SizeProbeMethod;

    use super::{Config, LoadError};

    fn write(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kithara-config-{name}.yaml"));
        fs::write(&path, contents).expect("write the test document");
        path
    }

    #[kithara::test(native, flash(false))]
    fn no_file_leaves_the_baked_document_in_force() {
        let config = Config::load(None, None).expect("the baked document stands alone");

        assert_eq!(
            config.size_probe_method(),
            SizeProbeMethod::RangeGet,
            "the shipped document selects range_get"
        );
        assert!(!config.tracks().is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn a_file_overrides_only_what_it_names() {
        let path = write(
            "overrides-one-field",
            "network:\n  size_probe_method: head\n",
        );

        let config = Config::load(Some(&path), None).expect("the overlay loads");

        assert_eq!(config.size_probe_method(), SizeProbeMethod::Head);
        assert!(
            !config.tracks().is_empty(),
            "a section the overlay never names keeps its baked value"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_path_named_explicitly_must_exist() {
        let missing = std::env::temp_dir().join("kithara-config-absent.yaml");

        let error = Config::load(Some(&missing), None).expect_err("the operator named this file");

        assert!(matches!(error, LoadError::Missing(_)));
    }

    #[kithara::test(native, flash(false))]
    fn a_file_beside_the_binary_may_be_absent() {
        let absent = std::env::temp_dir().join("kithara-config-not-there.yaml");

        Config::load(None, Some(&absent)).expect("an unnamed file is optional");
    }

    #[kithara::test(native, flash(false))]
    fn an_unresolved_reference_refuses_to_start() {
        let path = write(
            "unresolved-reference",
            concat!(
                "drm:\n  providers:\n    - name: x\n      domains: [x.test]\n",
                "      cipher_key: $KITHARA_DEFINITELY_UNSET_IN_TESTS\n",
            ),
        );

        let error = Config::load(Some(&path), None).expect_err("the reference resolves nowhere");

        assert!(
            error
                .to_string()
                .contains("KITHARA_DEFINITELY_UNSET_IN_TESTS"),
            "{error}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_malformed_file_names_its_path() {
        let path = write("malformed", "network: [not, a, mapping]\n");

        let error = Config::load(Some(&path), None).expect_err("the shape is wrong");

        assert!(
            error.to_string().contains("kithara-config-malformed"),
            "{error}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn the_dump_carries_references_not_secrets() {
        let config = Config::load(None, None).expect("the baked document stands alone");

        let dump = config.dump();

        assert!(
            dump.contains("$KITHARA_DRM_PROD_KEY"),
            "the dump prints the reference, not what it resolves to"
        );
    }
}
