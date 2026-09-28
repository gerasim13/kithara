use std::collections::HashMap;

use serde_yaml_ng::Value;

use crate::merge;

/// What a build embeds: the document text, every reference it names, and the
/// values the build found for them.
pub struct Bake {
    /// The document the target parses at startup.
    pub document: String,
    /// Each reference as the dotted path it sits at and the name it reads.
    pub refs: Vec<(String, String)>,
    /// Names the build environment answered with a non-empty value, sorted and
    /// unique.
    pub resolved: Vec<(String, String)>,
}

/// Bake `app` for `target_arch`.
///
/// A `wasm32` build lays `web` on top and resolves nothing, because a browser
/// bundle must not carry a secret; every other target embeds `app` verbatim and
/// resolves its references from `env`.
///
/// # Errors
///
/// Returns the parse error of either document, or of re-serialising the
/// merged one.
pub fn bake(
    target_arch: &str,
    app: &str,
    web: &str,
    env: &HashMap<String, String>,
) -> Result<Bake, serde_yaml_ng::Error> {
    let mut document: Value = serde_yaml_ng::from_str(app)?;
    let is_web = target_arch == "wasm32";
    let text = if is_web {
        merge(&mut document, serde_yaml_ng::from_str(web)?);
        serde_yaml_ng::to_string(&document)?
    } else {
        app.to_owned()
    };
    let mut refs = Vec::new();
    collect_refs(&document, "", &mut refs);
    let resolved = if is_web {
        Vec::new()
    } else {
        let mut names: Vec<&String> = refs.iter().map(|(_, name)| name).collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .filter_map(|name| {
                let value = env.get(name).filter(|value| !value.is_empty())?;
                Some((name.clone(), value.clone()))
            })
            .collect()
    };
    Ok(Bake {
        document: text,
        refs,
        resolved,
    })
}

fn collect_refs(value: &Value, path: &str, refs: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => {
            if let Some(name) = text.strip_prefix('$').filter(|_| !text.contains("${")) {
                refs.push((path.to_string(), name.to_string()));
                return;
            }
            let mut rest = text.as_str();
            while let Some(start) = rest.find("${") {
                let tail = &rest[start + 2..];
                let Some(end) = tail.find('}') else { break };
                refs.push((path.to_string(), tail[..end].to_string()));
                rest = &tail[end + 1..];
            }
        }
        Value::Sequence(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_refs(item, &format!("{path}[{index}]"), refs);
            }
        }
        Value::Mapping(entries) => {
            for (key, entry) in entries {
                let key = key.as_str().unwrap_or("?");
                let child = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                collect_refs(entry, &child, refs);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use kithara_test_utils::kithara;
    use serde_yaml_ng::Value;

    use super::bake;

    mod consts {
        pub(super) const APP: &str = "drm:\n  providers:\n    - key: $KITHARA_KEY\n      url: https://${KITHARA_HOST}/keys\n  unused: $KITHARA_EMPTY\n";
        pub(super) const WEB: &str = "drm:\n  providers: []\n  unused: null\n";
        pub(super) const KEY: &str = "kithara-bake-sentinel";
    }

    fn env() -> HashMap<String, String> {
        HashMap::from([
            ("KITHARA_KEY".to_owned(), consts::KEY.to_owned()),
            ("KITHARA_HOST".to_owned(), "keys.example".to_owned()),
            ("KITHARA_EMPTY".to_owned(), String::new()),
        ])
    }

    #[kithara::test(native)]
    fn a_wasm32_bake_lays_the_overlay_and_resolves_nothing() {
        let baked = bake("wasm32", consts::APP, consts::WEB, &env()).expect("both documents parse");

        let document: Value = serde_yaml_ng::from_str(&baked.document).expect("the bake parses");
        assert_eq!(document["drm"]["providers"], Value::Sequence(Vec::new()));
        assert!(baked.refs.is_empty());
        assert!(baked.resolved.is_empty());
        assert!(!baked.document.contains("KITHARA"));
    }

    #[kithara::test(native)]
    fn a_native_bake_embeds_the_document_verbatim_and_resolves_what_is_set() {
        let baked =
            bake("aarch64", consts::APP, consts::WEB, &env()).expect("both documents parse");

        assert_eq!(baked.document, consts::APP);
        assert_eq!(
            baked.refs,
            [
                ("drm.providers[0].key".to_owned(), "KITHARA_KEY".to_owned()),
                ("drm.providers[0].url".to_owned(), "KITHARA_HOST".to_owned()),
                ("drm.unused".to_owned(), "KITHARA_EMPTY".to_owned()),
            ]
        );
        assert_eq!(
            baked.resolved,
            [
                ("KITHARA_HOST".to_owned(), "keys.example".to_owned()),
                ("KITHARA_KEY".to_owned(), consts::KEY.to_owned()),
            ]
        );
    }
}
