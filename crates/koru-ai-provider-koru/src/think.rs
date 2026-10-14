// SPDX-License-Identifier: MPL-2.0

//! Split streamed Chat Completions content into visible text and thoughts.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Reasoning(String),
}

#[derive(Default)]
pub struct ThinkSplitter {
    in_think: bool,
    pending: String,
}

impl ThinkSplitter {
    pub fn push(&mut self, chunk: &str) -> Vec<Piece> {
        if chunk.is_empty() {
            return Vec::new();
        }
        self.pending.push_str(chunk);
        let mut out = Vec::new();
        loop {
            if self.in_think {
                if let Some(end) = self.pending.find(CLOSE) {
                    emit(&mut out, Piece::Reasoning(self.pending[..end].to_owned()));
                    self.pending.replace_range(..end + CLOSE.len(), "");
                    self.in_think = false;
                } else {
                    hold_incomplete(&mut self.pending, CLOSE, |kept| {
                        emit(&mut out, Piece::Reasoning(kept));
                    });
                    break;
                }
            } else if let Some(start) = self.pending.find(OPEN) {
                emit(&mut out, Piece::Text(self.pending[..start].to_owned()));
                self.pending.replace_range(..start + OPEN.len(), "");
                self.in_think = true;
            } else {
                hold_incomplete(&mut self.pending, OPEN, |kept| {
                    emit(&mut out, Piece::Text(kept));
                });
                break;
            }
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Piece> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.pending);
        if self.in_think {
            vec![Piece::Reasoning(rest)]
        } else {
            vec![Piece::Text(rest)]
        }
    }
}

fn emit(out: &mut Vec<Piece>, piece: Piece) {
    let empty = match &piece {
        Piece::Text(text) | Piece::Reasoning(text) => text.is_empty(),
    };
    if !empty {
        out.push(piece);
    }
}

fn hold_incomplete(pending: &mut String, tag: &str, emit_kept: impl FnOnce(String)) {
    let keep = (1..tag.len())
        .rev()
        .find(|&n| pending.ends_with(&tag[..n]))
        .unwrap_or(0);
    let emit_len = pending.len() - keep;
    if emit_len > 0 {
        let kept = pending[..emit_len].to_owned();
        pending.replace_range(..emit_len, "");
        emit_kept(kept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_tags_across_chunks_never_leak_into_text() {
        let mut splitter = ThinkSplitter::default();
        let first = splitter.push("hello <th");
        assert_eq!(first, vec![Piece::Text("hello ".into())]);
        let second = splitter.push("ink>secret</th");
        assert_eq!(second, vec![Piece::Reasoning("secret".into())]);
        let third = splitter.push("ink> visible");
        assert_eq!(third, vec![Piece::Text(" visible".into())]);
        assert!(splitter.finish().is_empty());
    }

    #[test]
    fn reasoning_content_without_tags_is_left_to_the_caller() {
        let mut splitter = ThinkSplitter::default();
        assert_eq!(splitter.push("plain"), vec![Piece::Text("plain".into())]);
    }
}
