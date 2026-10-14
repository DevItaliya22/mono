// SPDX-License-Identifier: MPL-2.0

//! One device's history, inside the *Linked devices* page (maintainers' decision 13).
//!
//! ```text
//! ‹ Pixel                                   the back row: the list again
//! Last synced 5 Oct 2026, 14:02
//! ┌──────────────────────────────────────────────────────────────┐
//! │ Updated “Roadmap” · 3 ops                 a document: opens it │
//! │ 5 Oct 2026, 14:02                                              │
//! ├──────────────────────────────────────────────────────────────┤
//! │ Reconciled 1 profile change, 2 document changes                │
//! │ 5 Oct 2026, 13:58                                              │
//! └──────────────────────────────────────────────────────────────┘
//! Load older                                when Agent has an older page
//! ```
//!
//! The events are the model's (`LinkedDevices::history_device`), newest
//! first, as Agent pages them; *Load older* asks for the next page through
//! `take_older_history`, which hands the cursor out once. The whole history
//! scrolls with the page rather than in a window of seven rows.

use super::super::profile_page::{Control, Look, Placed};
use super::*;
use koru_agent_wire::DeviceStateEventView;

const LAST_NONE: &str = "No state changes received yet";
const LOADING: &str = "Loading device history\u{2026}";
const EMPTY: &str = "No catalog, profile, workspace, document, or blob changes have been received from this device.";
const OLDER: &str = "Load older";
/// One event's row: its label and when.
const EVENT_H: f32 = 48.0;

/// One event, where its two lines go.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EventRow {
    pub rect: Rect,
    pub label: String,
    pub label_at: Rect,
    pub when: String,
    pub when_at: Rect,
    /// A document's update: pressing the row opens it.
    pub opens: bool,
}

/// The history sub-page, unscrolled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryLaid {
    pub back: Rect,
    pub back_text: String,
    pub last: Rect,
    pub last_text: String,
    pub card: Option<Rect>,
    pub events: Vec<EventRow>,
    /// *Loading…*, or that nothing has come from the device.
    pub message: Option<(Rect, String)>,
}

impl ManageView {
    pub(in crate::manage) fn lay_history(
        &mut self,
        column: Rect,
        controls: &mut Vec<Placed>,
    ) -> (HistoryLaid, f32) {
        let Some(device) = self.devices.history_device().cloned() else {
            let none = Rect::new(column.x, column.y, 0.0, 0.0);
            let laid = HistoryLaid {
                back: none,
                back_text: String::new(),
                last: none,
                last_text: String::new(),
                card: None,
                events: Vec::new(),
                message: None,
            };
            return (laid, column.y);
        };
        let p = theme::p();
        // The back row is the page's heading: "‹ " and the device's name.
        let back_text = format!("\u{2039} {}", device.display_name);
        let back_h = KoruTextTitle::new(&back_text).measure(&mut self.engine, column.w);
        let back_w = self
            .engine
            .simple_layout(&back_text, theme::role::title())
            .width()
            .clamp(1.0, column.w);
        let back = Rect::new(column.x, column.y, back_w, back_h);
        controls.push(Placed {
            control: Control::HistoryBack,
            rect: back,
            label: format!("Back to linked devices from {}", device.display_name),
            look: Look::Header,
        });
        let mut y = back.y + back.h + 2.0;
        let last_text = device.last_sync_millis.map_or_else(
            || LAST_NONE.to_owned(),
            |at| format!("Last synced {}", date_time_label(at)),
        );
        let line = theme::role::meta().metrics.line_height;
        let last = Rect::new(column.x, y, column.w, line);
        y += line + Card::GAP;

        let inner_x = column.x + CardRow::PAD_X;
        let inner_w = (column.w - CardRow::PAD_X * 2.0).max(1.0);
        let row_line = theme::role::row().metrics.line_height;
        let card_top = y;
        let mut events = Vec::new();
        for (index, event) in device.history.iter().enumerate() {
            let rect = Rect::new(column.x, y, column.w, EVENT_H);
            let block = row_line + line;
            let top = y + (EVENT_H - block) * 0.5;
            let label = device_event_label(event);
            let opens = event_document(event).is_some();
            let label_text = self.engine.elided(&label, theme::role::row(), inner_w);
            if opens {
                controls.push(Placed {
                    control: Control::HistoryEvent(index),
                    rect,
                    label: label.clone(),
                    look: Look::Row,
                });
            }
            events.push(EventRow {
                rect,
                label: label_text,
                label_at: Rect::new(inner_x, top, inner_w, row_line),
                when: date_time_label(event.at_millis),
                when_at: Rect::new(inner_x, top + row_line, inner_w, line),
                opens,
            });
            y += EVENT_H;
        }
        let card =
            (!events.is_empty()).then(|| Rect::new(column.x, card_top, column.w, y - card_top));
        let mut message = None;
        let said = if self.devices.history_loading() {
            Some(LOADING)
        } else if events.is_empty() {
            Some(EMPTY)
        } else {
            None
        };
        if let Some(text) = said {
            if card.is_some() {
                y += 10.0;
            }
            let h = KoruTextParagraph::new(text, theme::role::meta(), "", p.text_dim)
                .measure(&mut self.engine, column.w);
            message = Some((Rect::new(column.x, y, column.w, h), text.to_owned()));
            y += h;
        } else if self.devices.has_older_history() {
            y += 10.0;
            let w = ActionButton::new(OLDER, ActionStyle::Ghost, None).width(&mut self.engine);
            controls.push(Placed {
                control: Control::OlderHistory,
                rect: Rect::new(column.x, y, w, ActionButton::H),
                label: OLDER.into(),
                look: Look::Action(ActionStyle::Ghost),
            });
            y += ActionButton::H;
        }
        let laid = HistoryLaid {
            back,
            back_text,
            last,
            last_text,
            card,
            events,
            message,
        };
        (laid, y)
    }

