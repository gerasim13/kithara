use std::collections::HashMap;

use kithara_app_document::bake;
use serde_yaml_ng::Value;

mod consts {
    pub(super) const APP: &str = include_str!("../../app.yaml");
    pub(super) const WEB: &str = include_str!("../../app.web.yaml");
    pub(super) const SENTINEL: &str = "kithara-bake-sentinel";
}

fn env() -> HashMap<String, String> {
    HashMap::from([(
        "KITHARA_DRM_PROD_KEY".to_owned(),
        consts::SENTINEL.to_owned(),
    )])
}

#[kithara::test(native)]
fn the_web_bundle_carries_no_drm_provider_and_no_reference() {
    let baked = bake("wasm32", consts::APP, consts::WEB, &env()).expect("both documents parse");

    let document: Value = serde_yaml_ng::from_str(&baked.document).expect("the bake parses");
    assert_eq!(document["drm"]["providers"], Value::Sequence(Vec::new()));
    assert!(baked.refs.is_empty());
    assert!(baked.resolved.is_empty());
    assert!(!baked.document.contains("KITHARA"));
    assert!(!baked.document.contains(consts::SENTINEL));
}

#[kithara::test(native)]
fn a_native_build_resolves_the_production_drm_key() {
    let baked = bake("aarch64", consts::APP, consts::WEB, &env()).expect("both documents parse");

    assert_eq!(baked.document, consts::APP);
    assert_eq!(
        baked.resolved,
        [(
            "KITHARA_DRM_PROD_KEY".to_owned(),
            consts::SENTINEL.to_owned()
        )]
    );
}
