// SPDX-License-Identifier: MPL-2.0

use std::io::Read;

use crate::{READ_BATCH_BYTES, TransportError, TransportResult, TransportStage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadBatch {
    Bytes(usize),
    Eof,
}

pub struct PtyReader {
    pub(crate) inner: Box<dyn Read + Send>,
    eof_seen: bool,
}

impl PtyReader {
    pub(crate) fn new(inner: Box<dyn Read + Send>) -> Self {
        Self {
            inner,
            eof_seen: false,
        }
    }

    pub fn read_batch(&mut self, buffer: &mut [u8]) -> TransportResult<ReadBatch> {
        if self.eof_seen || buffer.is_empty() {
            return Err(TransportError::stage(TransportStage::InvalidRequest));
        }
        match self.inner.read(buffer) {
            Ok(0) => {
                self.eof_seen = true;
                Ok(ReadBatch::Eof)
            }
            Ok(count) => Ok(ReadBatch::Bytes(count)),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                self.eof_seen = true;
                Ok(ReadBatch::Eof)
            }
            Err(error) => Err(TransportError::at(TransportStage::Read, &error)),
        }
    }

    /// Block until EOF, parsing each borrowed OS read before the next read.
    pub fn run(mut self, mut parse: impl FnMut(&[u8])) -> TransportResult<()> {
        let mut buffer = vec![0; READ_BATCH_BYTES];
        loop {
            match self.read_batch(&mut buffer)? {
                ReadBatch::Bytes(count) => parse(&buffer[..count]),
                ReadBatch::Eof => return Ok(()),
            }
        }
    }
}
