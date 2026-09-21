use super::BlobError;

/// Little-endian append-only writer over a byte buffer.
pub struct Writer<'a>(&'a mut Vec<u8>);

impl<'a> Writer<'a> {
    pub const fn new(bytes: &'a mut Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub fn write_f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// Write a `u64` length prefix, clamping an oversized `usize` to `u64::MAX`
    /// (a length that always fails to read back).
    pub fn write_len(&mut self, len: usize) {
        self.write_u64(u64::try_from(len).unwrap_or(u64::MAX));
    }

    pub fn write_optional_u64(&mut self, value: Option<u64>) {
        self.write_bool(value.is_some());
        self.write_u64(value.unwrap_or(0));
    }

    /// # Errors
    ///
    /// Errors if the written section is longer than a `u64` can measure.
    pub fn write_section<F>(&mut self, write: F) -> Result<(), BlobError>
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

    /// Write a length-prefixed `f32` series.
    pub fn write_samples(&mut self, samples: &[f32]) {
        self.write_len(samples.len());
        for sample in samples {
            self.write_f32(*sample);
        }
    }

    /// # Errors
    ///
    /// Errors if the string is longer than a `u32` can measure.
    pub fn write_str(&mut self, value: &str) -> Result<(), BlobError> {
        let len = u32::try_from(value.len()).map_err(|_| BlobError::TooLarge)?;
        self.write_u32(len);
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn write_u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    delegate::delegate! {
        to self.0 {
            pub fn reserve(&mut self, extra: usize);
            #[call(push)]
            pub fn write_u8(&mut self, value: u8);
            #[call(extend_from_slice)]
            pub fn write_bytes(&mut self, bytes: &[u8]);
        }
    }
}
