include!(concat!(env!("OUT_DIR"), "/app_config_baked.rs"));

#[cfg(test)]
mod tests {
    use kithara::assets::{AssetSource, StorageBackend};
    use url::Url;

    use super::build_baked_asset_layouts;
    use crate::pools::{self, AppPools, AppStore};

    fn source(id: &str, expires: &str) -> AssetSource {
        AssetSource::Remote {
            url: Url::parse(&format!(
                "https://cdn-edge.zvq.me/track/streamhq?id={id}&expires={expires}"
            ))
            .expect("valid source URL"),
            discriminator: None,
        }
    }

    #[kithara::test(native, flash(false))]
    fn baked_query_identity_separates_tracks_without_splitting_renewed_urls() {
        let store = AppStore::builder(pools::build().expect("valid app pool policy"))
            .backend(StorageBackend::Memory)
            .layouts(build_baked_asset_layouts())
            .build();
        let first = source("27390231", "100");
        let renewed = source("27390231", "200");
        let second = source("151585912", "100");

        let file_first = store
            .scope::<kithara::file::File<AppPools>>(&first)
            .expect("first file scope");
        let file_renewed = store
            .scope::<kithara::file::File<AppPools>>(&renewed)
            .expect("renewed file scope");
        let file_second = store
            .scope::<kithara::file::File<AppPools>>(&second)
            .expect("second file scope");
        let hls_first = store
            .scope::<kithara::hls::Hls<AppPools>>(&first)
            .expect("first HLS scope");
        let hls_renewed = store
            .scope::<kithara::hls::Hls<AppPools>>(&renewed)
            .expect("renewed HLS scope");
        let hls_second = store
            .scope::<kithara::hls::Hls<AppPools>>(&second)
            .expect("second HLS scope");

        assert_eq!(file_first.asset_root(), file_renewed.asset_root());
        assert_ne!(file_first.asset_root(), file_second.asset_root());
        assert_eq!(hls_first.asset_root(), hls_renewed.asset_root());
        assert_ne!(hls_first.asset_root(), hls_second.asset_root());
    }
}
