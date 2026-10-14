// SPDX-License-Identifier: MPL-2.0

//! The five existing task queues, projected over the rail's own row geometry.
//! List creation/editing remain their existing separate surfaces. No Store
//! query or task operation belongs in this projection or its callbacks.

use accesskit::{Action, Role};
use koru_i18n::{Key, t};
use koru_workspace_host::accessibility::{
    ControlPart, ObjectKey, Occurrence, Revision, SemanticNode,
};
use winit::keyboard::{Key as WKey, NamedKey};

use super::{Bucket, FOOTER_PAD, RailRow, RailTarget, TasksRail, TasksRailRequest};
use crate::panes::{Outcome, explore::accessibility::Context};
use koru_draw::geometry::Rect;

const QUEUE: &str = "tasks-queue";
const ALL: [Bucket; 5] = [
    Bucket::Today,
    Bucket::Next,
    Bucket::Upcoming,
    Bucket::Later,
    Bucket::Completed,
];

fn token(bucket: Bucket) -> &'static str {
    match bucket {
        Bucket::Today => "today",
        Bucket::Next => "next",
        Bucket::Upcoming => "upcoming",
        Bucket::Later => "later",
        Bucket::Completed => "completed",
    }
}

fn label(bucket: Bucket) -> Key {
    match bucket {
        Bucket::Today => Key::A11yTaskBucketToday,
        Bucket::Next => Key::A11yTaskBucketNext,
        Bucket::Upcoming => Key::A11yTaskBucketUpcoming,
        Bucket::Later => Key::A11yTaskBucketLater,
        Bucket::Completed => Key::A11yTaskBucketCompleted,
    }
}

fn occurrence(cx: &Context, bucket: Bucket) -> Occurrence {
    Occurrence::surface(cx.window, cx.view, QUEUE, ControlPart::new(token(bucket)))
}

impl TasksRail {
    pub(crate) fn queue_semantics(&self, cx: &Context, top: f32) -> SemanticNode {
        let children = self
            .laid_out(cx.area, top)
            .into_iter()
            .filter_map(|(row, bounds)| {
                let RailRow::Bucket(bucket) = row else {
                    return None;
                };
                Some(
                    SemanticNode::new(occurrence(cx, bucket), Role::Tab)
                        .with_label(t(cx.lang, label(bucket)))
                        .with_bounds(bounds)
                        .with_selected(self.selected == Some(RailTarget::Bucket(bucket)))
                        .with_position(ALL.iter().position(|b| *b == bucket).unwrap())
                        .with_actions([Action::Focus, Action::Click, Action::ScrollIntoView]),
                )
            })
            .collect();
        SemanticNode::new(
            Occurrence::surface(cx.window, cx.view, QUEUE, ControlPart::ROOT),
            Role::TabList,
        )
        .with_label(t(cx.lang, Key::A11yTasksHeading))
        .with_bounds(Rect::new(
            cx.area.x,
            top,
            cx.area.w,
            (cx.area.y + cx.area.h - FOOTER_PAD - top).max(0.0),
        ))
        .with_size_of_set(ALL.len())
        .with_children(children)
    }

    pub(crate) fn queue_focus_occurrence(&self, cx: &Context) -> Option<Occurrence> {
        self.queue_focus.map(|bucket| occurrence(cx, bucket))
    }

    pub(crate) fn revise_queues(&self, rev: &mut Revision) {
        rev.str(self.queue_focus.map(token).unwrap_or("none"));
        rev.str(match self.selected {
            Some(RailTarget::Bucket(bucket)) => token(bucket),
            _ => "none",
        });
        rev.u64(self.lists.len() as u64);
        rev.u64(u64::from(self.scroll.to_bits()));
    }

    pub(crate) fn has_queue_focus(&self) -> bool {
        self.queue_focus.is_some()
    }

    pub(crate) fn clear_queue_focus(&mut self) -> bool {
        self.queue_focus.take().is_some()
    }

    pub(crate) fn focus_queues(&mut self, area: Rect, top: f32) -> bool {
        if area.w <= 0.0 {
            return false;
        }
        let bucket = match self.selected {
            Some(RailTarget::Bucket(bucket)) => bucket,
            _ => Bucket::Today,
        };
        self.queue_focus = Some(bucket);
        self.reveal_queue(area, top, bucket);
        true
    }

    fn reveal_queue(&mut self, area: Rect, top: f32, bucket: Bucket) {
        let Some((_, bounds)) = self
            .laid_out(area, top)
            .into_iter()
            .find(|(row, _)| *row == RailRow::Bucket(bucket))
        else {
            return;
        };
        let bottom = area.y + area.h - FOOTER_PAD;
        if bounds.y < top {
            self.scroll += bounds.y - top;
        } else if bounds.y + bounds.h > bottom {
            self.scroll += bounds.y + bounds.h - bottom;
        }
        self.clamp_scroll(area, top);
    }

    /// The same intent pointer, keyboard and AT activation produce.
    pub(crate) fn open_bucket(&mut self, bucket: Bucket, new_tab: bool) -> TasksRailRequest {
        if !new_tab {
            self.selected = Some(RailTarget::Bucket(bucket));
        }
        self.queue_focus = None;
        TasksRailRequest::OpenBucket { bucket, new_tab }
    }

    pub(crate) fn queue_action(
        &mut self,
        area: Rect,
        top: f32,
        target: Occurrence,
        action: Action,
    ) -> Option<Outcome> {
        if target.object != ObjectKey::Surface(QUEUE) {
            return None;
        }
        let bucket = ALL
            .into_iter()
            .find(|bucket| token(*bucket) == target.part.name())?;
        if area.w <= 0.0 {
            return None;
        }
        match action {
            Action::Focus => {
                self.queue_focus = Some(bucket);
                self.reveal_queue(area, top, bucket);
                Some(Outcome::Consumed)
            }
            Action::ScrollIntoView => {
                self.reveal_queue(area, top, bucket);
                Some(Outcome::Consumed)
            }
            Action::Click => Some(Outcome::Tasks(self.open_bucket(bucket, false))),
            _ => None,
        }
    }

    pub(crate) fn queue_key(&mut self, area: Rect, top: f32, key: &WKey) -> Option<Outcome> {
        let bucket = self.queue_focus?;
        let index = ALL.iter().position(|b| *b == bucket)?;
        let next = match key {
            WKey::Named(NamedKey::ArrowDown) => (index + 1).min(ALL.len() - 1),
            WKey::Named(NamedKey::ArrowUp) => index.saturating_sub(1),
            WKey::Named(NamedKey::Home) => 0,
            WKey::Named(NamedKey::End) => ALL.len() - 1,
            WKey::Named(NamedKey::Enter) | WKey::Named(NamedKey::Space) => {
                return Some(Outcome::Tasks(self.open_bucket(bucket, false)));
            }
            WKey::Named(NamedKey::Escape) | WKey::Named(NamedKey::Tab) => {
                self.queue_focus = None;
                return Some(Outcome::Consumed);
            }
            _ => return None,
        };
        self.queue_focus = Some(ALL[next]);
        self.reveal_queue(area, top, ALL[next]);
        Some(Outcome::Consumed)
    }
}

#[cfg(test)]
mod tests;
