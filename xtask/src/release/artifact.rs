use std::{fs, io, path::Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// The sha256 of a file, hex-encoded. For the framework zip this is also the
/// checksum Swift Package Manager verifies a binary target against.
pub(crate) fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut DigestWriter(&mut digest))
        .with_context(|| format!("hashing {}", path.display()))?;
    Ok(hex::encode(digest.finalize()))
}

struct DigestWriter<'a, D>(&'a mut D);

impl<D: Digest> io::Write for DigestWriter<'_, D> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .with_context(|| format!("{} has no file name", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_the_hex_digest_of_the_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.zip");
        fs::write(&path, b"abc").unwrap();

        assert_eq!(
            sha256(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
