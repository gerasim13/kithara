use super::BlobError;

/// Little-endian append-only writer over a byte buffer.
pub(crate) struct Writer<'a>(&'a mut Vec<u8>);

impl<'a> Writer<'a> {
    pub(crate) const fn new(bytes: &'a mut Vec<u8>) -> Self {
        Self(bytes)
    }

    pub(crate) fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub(crate) fn write_f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// Write a `u64` length prefix, clamping an oversized `usize` to `u64::MAX`
    /// (a length that always fails to read back).
    pub(crate) fn write_len(&mut self, len: usize) {
        self.write_u64(u64::try_from(len).unwrap_or(u64::MAX));
    }

    pub(crate) fn write_optional_u64(&mut self, value: Option<u64>) {
        self.write_bool(value.is_some());
        self.write_u64(value.unwrap_or(0));
    }

    pub(crate) fn write_section<F>(&mut self, write: F) -> Result<(), BlobError>
    where
        F: FnOnce(&mut Vec<u8>),
    {
        let len_offset = self.0.len();
        self.write_u64(0);
        let section_offset = self.0.len();
        write(self.0);
        let len = u64::try_from(self.0.len() - section_offset).map_err(|_| BlobError::TooLarge)?;
        self.0[len_offset..section_offset].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }

    pub(crate) fn write_str(&mut self, value: &str) -> Result<(), BlobError> {
        let len = u32::try_from(value.len()).map_err(|_| BlobError::TooLarge)?;
        self.write_u32(len);
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub(crate) fn write_u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    delegate::delegate! {
        to self.0 {
            pub(crate) fn reserve(&mut self, extra: usize);
            #[call(push)]
            pub(crate) fn write_u8(&mut self, value: u8);
            #[call(extend_from_slice)]
            pub(crate) fn write_bytes(&mut self, bytes: &[u8]);
        }
    }
}
