// SPDX-License-Identifier: MPL-2.0

//! The Ctrl+S prompt: name a snapshot of the document as it stands.
//!
//! Two modes, one surface. It opens as **Save** immediately — the keystroke
//! must never wait on a probe — while the history worker checks whether
//! anything actually changed since the newest version. If nothing did, the
//! prompt **changes to Rename** for that newest entry (owner decision,
//! 2026-08-31): saving an identical snapshot again would only stack a
//! duplicate row, and what the person plausibly wants is to name the one
//! that exists. The morph never touches a name already being typed.
//!
//! It reuses the editor's [`NamePrompt`] for the same reason the taskbar's
//! *New document* prompt does: text selection, undo, paste and platform key
//! behaviour must not change with the prompt's subject.

use koru_core::NodeId;
use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_editor::components::Outcome;
use koru_editor::components::draw::draw_name_prompt_contents;
use koru_editor::components::name_prompt::{NamePrompt, field_rect};
use koru_editor::components::text_field::TextField;
use koru_editor::trigger::{Anchor, Trigger, TriggerKind};
use koru_i18n::Key as I18nKey;
use koru_ui::{KoruDialogFrame, KoruDialogFrameInspection, KoruDialogFrameState};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

/// What the prompt is naming.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Mode {
    /// Capture the document as it stands, under this name.
    Save,
    /// Nothing changed since the newest version: rename that entry instead.
    Rename { entry: String },
}

pub(super) enum Action {
    Keep,
    Cancel,
    /// Save a new snapshot. NamePrompt currently refuses an empty name before
    /// reaching this outcome; the outcome mapping itself adds no validation.
    Save(String),
    /// Rename an existing entry. NamePrompt currently refuses an empty name
    /// before reaching this outcome, so the prompt cannot clear a label.
    Rename {
        entry: String,
        label: String,
    },
}

pub(super) struct SnapshotPrompt {
    prompt: NamePrompt,
    dragging: bool,
    mode: Mode,
    /// Whether a keystroke has reached the name box. A probe answer arriving
    /// after typing began must not clobber what the person wrote.
    touched: bool,
}

impl SnapshotPrompt {
    /// The name box, for a dictation the container is routing.
    pub(super) fn dictation_field(&mut self) -> &mut koru_ui::TextField {
        &mut self.prompt.input
    }

    pub(super) fn new(engine: &mut TextEngine) -> Self {
        Self {
            prompt: NamePrompt {
                // Not anchored to document text; NamePrompt hands a Trigger
                // back as part of its reusable contract. This detached insert
                // trigger is never committed into a row; the container
                // consumes only the name.
                trigger: Trigger {
                    kind: TriggerKind::Slash,
                    start: 0,
                    node: NodeId::new(),
                    anchor: Anchor::Insert,
                    stable: None,
                },
                input: TextField::new(engine, theme::role::body()),
                page_icon: None,
            },
            dragging: false,
            mode: Mode::Save,
            touched: false,
        }
    }

    /// The probe's answer: nothing changed since `entry`, so the prompt is a
    /// rename of it now. Prefills the entry's current name unless the person
    /// already started typing one.
    pub(super) fn become_rename(
        &mut self,
        engine: &mut TextEngine,
        entry: String,
        label: Option<String>,
    ) {
        self.mode = Mode::Rename { entry };
        if !self.touched
            && let Some(label) = label
        {
            self.prompt.input.set_text(engine, &label);
        }
    }

    pub(super) fn panel(viewport: (f32, f32)) -> Rect {
        KoruDialogFrame::name_prompt(viewport).panel()
    }

    fn field(&self, viewport: (f32, f32)) -> Rect {
        let panel = Self::panel(viewport);
        let field = field_rect(panel.w);
        Rect::new(panel.x + field.x, panel.y + field.y, field.w, field.h)
    }

    pub(super) fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Action {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            return Action::Cancel;
        }
        self.touched = true;
        match self.prompt.key(engine, key, text, mods) {
            Outcome::Commit(spec) => match &self.mode {
                Mode::Save => Action::Save(spec.label),
                Mode::Rename { entry } => Action::Rename {
                    entry: entry.clone(),
                    label: spec.label,
                },
            },
            _ => Action::Keep,
        }
    }

    pub(super) fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) {
        let panel = Self::panel(viewport);
        self.dragging = self.prompt.click(engine, x - panel.x, y - panel.y, panel.w);
    }

    pub(super) fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
    ) -> bool {
        if !self.dragging {
            return false;
        }
        let panel = Self::panel(viewport);
        self.prompt.drag(engine, x - panel.x, panel.w);
        true
    }

    pub(super) fn mouse_up(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    pub(super) fn double_click(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) {
        let panel = Self::panel(viewport);
        self.prompt
            .double_click(engine, x - panel.x, y - panel.y, panel.w);
    }

    pub(super) fn cursor(&self, viewport: (f32, f32), x: f32, y: f32) -> CursorIcon {
        if self.field(viewport).contains(x, y) {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        }
    }

    pub(super) fn draw(
        &mut self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        KoruDialogFrame::name_prompt(viewport).draw(
            scene,
            root,
            KoruDialogFrameInspection {
                state: KoruDialogFrameState::Open,
                ..Default::default()
            },
            |scene, root, panel| {
                koru_draw::inspect::current(|r| {
                    r.property(
                        "mode",
                        match self.mode {
                            Mode::Save => "Save",
                            Mode::Rename { .. } => "Rename",
                        },
                        "SnapshotPrompt::mode; private entry IDs excluded",
                    );
                    r.property(
                        "native source part",
                        "s:shell#snapshot",
                        "static host blocker part; not a per-opening occurrence or full locator",
                    );
                    r.property(
                        "field width",
                        field_rect(panel.w).w,
                        "existing NamePrompt::field_rect; may be negative in tiny viewport",
                    );
                });
                let _content = koru_draw::inspect::scope(
                    "Snapshot name content",
                    "unregistered",
                    root,
                    scene::rect(panel.x, panel.y, panel.w, panel.h),
                    |_| {},
                );
                let lang = crate::format::lang();
                let p = theme::p();
                let (title, hint) = match self.mode {
                    Mode::Save => (I18nKey::SnapshotSaveTitle, I18nKey::SnapshotSaveHint),
                    Mode::Rename { .. } => {
                        (I18nKey::SnapshotRenameTitle, I18nKey::SnapshotRenameHint)
                    }
                };
                draw_name_prompt_contents(
                    scene,
                    root,
                    &mut self.prompt,
                    engine,
                    panel.x,
                    panel.y,
                    panel.w,
                    koru_i18n::t(lang, title),
                    koru_i18n::t(lang, I18nKey::SnapshotNamePlaceholder),
                    // No page-icon button: a snapshot has no icon to set.
                    false,
                );

                let hint = engine.simple_layout(koru_i18n::t(lang, hint), theme::role::hint());
                scene::draw_layout_first_line(
                    scene,
                    root,
                    &hint,
                    panel.x + theme::PANEL_PAD + 4.0,
                    panel.y + 62.0,
                    Some(p.text_dim),
                );
            },
        );
    }
}

#[cfg(test)]
mod tests;
