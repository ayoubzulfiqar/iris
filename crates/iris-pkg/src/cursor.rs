//! Little-endian cursor over a borrowed archive or texture.

use crate::error::Error;
use crate::limits;

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(crate) fn read_exact(
        &mut self,
        len: usize,
        context: &'static str,
    ) -> Result<&'a [u8], Error> {
        let available = self.remaining();
        if len > available {
            return Err(Error::UnexpectedEof {
                context,
                expected: len,
                available,
            });
        }
        let start = self.pos;
        self.pos = start.saturating_add(len);
        self.data
            .get(start..self.pos)
            .ok_or(Error::Invariant("cursor window"))
    }

    pub(crate) fn bytes_vec(&mut self, len: u32, context: &'static str) -> Result<Vec<u8>, Error> {
        let len = usize::try_from(len).map_err(|_| Error::Limit {
            context,
            value: u64::from(len),
            limit: limits::MAX_MIP_BYTES as u64,
        })?;
        if len > limits::MAX_MIP_BYTES {
            return Err(Error::Limit {
                context,
                value: len as u64,
                limit: limits::MAX_MIP_BYTES as u64,
            });
        }
        Ok(self.read_exact(len, context)?.to_vec())
    }

    fn array4(&mut self, context: &'static str) -> Result<[u8; 4], Error> {
        let bytes = self.read_exact(4, context)?;
        bytes
            .try_into()
            .map_err(|_| Error::Invariant("4 byte read"))
    }

    pub(crate) fn u32(&mut self, context: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array4(context)?))
    }

    pub(crate) fn i32(&mut self, context: &'static str) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array4(context)?))
    }

    pub(crate) fn f32(&mut self, context: &'static str) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array4(context)?))
    }

    pub(crate) fn sized_string(
        &mut self,
        context: &'static str,
        max: usize,
    ) -> Result<String, Error> {
        let len_u32 = self.u32(context)?;
        let len = usize::try_from(len_u32).map_err(|_| Error::Limit {
            context,
            value: u64::from(len_u32),
            limit: max as u64,
        })?;
        if len > max {
            return Err(Error::Limit {
                context,
                value: len as u64,
                limit: max as u64,
            });
        }
        let bytes = self.read_exact(len, context)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::InvalidUtf8 { context })
    }

    pub(crate) fn cstring(&mut self, context: &'static str, max: usize) -> Result<String, Error> {
        let available = self.remaining();
        let window = available.min(max.saturating_add(1));
        let start = self.pos;
        let end = start.saturating_add(window);
        let slice = self
            .data
            .get(start..end)
            .ok_or(Error::Invariant("cstring window"))?;
        let Some(nul) = slice.iter().position(|byte| *byte == 0) else {
            if available > max {
                return Err(Error::Limit {
                    context,
                    value: available as u64,
                    limit: max as u64,
                });
            }
            return Err(Error::UnexpectedEof {
                context,
                expected: 1,
                available,
            });
        };
        let bytes = slice.get(..nul).ok_or(Error::Invariant("cstring body"))?;
        let text = std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 { context })?;
        self.pos = start + nul + 1;
        Ok(text.to_owned())
    }
}
