// SPDX-License-Identifier: MPL-2.0

//! Shared desktop account chooser. It replaces the pane while open, so narrow
//! windows and keyboard navigation use the same bounded row geometry as paint.
use koru_agent_wire::{AccountCommand, AccountState};
use koru_draw::{geometry::Rect, scene, text::TextEngine, theme};
use koru_i18n::{Key as I18nKey, Lang, t};
use vello::{Scene, kurbo::Affine};
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct AccountSwitcher {
    pub open: bool,
    focus: usize,
    first: usize,
}
impl AccountSwitcher {
    fn capacity(height: f32) -> usize {
        ((height - 144.0) / 52.0).floor().max(1.0) as usize
    }
    fn row(width: f32, index: usize) -> Rect {
        Rect::new(
            24.0,
            88.0 + index as f32 * 52.0,
            (width - 48.0).max(1.0),
            44.0,
        )
    }
    fn count(state: &AccountState) -> usize {
        state.held_accounts.len() + 2
    }
    fn reveal(&mut self, height: f32) {
        let capacity = Self::capacity(height);
        if self.focus < self.first {
            self.first = self.focus;
        }
        if self.focus >= self.first + capacity {
            self.first = self.focus + 1 - capacity;
        }
    }
    fn choose(&mut self, state: &AccountState, lang: Lang) -> Option<AccountCommand> {
        if self.focus <= state.held_accounts.len() && !state.capability {
            return None;
        }
        self.open = false;
        if let Some(row) = state.held_accounts.get(self.focus) {
            (row.scope_id != state.active_scope).then(|| AccountCommand::SwitchAccount {
                account_id: row.account_id.clone(),
            })
        } else if self.focus == state.held_accounts.len() {
            Some(AccountCommand::AddAccount {
                locale: lang.tag().into(),
            })
        } else {
            None
        }
    }
    pub fn click(
        &mut self,
        state: &AccountState,
        lang: Lang,
        size: (f32, f32),
        x: f32,
        y: f32,
    ) -> Option<AccountCommand> {
        for index in self.first..Self::count(state).min(self.first + Self::capacity(size.1)) {
            let r = Self::row(size.0, index - self.first);
            if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
                self.focus = index;
                return self.choose(state, lang);
            }
        }
        None
    }
    pub fn key(
        &mut self,
        state: &AccountState,
        lang: Lang,
        height: f32,
        key: &Key,
        mods: ModifiersState,
    ) -> Option<AccountCommand> {
        let count = Self::count(state);
        match key {
            Key::Named(NamedKey::Escape) => self.open = false,
            Key::Named(NamedKey::Enter) => return self.choose(state, lang),
            Key::Character(value) if value == " " => return self.choose(state, lang),
            Key::Named(NamedKey::ArrowUp) => self.focus = (self.focus + count - 1) % count,
            Key::Named(NamedKey::ArrowDown) => self.focus = (self.focus + 1) % count,
            Key::Named(NamedKey::Tab) => {
                self.focus = (self.focus + if mods.shift_key() { count - 1 } else { 1 }) % count
            }
            _ => {}
        }
        self.reveal(height);
        None
    }
    pub fn scroll(&mut self, state: &AccountState, height: f32, delta: f32) {
        let max = Self::count(state).saturating_sub(Self::capacity(height));
        self.first = if delta > 0.0 {
            self.first.saturating_sub(1)
        } else {
            (self.first + 1).min(max)
        };
    }
    pub fn draw(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        state: &AccountState,
        lang: Lang,
        size: (f32, f32),
    ) {
        self.focus = self.focus.min(Self::count(state) - 1);
        self.first = self
            .first
            .min(Self::count(state).saturating_sub(Self::capacity(size.1)));
        scene::fill(
            canvas,
            root,
            theme::p().bg,
            &scene::rect(0.0, 0.0, size.0, size.1),
        );
        let title = engine.simple_layout(
            t(lang, I18nKey::AccountSwitcherTitle),
            theme::role::subtitle(),
        );
        scene::draw_layout_first_line(canvas, root, &title, 24.0, 42.0, Some(theme::p().text));
        for index in self.first..Self::count(state).min(self.first + Self::capacity(size.1)) {
            let label = if let Some(row) = state.held_accounts.get(index) {
                let short = &row.account_id[row.account_id.len().saturating_sub(8)..];
                let label = t(lang, I18nKey::AccountSwitcherLabel).replace("{}", short);
                let status = if row.scope_id == state.active_scope {
                    Some(I18nKey::AccountSwitcherActive)
                } else if !row.active_generations.is_empty() {
                    Some(I18nKey::AccountSwitcherPending)
                } else if !row.credential_ready {
                    Some(I18nKey::AccountSwitcherRepair)
                } else {
                    None
                };
                status.map_or(label.clone(), |key| format!("{label} · {}", t(lang, key)))
            } else if index == state.held_accounts.len() {
                t(lang, I18nKey::AccountSwitcherAdd).into()
            } else {
                t(lang, I18nKey::WorkspacesCancel).into()
            };
            // Fixed row bounds apply to drawing, hit testing and keyboard scrolling.
            let rect = Self::row(size.0, index - self.first);
            scene::fill(
                canvas,
                root,
                theme::p().panel_bg,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, 6.0),
            );
            scene::stroke(
                canvas,
                root,
                if index == self.focus {
                    theme::p().text
                } else {
                    theme::p().panel_border
                },
                theme::BORDER_W,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, 6.0),
            );
            let layout = engine.simple_layout_wrapped(&label, theme::role::meta(), rect.w - 20.0);
            scene::draw_layout(
                canvas,
                root,
                &layout,
                rect.x + 10.0,
                rect.y + 5.0,
                Some(if !state.capability && index <= state.held_accounts.len() {
                    theme::p().text_dim
                } else {
                    theme::p().text
                }),
            );
        }
    }
}
