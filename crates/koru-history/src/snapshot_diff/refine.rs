// SPDX-License-Identifier: MPL-2.0

//! Word/grapheme refinement inside paired modified blocks.

use crate::TextSpan;
use crate::text_diff;

/// Refine the difference between two block texts into [`TextSpan`]s.
///
/// Uses Unicode word boundaries via [`unicode_segmentation`], which naturally
/// falls back to grapheme-level tokens for CJK and emoji, so graphemes are
/// never split.
pub fn refine_text(before: &str, after: &str) -> Vec<TextSpan> {
    text_diff(before, after)
}
