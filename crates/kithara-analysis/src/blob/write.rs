use kithara_bufpool::{ByteBuffer, PoolError};

use super::{Blob, BlobError};

/// Little-endian append-only writer over a byte buffer.
pub(crate) struct Writer<'a> {
    bytes: Output<'a>,
    error: Option<PoolError>,
}

enum Output<'a> {
    Pooled(&'a mut ByteBuffer),
    Vec(&'a mut Vec<u8>),
}

impl<'a> Writer<'a> {
    pub(crate) const fn new(bytes: &'a mut Vec<u8>) -> Self {
        Self {
            bytes: Output::Vec(bytes),
            error: None,
        }
    }

    pub(crate) const fn pooled(bytes: &'a mut ByteBuffer) -> Self {
        Self {
            bytes: Output::Pooled(bytes),
            error: None,
        }
    }

    pub(crate) fn reserve(&mut self, extra: usize) {
        if self.error.is_some() {
            return;
        }
        match &mut self.bytes {
            Output::Vec(bytes) => bytes.reserve(extra),
            Output::Pooled(bytes) => {
                let len = bytes.len();
                let Some(target) = len.checked_add(extra) else {
                    self.error = Some(PoolError::CapacityOverflow {
                        elements: usize::MAX,
                        element_size: 1,
                    });
                    return;
                };
                if let Err(error) = bytes.ensure_len(target) {
                    self.error = Some(error);
                } else {
                    bytes.truncate(len);
                }
            }
        }
    }

    pub(crate) fn write_u8(&mut self, value: u8) {
        self.extend_from_slice(&[value]);
    }

    pub(crate) fn write_f32(&mut self, value: f32) {
        self.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_f64(&mut self, value: f64) {
        self.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    /// Write a `u64` length prefix, clamping an oversized `usize` to `u64::MAX`
    /// (a length that always fails to read back).
    pub(crate) fn write_len(&mut self, len: usize) {
        self.write_u64(u64::try_from(len).unwrap_or(u64::MAX));
    }

    pub(crate) fn write_u32(&mut self, value: u32) {
        self.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_optional_u64(&mut self, value: Option<u64>) {
        self.write_bool(value.is_some());
        self.write_u64(value.unwrap_or(0));
    }

    pub(crate) fn write_section<F>(&mut self, write: F) -> Result<(), BlobError>
    where
        F: FnOnce(&mut Self) -> Result<(), BlobError>,
    {
        let len_offset = self.len();
        self.write_u64(0);
        let section_offset = self.len();
        write(self)?;
        self.result()?;
        let len = u64::try_from(self.len() - section_offset).map_err(|_| BlobError::TooLarge)?;
        self.as_mut_slice()[len_offset..section_offset].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }

    pub(crate) fn write_str(&mut self, value: &str) -> Result<(), BlobError> {
        let len = u32::try_from(value.len()).map_err(|_| BlobError::TooLarge)?;
        self.write_u32(len);
        self.extend_from_slice(value.as_bytes());
        self.result()
    }

    pub(crate) fn write_blob<T: Blob>(&mut self, value: &T) -> Result<(), BlobError> {
        self.write_u32(T::VERSION);
        value.encode(self);
        self.result()
    }

    pub(crate) fn result(&self) -> Result<(), BlobError> {
        self.error
            .clone()
            .map_or(Ok(()), |error| Err(BlobError::Pool(error)))
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        match &mut self.bytes {
            Output::Pooled(bytes) => bytes,
            Output::Vec(bytes) => bytes,
        }
    }

    fn extend_from_slice(&mut self, values: &[u8]) {
        if self.error.is_some() {
            return;
        }
        match &mut self.bytes {
            Output::Vec(bytes) => bytes.extend_from_slice(values),
            Output::Pooled(bytes) => {
                if let Err(error) = bytes.try_extend_from_slice(values) {
                    self.error = Some(error);
                }
            }
        }
    }

    fn len(&self) -> usize {
        match &self.bytes {
            Output::Pooled(bytes) => bytes.len(),
            Output::Vec(bytes) => bytes.len(),
        }
    }
}
