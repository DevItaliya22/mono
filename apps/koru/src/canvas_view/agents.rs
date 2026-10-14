// SPDX-License-Identifier: MPL-2.0

//! **The agent menu** an agent node's header opens.
//!
//! The shared [`Menu`] — the widget every context menu in Koru is — listing
//! the agents ready on this machine by name, the node's own agent ticked.
//! Choosing one is one `koru_core::canvas::set_agent`. With nothing ready it
//! is one dimmed row that says so, and the container is asked to find out
//! which agents are ready, so the list fills while the menu is up.
//!
//! Its rows are fixed when it opens, as every menu's are; the view reopens it
//! over the same node when the list it was built from changes.

use std::borrow::Cow;

use koru_core::NodeId;
use koru_draw::text::TextEngine;
use koru_i18n::{Key, Lang, t};
use koru_ui::{Menu, MenuItem};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key as WinitKey, NamedKey};

/// One row: an agent by its stored key and its name, or the one row that
/// says none is ready.
#[derive(Clone, Debug)]
pub struct AgentRow {
    /// `None` on the *no agent is ready* row.
    pub stored: Option<String>,
    label: String,
    current: bool,
}

impl MenuItem for AgentRow {
    fn label(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }

    fn checked(&self) -> Option<bool> {
        self.stored.is_some().then_some(self.current)
    }
}

/// What a key or a press in the menu came to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Keep,
    Close,
    /// This agent, by its stored key, for the menu's node.
    Choose(String),
}

pub struct AgentMenu {
    /// The agent node the menu is choosing for.
    pub node: NodeId,
    anchor: (f32, f32),
    menu: Menu<AgentRow>,
    enabled: Vec<bool>,
    /// The list it was built from, so a change to it can rebuild the rows.
    built_from: Vec<(String, String)>,
}

impl AgentMenu {
    /// Open over `node` at `anchor`, listing `agents` with `current` ticked.
    pub fn open(
        engine: &mut TextEngine,
        lang: Lang,
        anchor: (f32, f32),
        node: NodeId,
        agents: &[(String, String)],
        current: Option<&str>,
    ) -> Self {
        let rows: Vec<AgentRow> = if agents.is_empty() {
            vec![AgentRow {
                stored: None,
                label: t(lang, Key::CanvasNoAgentReady).to_owned(),
                current: false,
            }]
        } else {
            agents
                .iter()
                .map(|(stored, name)| AgentRow {
                    stored: Some(stored.clone()),
                    label: name.clone(),
                    current: current.map(str::trim) == Some(stored.as_str()),
                })
                .collect()
        };
        let enabled = rows.iter().map(|row| row.stored.is_some()).collect();
        Self {
            node,
            anchor,
            menu: Menu::new(engine, anchor, rows),
            enabled,
            built_from: agents.to_vec(),
        }
    }

    /// Whether it lists nothing but the *no agent is ready* row.
    pub fn is_empty(&self) -> bool {
        self.built_from.is_empty()
    }

    pub fn built_from(&self) -> &[(String, String)] {
        &self.built_from
    }

    pub fn anchor(&self) -> (f32, f32) {
        self.anchor
    }

    /// The rows as a screen reader hears them: label and whether it can be
    /// chosen.
    pub fn rows(&self) -> impl Iterator<Item = (usize, &str, bool)> {
        self.menu
            .items()
            .iter()
            .enumerate()
            .map(|(i, row)| (i, row.label.as_str(), self.enabled[i]))
    }

    pub fn panel(&self, viewport: (f32, f32)) -> koru_draw::geometry::Rect {
        self.menu.panel(viewport)
    }

    pub fn item_rect(&self, viewport: (f32, f32), i: usize) -> koru_draw::geometry::Rect {
        self.menu.item_rect(viewport, i)
    }

    pub fn hover(&self) -> Option<usize> {
        self.menu.hover
    }

    /// Choose row `i`, the way a press or Enter on it does.
    pub fn choose(&self, i: usize) -> Outcome {
        match self.menu.items().get(i) {
            Some(AgentRow {
                stored: Some(stored),
                ..
            }) => Outcome::Choose(stored.clone()),
            // The dimmed row does nothing, and the menu stays.
            Some(_) => Outcome::Keep,
            None => Outcome::Close,
        }
    }

    /// A key, while the menu holds the keyboard.
    pub fn key(&mut self, key: &WinitKey) -> Outcome {
        match key {
            WinitKey::Named(NamedKey::Escape) => Outcome::Close,
            WinitKey::Named(NamedKey::ArrowDown) => {
                self.menu.step(&self.enabled, 1);
                Outcome::Keep
            }
            WinitKey::Named(NamedKey::ArrowUp) => {
                self.menu.step(&self.enabled, -1);
                Outcome::Keep
            }
            WinitKey::Named(NamedKey::Enter) | WinitKey::Named(NamedKey::Space) => {
                match self.menu.hover {
                    Some(i) => self.choose(i),
                    None => Outcome::Keep,
                }
            }
            // A menu holds the keyboard while it is open.
            _ => Outcome::Keep,
        }
    }

    /// A press: a row chooses it, anywhere else closes the menu and does
    /// nothing more — the press was aimed at the menu, not what is under it.
    pub fn mouse_down(&self, viewport: (f32, f32), x: f32, y: f32) -> Outcome {
        match self.menu.item_at(viewport, x, y) {
            Some(i) => self.choose(i),
            None => Outcome::Close,
        }
    }

    /// Follow the pointer. `true` when the lit row changed.
    pub fn mouse_move(&mut self, viewport: (f32, f32), x: f32, y: f32) -> bool {
        let next = self.menu.item_at(viewport, x, y);
        let changed = next != self.menu.hover;
        self.menu.hover = next;
        changed
    }

    pub fn over_row(&self, viewport: (f32, f32), x: f32, y: f32) -> bool {
        self.menu
            .item_at(viewport, x, y)
            .is_some_and(|i| self.enabled[i])
    }

    pub fn draw(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        self.menu.draw(scene, root, engine, viewport, &self.enabled);
    }
}
