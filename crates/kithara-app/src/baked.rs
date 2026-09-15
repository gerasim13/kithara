include!(concat!(env!("OUT_DIR"), "/app_config_baked.rs"));

/// The value a `$KITHARA_...` reference resolves to: the process environment
/// first, then the value this build baked in obfuscated. Tests that talk to a
/// key server read their credentials here, so a test binary carries them the
/// same way the application does.
#[must_use]
pub fn secret(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| baked_env(name))
}

#[cfg(test)]
mod tests {
    use super::baked_env;

    #[kithara::test(native, flash(false))]
    fn a_name_the_document_never_references_is_absent() {
        assert_eq!(baked_env("KITHARA_NOT_REFERENCED_BY_APP_YAML"), None);
    }

    #[kithara::test(native, flash(false))]
    fn the_document_text_survives_the_build_verbatim() {
        assert_eq!(
            super::BAKED_DOCUMENT,
            include_str!("../app.yaml"),
            "the embedded document must be the file byte-for-byte, not a rendering of it"
        );
    }
}
