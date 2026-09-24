#![forbid(unsafe_code)]

use std::{
    fs,
    fs::File,
    io::{Error, ErrorKind, Read, Write},
    path::PathBuf,
};

use kithara_bufpool::ByteBuffer;
use tempfile::NamedTempFile;

use crate::error::{AssetsError, AssetsResult};

/// One on-disk index snapshot, read and replaced whole.
///
/// No handle outlives a call: a read copies the file into the caller's
/// buffer and a write publishes a sibling temp file by rename. Nothing keeps
/// the file open or mapped between flushes, so no platform can refuse the
/// rename that replaces it.
pub(crate) struct IndexFile {
    path: PathBuf,
}

impl IndexFile {
    pub(crate) const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Read the whole snapshot into `buf`, which ends up holding exactly the
    /// bytes read. A file that does not exist yet reads as empty.
    pub(crate) fn read_into(&self, buf: &mut ByteBuffer) -> AssetsResult<()> {
        buf.clear();
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|error| AssetsError::Io(Error::other(error)))?;
        buf.ensure_len(len)?;
        let mut read = 0;
        while read < len {
            match file.read(&mut buf[read..len])? {
                0 => break,
                n => read += n,
            }
        }
        buf.truncate(read);
        Ok(())
    }

    /// Replace the snapshot with `bytes`. `durable` forces them onto the
    /// medium before the rename names them.
    pub(crate) fn write(&self, bytes: &[u8], durable: bool) -> AssetsResult<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::other("index file has no parent directory"))?;
        fs::create_dir_all(parent)?;
        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(bytes)?;
        if durable {
            tmp.as_file().sync_data()?;
        }
        tmp.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}
