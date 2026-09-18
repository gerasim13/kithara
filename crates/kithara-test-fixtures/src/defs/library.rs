use kithara_platform::time::Duration;
use kithara_test_macros as kithara;
use url::Url;

use crate::{
    context::BuildContext,
    remote_file::{RemoteFileError, fetch_verified},
};

enum Library {}

impl Library {
    const BASE: &str = "https://stream.silvercomet.top/fixtures/";
    const ENV: &str = "KITHARA_REMOTE_FIXTURES";
    const TIMEOUT: Duration = Duration::from_secs(600);
}

fn enabled() -> Result<(), RemoteFileError> {
    std::env::var_os(Library::ENV)
        .filter(|value| !value.is_empty())
        .map(|_| ())
        .ok_or(RemoteFileError::Missing(Library::ENV))
}

#[kithara::asset(
    ext = "flac",
    content_type = "audio/flac",
    env = ["KITHARA_REMOTE_FIXTURES"],
    optional
)]
#[case::newtechno(
    "newtechno.flac",
    "7ee0e157a3dd1ea44554c9e22f81a72ed1100942a2f17982e90043f40801f1b2",
    35891249
)]
#[case::ryabina(
    "ryabina.flac",
    "6611598502e707ff6b7d46dd14bad002fb55264a9c3a47ad7ce8b739c254600c",
    27344988
)]
#[case::song1(
    "song1.flac",
    "896e3fc87978f84a7f7521dce99950df1b68eddc5e36e88ba4c49780bade0bc3",
    32001485
)]
#[case::dragoncoda(
    "dragoncoda.flac",
    "62dd0faa3735e02665b042abcb0c8dce53b0d1792644f00526eeab3881cb0d88",
    29600940
)]
#[case::newtriphop(
    "newtriphop.flac",
    "ba4a6ffbe7ce67c11dc690b9334c5dd261c84f4bd90e474637b280d3beeefeec",
    37192612
)]
#[case::slowtechno(
    "slowtechno.flac",
    "9453f53dd3f693c15b86da5604b3318b0da041fc2dc0849a01cf65fcf6fea0d9",
    28999562
)]
#[case::song2(
    "song2.flac",
    "a7edc94bc227fdb0d0f016b01e23a65821e54812ea0f05fb0a28c4db7855ae0d",
    23390927
)]
#[case::track05(
    "track05.flac",
    "0894d5bebccbfd5134957aa666dbda87f457222dcda1db2e773cee0135f06017",
    26305501
)]
#[case::c343(
    "c343.flac",
    "422b3c9e415098a2133592fa561612218300bf378c3f6b02dfb9f4c7ab6c9e84",
    53951343
)]
#[case::e101(
    "e101.flac",
    "bcbfdaba2f22d5f0ebb92d63b1e4b5e252bc61f3a99b9a2d85dd0f929efb6fcd",
    47687846
)]
#[case::g242(
    "g242.flac",
    "92dd30f8dace371e081685ee18b2ad34407360fd279b7b3a0f0ded78d3436789",
    55173208
)]
fn library_flac(
    _context: &BuildContext<'_>,
    file: &str,
    sha256: &str,
    length: u64,
) -> Result<Vec<u8>, RemoteFileError> {
    enabled()?;
    let url = Url::parse(Library::BASE)?.join(file)?;
    Ok(
        fetch_verified(&url, sha256, length, Library::TIMEOUT).unwrap_or_else(|error| {
            panic!("requested library fixture `{file}` failed verification: {error}")
        }),
    )
}

#[kithara::asset(
    ext = "analysis",
    content_type = "application/x-kithara-analysis",
    depends_on = ["library_flac_{case}"],
    env = ["KITHARA_REMOTE_FIXTURES"],
    optional
)]
#[case::newtechno()]
#[case::ryabina()]
#[case::song1()]
#[case::dragoncoda()]
#[case::newtriphop()]
#[case::slowtechno()]
#[case::song2()]
#[case::track05()]
#[case::c343()]
#[case::e101()]
#[case::g242()]
fn library_analysis(
    _context: &BuildContext<'_>,
    inputs: &[&[u8]],
) -> Result<Vec<u8>, RemoteFileError> {
    enabled()?;
    let flac = inputs
        .first()
        .ok_or(RemoteFileError::Missing("library_flac dependency"))?;
    let (artifact, frames) = super::rhythm::beat_flac(flac);
    Ok(super::rhythm::analysis_file(artifact, frames))
}
