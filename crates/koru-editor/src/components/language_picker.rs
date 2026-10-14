// SPDX-License-Identifier: MPL-2.0

//! The language chip's dropdown: pick a grammar for a code block, or "Auto".
//!
//! Opens from a hover on the chip rather than from typed text, so it has no
//! trigger and no text field — it is a flat list of a curated set of common
//! languages, arrow-key navigable and click-able. The fence (``` ```rust ``)
//! covers anything not listed here; this is the always-available override.

use winit::keyboard::NamedKey;

use super::Outcome;
use koru_core::NodeId;
use koru_draw::text::TextEngine;
use koru_draw::theme;

/// The languages the dropdown offers, in menu order. The token is what
/// `set_language` stores and `koru_blocks::resolve` understands. Kept short —
/// the fence (``` ```rust ```) covers anything not listed; this is the
/// always-available override for the common ones.
const LANGUAGES: &[(&str, &str)] = &[
    ("Auto", ""),
    ("Rust", "rust"),
    ("Python", "python"),
    ("JavaScript", "javascript"),
    ("TypeScript", "typescript"),
    ("JSON", "json"),
    ("HTML", "html"),
    ("CSS", "css"),
    ("Bash", "bash"),
    ("Go", "go"),
];

pub struct LanguagePicker {
    /// The code block whose grammar is being chosen.
    pub node: NodeId,
    pub sel: usize,
    /// The panel width, measured from the labels so the dropdown wraps its text
    /// with padding rather than a fixed width. `0.0` until measured; `panel_size`
    /// floors it.
    pub width: f32,
}

impl LanguagePicker {
    /// Open for `node`, with the block's current grammar preselected — the
    /// same bargain a native dropdown makes: what the chip says is what the
    /// list highlights, and Enter (or a click) confirms it. A token the list
    /// does not know starts at Auto rather than refusing to open; the fence
    /// remains the way back for anything exotic.
    pub fn open_for(node: NodeId, current: Option<&str>) -> Self {
        let sel = current
            .and_then(|token| LANGUAGES.iter().position(|(_, t)| *t == token))
            .unwrap_or(0);
        Self {
            node,
            sel,
            width: 0.0,
        }
    }

    /// The panel width that fits the widest language label plus inner padding,
    /// so the dropdown is only as wide as its content.
    pub fn measure_width(engine: &mut TextEngine) -> f32 {
        let widest = LANGUAGES
            .iter()
            .map(|(name, _)| engine.simple_layout(name, theme::role::ui()).width())
            .fold(0.0_f32, f32::max);
        widest + 20.0
    }

    pub fn height(&self) -> f32 {
        theme::PANEL_PAD * 2.0 + LANGUAGES.len() as f32 * theme::ROW_H
    }

    pub fn key(&mut self, named: Option<NamedKey>) -> Outcome {
        match named {
            Some(NamedKey::ArrowDown) => {
                self.sel = (self.sel + 1) % LANGUAGES.len();
                Outcome::Consumed
            }
            Some(NamedKey::ArrowUp) => {
                self.sel = (self.sel + LANGUAGES.len().saturating_sub(1)) % LANGUAGES.len();
                Outcome::Consumed
            }
            Some(NamedKey::Enter) | Some(NamedKey::Tab) => self.choose(self.sel),
            _ => Outcome::Ignored,
        }
    }

    pub fn click(&self, local_y: f32) -> Outcome {
        let row = (local_y / theme::ROW_H).floor();
        if row < 0.0 {
            return Outcome::Consumed;
        }
        self.choose(row as usize)
    }

    fn choose(&self, i: usize) -> Outcome {
        match LANGUAGES.get(i) {
            Some((_, "")) => Outcome::SetLanguage(self.node, None),
            Some((_, token)) => Outcome::SetLanguage(self.node, Some((*token).to_string())),
            None => Outcome::Close,
        }
    }
}

/// The labels in menu order, for the panel's draw path.
pub fn labels() -> &'static [(&'static str, &'static str)] {
    LANGUAGES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_first_and_clears_the_language() {
        let p = LanguagePicker::open_for(NodeId::new(), None);
        assert_eq!(LANGUAGES[0].0, "Auto");
        assert!(matches!(p.choose(0), Outcome::SetLanguage(_, None),));
    }

    #[test]
    fn a_named_language_carries_its_token() {
        let p = LanguagePicker::open_for(NodeId::new(), None);
        let rust = LANGUAGES.iter().position(|(n, _)| *n == "Rust").unwrap();
        let chosen = p.choose(rust);
        assert!(
            matches!(&chosen, Outcome::SetLanguage(_, Some(l)) if l == "rust"),
            "Rust should carry the \"rust\" token, got {chosen:?}"
        );
        // The node carried is the one the picker was opened for.
        assert!(matches!(chosen, Outcome::SetLanguage(_, _)));
    }

    #[test]
    fn arrow_keys_wrap_and_enter_chooses() {
        let mut p = LanguagePicker::open_for(NodeId::new(), None);
        let last = LANGUAGES.len() - 1;
        p.sel = last;
        p.key(Some(NamedKey::ArrowDown));
        assert_eq!(p.sel, 0, "wraps down to the first");
        p.key(Some(NamedKey::ArrowUp));
        assert_eq!(p.sel, last, "wraps up to the last");
        assert!(matches!(
            p.key(Some(NamedKey::Enter)),
            Outcome::SetLanguage(_, _),
        ));
    }

    /// **The picker never offers what the engine cannot colour.** Every
    /// non-empty token must resolve in `koru-blocks` and produce ranges.
    /// TypeScript once rendered plain — syntect's bundled set does not carry
    /// it, so `resolve` answered None and nothing downstream ever noticed —
    /// which is exactly the failure class this closes.
    #[test]
    fn every_token_the_picker_offers_highlights() {
        for (name, token) in labels() {
            if token.is_empty() {
                continue; // Auto: no grammar by definition.
            }
            let sample = format!("// {name}\nlet value = \"{name}\";");
            let ranges = koru_blocks::highlight(&sample, Some(token), false);
            assert!(
                !ranges.is_empty(),
                "{name} (token {token:?}) resolved to no colour"
            );
        }
    }

    /// The dropdown opens on what the chip already says: a block set to
    /// Python lists Python active, and Enter confirms it rather than
    /// silently falling back to Auto.
    #[test]
    fn opening_preselects_the_current_language() {
        let python = LANGUAGES.iter().position(|(n, _)| *n == "Python").unwrap();
        let mut p = LanguagePicker::open_for(NodeId::new(), Some("python"));
        assert_eq!(p.sel, python);
        assert!(matches!(
            p.key(Some(NamedKey::Enter)),
            Outcome::SetLanguage(_, Some(t)) if t == "python",
        ));

        // No token is Auto; one the list has never heard of starts over too,
        // and Enter there clears back to Auto rather than keeping the mystery.
        for current in [None, Some("klingon")] {
            let mut p = LanguagePicker::open_for(NodeId::new(), current);
            assert_eq!(p.sel, 0, "{current:?} starts at Auto");
            assert!(matches!(
                p.key(Some(NamedKey::Enter)),
                Outcome::SetLanguage(_, None),
            ));
        }
    }
}
