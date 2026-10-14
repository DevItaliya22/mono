// SPDX-License-Identifier: MPL-2.0

use crate::FrameError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseFrame>, FrameError> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&line, &mut frames)?;
        }
        Ok(frames)
    }

    pub fn finish(&mut self) -> Result<Vec<SseFrame>, FrameError> {
        let mut frames = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.line(&line, &mut frames)?;
        }
        if self.event.is_some() || !self.data.is_empty() {
            self.dispatch(&mut frames);
        }
        Ok(frames)
    }

    fn line(&mut self, line: &[u8], frames: &mut Vec<SseFrame>) -> Result<(), FrameError> {
        let line = std::str::from_utf8(line).map_err(|_| FrameError::Decode)?;
        if line.is_empty() {
            self.dispatch(frames);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            // `id` and `retry` are transport metadata Koru does not consume.
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, frames: &mut Vec<SseFrame>) {
        if !self.data.is_empty() {
            frames.push(SseFrame {
                event: self.event.take(),
                data: self.data.join("\n"),
            });
        } else {
            self.event = None;
        }
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_multiline_crlf_comments_and_retry_fields() {
        let mut decoder = SseDecoder::default();
        let frames = decoder
            .push(b": warm\r\nretry: 500\r\nevent: response.created\r\ndata: {\"a\":\r\ndata: 1}\r\n\r\n")
            .unwrap();
        assert_eq!(
            frames,
            vec![SseFrame {
                event: Some("response.created".into()),
                data: "{\"a\":\n1}".into(),
            }]
        );
    }

    #[test]
    fn a_utf8_scalar_may_cross_chunks() {
        let bytes = "data: {\"text\":\"🌿\"}\n\n".as_bytes();
        let split = bytes
            .windows("🌿".len())
            .position(|window| window == "🌿".as_bytes())
            .unwrap()
            + 2;
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        let frames = decoder.push(&bytes[split..]).unwrap();
        assert_eq!(frames[0].data, "{\"text\":\"🌿\"}");
    }
}
