// SPDX-License-Identifier: MPL-2.0

use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi;

use crate::MAX_OSC_BYTES;
use crate::event::Listener;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc {
        forwarded: usize,
    },
}

/// Streaming guard around VTE's growable `std` OSC buffer.
///
/// Ordinary output is passed in contiguous slices. Only an OSC payload beyond
/// the fixed budget is dropped, while its terminator is still forwarded so
/// parser state recovers across arbitrary PTY read boundaries.
pub(crate) struct BoundedProcessor {
    inner: ansi::Processor,
    state: State,
}

impl Default for BoundedProcessor {
    fn default() -> Self {
        Self {
            inner: ansi::Processor::new(),
            state: State::Ground,
        }
    }
}

impl BoundedProcessor {
    pub(crate) fn advance(&mut self, term: &mut Term<Listener>, bytes: &[u8]) {
        let mut offset = 0;
        while offset < bytes.len() {
            match self.state {
                State::Ground => {
                    let end = bytes[offset..]
                        .iter()
                        .position(|byte| *byte == 0x1b)
                        .map_or(bytes.len(), |position| offset + position + 1);
                    self.inner.advance(term, &bytes[offset..end]);
                    if bytes[end.saturating_sub(1)] == 0x1b {
                        self.state = State::Escape;
                    }
                    offset = end;
                }
                State::Escape => {
                    let byte = bytes[offset];
                    self.inner.advance(term, std::slice::from_ref(&byte));
                    self.state = match byte {
                        b']' => State::Osc { forwarded: 0 },
                        0x1b => State::Escape,
                        _ => State::Ground,
                    };
                    offset += 1;
                }
                State::Osc { forwarded } => {
                    let tail = &bytes[offset..];
                    let terminator = tail
                        .iter()
                        .position(|byte| matches!(*byte, 0x07 | 0x18 | 0x1a | 0x1b));
                    let payload_end = terminator.map_or(bytes.len(), |position| offset + position);
                    let allowed = MAX_OSC_BYTES.saturating_sub(forwarded);
                    let pass_end = payload_end.min(offset + allowed);
                    if pass_end > offset {
                        self.inner.advance(term, &bytes[offset..pass_end]);
                    }
                    let passed = pass_end - offset;
                    offset = payload_end;
                    self.state = State::Osc {
                        forwarded: forwarded + passed,
                    };

                    if terminator.is_some() {
                        let byte = bytes[offset];
                        self.inner.advance(term, std::slice::from_ref(&byte));
                        self.state = if byte == 0x1b {
                            State::Escape
                        } else {
                            State::Ground
                        };
                        offset += 1;
                    }
                }
            }
        }
    }
}
