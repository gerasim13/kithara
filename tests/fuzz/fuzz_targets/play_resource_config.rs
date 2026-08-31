#![no_main]

use std::sync::LazyLock;

use arbitrary::Arbitrary;
use kithara::{
    assets::{AssetStore, StorageBackend},
    bufpool::testing::{Pools, TestPools, pools},
    play::{ResourceConfig, ResourceSrc},
};
use libfuzzer_sys::fuzz_target;

static POOLS: LazyLock<Pools> = LazyLock::new(pools);
static STORE: LazyLock<AssetStore<TestPools>> = LazyLock::new(|| {
    AssetStore::builder(POOLS.clone())
        .backend(StorageBackend::Memory)
        .build()
});

#[derive(Arbitrary, Debug)]
struct Input {
    raw: Vec<u8>,
}

fuzz_target!(|input: Input| {
    let mut raw = input.raw;
    raw.truncate(4 * 1024);

    let text = String::from_utf8_lossy(&raw);
    let _ = ResourceSrc::parse(text.as_ref()).map(|src| {
        ResourceConfig::<TestPools>::for_src(src)
            .store(STORE.clone())
            .build()
    });
});
