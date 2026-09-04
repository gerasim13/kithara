use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ron_io;
use crate::{
    envelope::{self, DocKind},
    error::UiDocError,
    ids::{DocId, ScreenRole, SourceUri},
    source::SourceResolver,
};

/// The ui contract this build offers a package.
///
/// It counts the vocabulary a package is written against - the roles asked
/// for, the endpoints answered, the extension kinds drawable - and not the
/// shape of any one document, which each document states for itself.
pub const UI_CONTRACT: u32 = 1;

/// What a package says about itself before any of its documents are read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PackageDoc {
    /// The file behind each role the package answers for.
    pub screens: BTreeMap<ScreenRole, String>,
    pub id: DocId,
    #[serde(default)]
    pub skin: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    pub schema: String,
    /// Whether what this package does not hold is read from the one below it.
    #[serde(default)]
    pub inherits: bool,
    /// The ui contract the package was written against.
    pub contract: u32,
    pub version: u32,
}

impl PackageDoc {
    /// The file this package puts behind `role`, once that file agrees it is
    /// that screen.
    ///
    /// The manifest says which file stands for a role and the document says
    /// which screen it is; a package whose two answers disagree has a typo in
    /// one of them, and reading only the manifest would compile the wrong
    /// screen without a word. Only the envelope is parsed here, and the
    /// resolver has already read the text the compile will parse in full.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when the package answers for no such screen, when
    /// the file behind it cannot be read, or when that file names another
    /// screen.
    pub fn screen(
        &self,
        resolver: &dyn SourceResolver,
        role: &ScreenRole,
    ) -> Result<String, UiDocError> {
        let file = self
            .screens
            .get(role)
            .ok_or_else(|| UiDocError::MissingRole {
                package: self.id.0.clone(),
                role: role.0.clone(),
            })?;
        let loaded = resolver.load(None, file)?;
        let envelope = envelope::probe(&loaded.text, &loaded.uri)?;
        if envelope.id.0 == role.0 {
            return Ok(file.clone());
        }
        Err(UiDocError::RoleMismatch {
            found: envelope.id.0,
            origin: loaded.uri,
            role: role.0.clone(),
        })
    }
}

/// Reads the manifest at `rel` and checks it before anything else is parsed.
///
/// The contract check comes first: a package written for another build is
/// refused here, while its documents are still unread, so the message names the
/// mismatch rather than whatever the first stale document happened to trip on.
///
/// # Errors
/// Returns [`UiDocError`] when the manifest is unavailable, malformed, written
/// against another contract, or declares nothing to answer with.
pub fn load_package(resolver: &dyn SourceResolver, rel: &str) -> Result<PackageDoc, UiDocError> {
    let loaded = resolver.load(None, rel)?;
    let doc = parse_package(&loaded.text, &loaded.uri)?;
    if doc.contract != UI_CONTRACT {
        return Err(UiDocError::ContractMismatch {
            needs: doc.contract,
            offers: UI_CONTRACT,
            origin: loaded.uri,
        });
    }
    if doc.screens.is_empty() {
        return Err(UiDocError::EmptyPackage { origin: loaded.uri });
    }
    for (role, file) in &doc.screens {
        if file.is_empty() {
            return Err(UiDocError::RoleWithoutFile {
                origin: loaded.uri,
                role: role.0.clone(),
            });
        }
    }
    Ok(doc)
}

