#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsStr;

#[cfg(not(target_arch = "wasm32"))]
use anyhow::{Context, Result, bail};
#[cfg(not(target_arch = "wasm32"))]
use kithara::platform::tokio;
#[cfg(not(target_arch = "wasm32"))]
use kithara_integration_tests::{
    TestServerHelper, cancel_token,
    sync_fixture::{RepositoryMp3, SyncAnalysisFixtures, repository_mp3, silvercomet_hls},
};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(OsStr::new("write")) || arguments.next().is_some() {
        bail!("usage: sync-analysis-fixtures write");
    }

    let server = TestServerHelper::new().await;
    let fixtures = SyncAnalysisFixtures::production()
        .context("initialize production sync analysis fixtures")?;
    let cancel = cancel_token();
    let tracks = [
        repository_mp3(&server, RepositoryMp3::Test).await?,
        repository_mp3(&server, RepositoryMp3::Silvercomet).await?,
        silvercomet_hls(&server).await?,
    ];
    for track in tracks {
        let path = fixtures
            .write_prepared(&cancel, &track)
            .await
            .with_context(|| format!("prepare analysis for '{}'", track.media()))?;
        println!("{} -> {}", track.media(), path.display());
    }
    Ok(())
}
