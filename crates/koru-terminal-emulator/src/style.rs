// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::CellStyle;

/// The concrete code-family face requested by an SGR cell.
///
/// The renderer maps these variants to the packaged roman/italic face and a
/// typed 400/700 weight. It must not synthesize bold or italic while those
/// packaged faces are available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalFace {
    #[default]
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl TerminalFace {
    pub fn for_style(style: CellStyle) -> Self {
        match (style.bold, style.italic) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    pub fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    pub fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }
}