    /// The document the `index`th event of the open history is about.
    pub(in crate::manage) fn history_document(&self, index: usize) -> Option<DocumentId> {
        self.devices
            .history_device()?
            .history
            .get(index)
            .and_then(event_document)
    }

    pub(in crate::manage) fn paint_history(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        history: &HistoryLaid,
    ) {
        let p = theme::p();
        let top = history.back;
        let bottom = history
            .message
            .as_ref()
            .map(|(rect, _)| rect.y + rect.h)
            .or(history.card.map(|card| card.y + card.h))
            .unwrap_or(history.last.y + history.last.h);
        let _history = inspect::scope(
            "Manage device history",
            "part",
            root,
            scene::rect(top.x, top.y, self.column().w, bottom - top.y),
            |r| {
                r.property(
                    "events",
                    history.events.len(),
                    "LinkedDevices::history_device",
                );
                r.property(
                    "older page",
                    self.devices.has_older_history(),
                    "take_older_history",
                );
                r.property("decision", "decision 13", "initiative");
            },
        );
        // The back row, lit like the Profile header's name.
        let lit = self.page_hover == Some(Control::HistoryBack)
            || self.page_focus == Some(Control::HistoryBack);
        if lit {
            let b = history.back;
            let shape = scene::rrect(b.x - 6.0, b.y, b.w + 12.0, b.h, 6.0);
            scene::fill(canvas, root, p.hover_tint, &shape);
            if self.page_focus == Some(Control::HistoryBack) {
                scene::stroke(canvas, root, p.focus_ring, theme::BORDER_W * 2.0, &shape);
            }
        }
        let width = self.column().w;
        KoruTextTitle::new(&history.back_text).draw(
            canvas,
            root,
            &mut self.engine,
            Rect::new(history.back.x, history.back.y, width, 0.0),
        );
        koru_ui::text_paragraph::draw_line(
            canvas,
            root,
            &mut self.engine,
            &history.last_text,
            history.last.x,
            history.last.y,
            theme::role::meta(),
            "theme::role::meta()",
            p.text_dim,
        );
        if let Some(card) = history.card {
            let dividers: Vec<f32> = history.events.iter().skip(1).map(|e| e.rect.y).collect();
            let _card = Card::new(None).draw(canvas, root, &mut self.engine, card, &dividers, 1.0);
            for (index, event) in history.events.iter().enumerate() {
                let control = Control::HistoryEvent(index);
                let r = event.rect;
                let inner = scene::rrect(r.x + 4.0, r.y + 3.0, r.w - 8.0, r.h - 6.0, 7.0);
                if event.opens && self.page_hover == Some(control) {
                    scene::fill(canvas, root, p.hover_tint, &inner);
                }
                if self.page_focus == Some(control) {
                    scene::stroke(canvas, root, p.focus_ring, theme::BORDER_W * 2.0, &inner);
                }
                koru_ui::text_paragraph::draw_line(
                    canvas,
                    root,
                    &mut self.engine,
                    &event.label,
                    event.label_at.x,
                    event.label_at.y,
                    theme::role::row(),
                    "theme::role::row()",
                    if event.opens { p.accent } else { p.text },
                );
                koru_ui::text_paragraph::draw_line(
                    canvas,
                    root,
                    &mut self.engine,
                    &event.when,
                    event.when_at.x,
                    event.when_at.y,
                    theme::role::meta(),
                    "theme::role::meta()",
                    p.text_dim,
                );
            }
        }
        if let Some((rect, text)) = &history.message {
            KoruTextParagraph::new(text, theme::role::meta(), "theme::role::meta()", p.text_dim)
                .draw(canvas, root, &mut self.engine, *rect);
        }
        if let Some(placed) = self.laid.placed(Control::OlderHistory).cloned() {
            self.paint_control(canvas, root, &placed);
        }
    }
}

/// A document's update names the document it opens.
fn event_document(event: &DeviceStateEventView) -> Option<DocumentId> {
    (event.kind == "document_updated")
        .then(|| event.document_id.as_deref().and_then(DocumentId::parse))
        .flatten()
}

/// One event, in the words the Link dialog's history used.
pub(crate) fn device_event_label(event: &DeviceStateEventView) -> String {
    if event.kind == "document_updated" {
        let name = event
            .document_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Untitled");
        let operations = event
            .items
            .map(|count| format!(" \u{b7} {count} op{}", if count == 1 { "" } else { "s" }));
        return format!(
            "Updated \u{201c}{name}\u{201d}{}",
            operations.as_deref().unwrap_or("")
        );
    }
    if event.kind == "catalog_reconciled"
        && let Some(detail) = event.detail.as_deref()
    {
        return format!("Reconciled {detail}");
    }
    let label = match event.kind.as_str() {
        "catalog_reconciled" => "Catalog reconciled",
        "profile_created" => "Profile added",
        "profile_updated" => "Profile updated",
        "workspace_created" => "Workspace added",
        "workspace_updated" => "Workspace updated",
        "document_created" => "Document added",
        "document_updated" => "Document updated",
        "blob_stored" => "File downloaded",
        _ => event.kind.as_str(),
    };
    let target = event
        .document_id
        .as_deref()
        .or(event.workspace_id.as_deref())
        .or(event.profile_id.as_deref())
        .or(event.blob_hash.as_deref())
        .map(|value| value.chars().take(12).collect::<String>());
    target.map_or_else(
        || label.to_owned(),
        |target| format!("{label} \u{b7} {target}"),
    )
}