/// Parses a package manifest.
///
/// # Errors
/// Returns [`UiDocError`] when the RON, schema, or version is invalid.
pub fn parse_package(text: &str, origin: &SourceUri) -> Result<PackageDoc, UiDocError> {
    let envelope = envelope::probe(text, origin)?;
    if envelope.kind != DocKind::Package {
        return Err(UiDocError::WrongDocKind {
            origin: origin.clone(),
            expected: DocKind::Package.name(),
            found: envelope.kind.name(),
        });
    }
    ron_io::options()
        .from_str(text)
        .map_err(|source| UiDocError::Syntax {
            origin: origin.clone(),
            source: Box::new(source),
        })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::source::MemResolver;

    const MANIFEST: &str = r#"(
        schema: "kithara.package",
        version: 1,
        id: "kithara-default",
        contract: 1,
        screens: {
            "player": "player.klayout.ron",
            "player-single": "player-single.klayout.ron",
        },
    )"#;

    fn layout(id: &str) -> String {
        format!(r#"(schema: "kithara.layout", version: 1, id: "{id}", root: ())"#)
    }

    fn holding(manifest: &str) -> MemResolver {
        let mut resolver = MemResolver::default();
        resolver.insert("package.kpackage.ron", manifest);
        resolver.insert("player.klayout.ron", &layout("player"));
        resolver.insert("player-single.klayout.ron", &layout("player-single"));
        resolver
    }

    #[kithara::test]
    fn a_package_names_the_file_behind_a_role() {
        let resolver = holding(MANIFEST);
        let package = load_package(&resolver, "package.kpackage.ron").unwrap();

        assert_eq!(
            package
                .screen(&resolver, &ScreenRole("player".into()))
                .unwrap(),
            "player.klayout.ron"
        );
    }

    #[kithara::test]
    fn a_role_the_package_does_not_answer_is_refused_by_name() {
        let resolver = holding(MANIFEST);
        let package = load_package(&resolver, "package.kpackage.ron").unwrap();

        let error = package
            .screen(&resolver, &ScreenRole("mixer".into()))
            .unwrap_err();

        assert!(matches!(
            error,
            UiDocError::MissingRole { role, .. } if role == "mixer"
        ));
    }

    #[kithara::test]
    fn a_package_inherits_nothing_unless_it_says_so() {
        let package = load_package(&holding(MANIFEST), "package.kpackage.ron").unwrap();

        assert!(!package.inherits);
    }

    #[kithara::test]
    fn a_package_that_says_so_inherits() {
        let manifest = MANIFEST.replace("contract: 1,", "contract: 1, inherits: true,");

        let package = load_package(&holding(&manifest), "package.kpackage.ron").unwrap();

        assert!(package.inherits);
    }

    /// The contract is checked while the documents are still unread, so the
    /// message names the mismatch and not whatever a stale document tripped on.
    #[kithara::test]
    fn a_package_written_for_another_contract_is_refused() {
        let manifest = MANIFEST.replace("contract: 1,", "contract: 7,");

        let error = load_package(&holding(&manifest), "package.kpackage.ron").unwrap_err();

        assert!(matches!(
            error,
            UiDocError::ContractMismatch {
                needs: 7,
                offers: 1,
                ..
            }
        ));
    }

    /// The contract is the entrance check. A package that is wrong in both ways
    /// must be told about the contract, because that is the one an author can
    /// act on: the rest of the manifest was written for a different build.
    #[kithara::test]
    fn a_foreign_contract_is_reported_before_anything_else_about_the_package() {
        let manifest = MANIFEST.replace("contract: 1,", "contract: 7,").replace(
            r#"screens: {
            "player": "player.klayout.ron",
            "player-single": "player-single.klayout.ron",
        },"#,
            "screens: {},",
        );

        let error = load_package(&holding(&manifest), "package.kpackage.ron").unwrap_err();

        assert!(matches!(error, UiDocError::ContractMismatch { .. }));
    }

    #[kithara::test]
    fn a_package_answering_for_nothing_is_refused() {
        let manifest = MANIFEST.replace(
            r#"screens: {
            "player": "player.klayout.ron",
            "player-single": "player-single.klayout.ron",
        },"#,
            "screens: {},",
        );

        let error = load_package(&holding(&manifest), "package.kpackage.ron").unwrap_err();

        assert!(matches!(error, UiDocError::EmptyPackage { .. }));
    }

    #[kithara::test]
    fn a_role_with_no_file_behind_it_is_refused() {
        let manifest = MANIFEST.replace(r#""player": "player.klayout.ron","#, r#""player": "","#);

        let error = load_package(&holding(&manifest), "package.kpackage.ron").unwrap_err();

        assert!(matches!(
            error,
            UiDocError::RoleWithoutFile { role, .. } if role == "player"
        ));
    }

    /// The manifest says which file stands for a role and the document says
    /// which screen it is. A package whose two answers disagree has a typo, and
    /// compiling on the manifest alone would draw the wrong screen in silence.
    #[kithara::test]
    fn a_file_naming_another_screen_is_refused_under_the_role_it_was_put_behind() {
        let manifest = MANIFEST.replace(
            r#""player": "player.klayout.ron","#,
            r#""player": "player-single.klayout.ron","#,
        );
        let resolver = holding(&manifest);
        let package = load_package(&resolver, "package.kpackage.ron").unwrap();

        let error = package
            .screen(&resolver, &ScreenRole("player".into()))
            .unwrap_err();

        assert!(matches!(
            error,
            UiDocError::RoleMismatch { found, .. } if found == "player-single"
        ));
    }

    #[kithara::test]
    fn a_role_whose_file_is_not_there_is_not_found() {
        let manifest = MANIFEST.replace(
            r#""player": "player.klayout.ron","#,
            r#""player": "gone.klayout.ron","#,
        );
        let resolver = holding(&manifest);
        let package = load_package(&resolver, "package.kpackage.ron").unwrap();

        let error = package
            .screen(&resolver, &ScreenRole("player".into()))
            .unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }

    #[kithara::test]
    fn a_manifest_that_is_another_kind_of_document_is_refused() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "package.kpackage.ron",
            r#"(schema: "kithara.layout", version: 1, id: "player", root: ())"#,
        );

        let error = load_package(&resolver, "package.kpackage.ron").unwrap_err();

        assert!(matches!(
            error,
            UiDocError::WrongDocKind {
                expected: "package",
                ..
            }
        ));
    }

    #[kithara::test]
    fn a_manifest_the_resolver_does_not_hold_is_not_found() {
        let error = load_package(&MemResolver::default(), "package.kpackage.ron").unwrap_err();

        assert!(matches!(error, UiDocError::NotFound { .. }));
    }
}
