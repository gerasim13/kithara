use super::BlobError;

/// Little-endian cursor reader over a byte slice.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    /// Succeed only if the whole blob was consumed.
    pub(crate) const fn finish(&self) -> Result<(), BlobError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(BlobError::Corrupt)
        }
    }

    pub(crate) fn read_array<const N: usize>(&mut self) -> Result<[u8; N], BlobError> {
        let end = self.cursor.checked_add(N).ok_or(BlobError::Corrupt)?;
        let chunk = self.bytes.get(self.cursor..end).ok_or(BlobError::Corrupt)?;
        let mut out = [0u8; N];
        out.copy_from_slice(chunk);
        self.cursor = end;
        Ok(out)
    }

    pub(crate) fn read_f32(&mut self) -> Result<f32, BlobError> {
        Ok(f32::from_le_bytes(self.read_array::<4>()?))
    }

    pub(crate) fn read_f64(&mut self) -> Result<f64, BlobError> {
        Ok(f64::from_le_bytes(self.read_array::<8>()?))
    }

    /// Read a `u64` length prefix as a `usize`.
    pub(crate) fn read_len(&mut self) -> Result<usize, BlobError> {
        usize::try_from(self.read_u64()?).map_err(|_| BlobError::Corrupt)
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, BlobError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    pub(crate) fn read_u64(&mut self) -> Result<u64, BlobError> {
        Ok(u64::from_le_bytes(self.read_array::<8>()?))
    }

    /// Bytes not yet consumed.
    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len() - self.cursor
    }
}
