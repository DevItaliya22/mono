// SPDX-License-Identifier: MPL-2.0

use serde_json::Value;

use crate::FrameError;

#[derive(Default)]
pub struct NdjsonDecoder {
    buffer: Vec<u8>,
}

impl NdjsonDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>, FrameError> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() {
                lines.push(decode_line(&line)?);
            }
        }
        Ok(lines)
    }

    pub fn finish(&mut self) -> Result<Option<Value>, FrameError> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        decode_line(&std::mem::take(&mut self.buffer)).map(Some)
    }
}

fn decode_line(line: &[u8]) -> Result<Value, FrameError> {
    serde_json::from_slice(line).map_err(|_| FrameError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_utf8_scalar_may_cross_chunks() {
        let encoded = serde_json::to_vec(&serde_json::json!({ "text": "🌿" })).unwrap();
        let split = encoded
            .windows("🌿".len())
            .position(|window| window == "🌿".as_bytes())
            .unwrap()
            + 2;
        let mut decoder = NdjsonDecoder::default();
        assert!(decoder.push(&encoded[..split]).unwrap().is_empty());
        assert_eq!(decoder.push(&encoded[split..]).unwrap().len(), 0);
        assert_eq!(decoder.finish().unwrap().unwrap()["text"], "🌿");
    }
}
