// SPDX-License-Identifier: MPL-2.0

//! What the page says when it has no rows.
//!
//! **Three states, and the whole of the work is telling them apart.** The
//! design (`docs/features/tasks/design/1d-states-and-lists.png`) draws each of them as
//! one row on the chip shape — no illustration, nothing centred in dead space —
//! so what differs between them is the sentence, whether there is something to
//! do about it, and *which* of them is allowed to appear:
//!
//! - **An empty bucket** is the fact plus the thing to do about it: *Nothing due
//!   today.* with **Add a task** at the row's right end. It says nothing about
//!   the workspace, because one empty queue is not an empty workspace.
//! - **The cold state** is worded away from it on purpose: a sweep in progress
//!   answers zero rows, and *Nothing due today.* mid-sweep is a claim the store
//!   has not made. *Checking your workspaces…* is a report about the store
//!   rather than about the person's work.
//! - **A store that would not answer** says so, and outranks both: an empty
//!   list and a failed query look identical, and the one somebody can act on is
//!   the one that names itself.
//!
//! **There was a fourth, and using it retired it.** A *teaching* state explained
//! `/task` whenever no bucket held anything. Two things were wrong with it in
//! the app rather than on the page: the capture row it was teaching about sits
//! directly above it, so an empty page said the same thing twice and said the
//! shorter, truer version first; and its gate was the rail's counts, which are
//! a second answer arriving on its own schedule — so a page whose own answer had
//! rows still taught, and a page that had just gained one went on teaching until
//! the counts caught up. A sentence that is sometimes wrong about whether you
//! have any work at all is worse than no sentence.
//!
//! Everything here is **pure** — no view, no engine, no clock — which is what
//! lets the suite drive each gate against the other three.

use crate::panes::explore::rail::{Bucket, RailTarget};

/// The action a state offers, at the row's right end.
pub(super) const ADD_A_TASK: &str = "Add a task";

/// What the cold state says. **A state, not a verb with a subject** — which
/// workspace is being swept is not something a reader of this page can act on.
pub(super) const CHECKING: &str = "Checking your workspaces\u{2026}";

/// Which of the three the page is in. `None` — no variant — is *there are rows*,
/// or *we have not looked yet*; a zero that means "not asked" is a lie a reader
/// cannot see through, so nothing is drawn for it at all.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Empty {
    /// The store would not answer, in its own words.
    Notice(String),
    /// The answer is a page of a workspace still being swept.
    Cold,
    /// This queue or this list is empty.
    ///
    /// `action` is whether **Add a task** is offered: every queue but Completed,
    /// which is an archive — an empty archive is not a gap to fill, and a button
    /// there would be offering to add something that would not appear.
    ///
    /// It no longer says *and others are not*: that clause was the teaching
    /// state's, and the second answer it needed to be sure of went with it.
    Bucket { line: String, action: bool },
}

impl Empty {
    /// The sentence, as one string.
    pub(super) fn line(&self) -> String {
        match self {
            Self::Notice(text) => text.clone(),
            Self::Cold => CHECKING.to_owned(),
            Self::Bucket { line, .. } => line.clone(),
        }
    }

    /// The action at the row's right end, or `None` for a state with nothing to
    /// do about it.
    ///
    /// **Only an empty bucket earns one.** A sweep is not something to act on,
    /// and a store that will not answer is not fixed by typing a task into it.
    pub(super) fn action(&self) -> Option<&'static str> {
        match self {
            Self::Bucket { action: true, .. } => Some(ADD_A_TASK),
            _ => None,
        }
    }

    /// Whether the row wears the quiet dot the rail's ambient indicators use.
    /// The cold state alone, where it is the only thing saying *still working*.
    pub(super) fn dotted(&self) -> bool {
        matches!(self, Self::Cold)
    }
}

/// Which state an empty page is in — **the one gate**, taken by drawing, by the
/// hit test and by the suite.
///
/// The order is the whole rule and each step of it is a decision:
///
/// 1. a **notice** outranks everything, because it is the only one that is
///    about the store rather than about the work;
/// 2. `known == false` draws **nothing** — before the first answer an empty
///    list means *we have not looked*, and half a second of blank is honest
///    where any sentence would not be;
/// 3. **cold** next, so a mid-sweep zero never reads as *you have no tasks*;
/// 4. otherwise this page is empty, which is the ordinary case and the one that
///    offers the action.
///
/// **Every input is about the page being drawn.** The teaching state needed the
/// rail's counts, and that was the one argument here that was not — which is
/// what let this function be right about the page and wrong on the screen.
pub(super) fn empty_state(
    target: RailTarget,
    known: bool,
    updating: bool,
    notice: Option<&str>,
) -> Option<Empty> {
    if let Some(notice) = notice {
        return Some(Empty::Notice(notice.to_owned()));
    }
    if !known {
        return None;
    }
    if updating {
        return Some(Empty::Cold);
    }
    Some(Empty::Bucket {
        line: bucket_line(target),
        action: target != RailTarget::Bucket(Bucket::Completed),
    })
}

/// What an empty queue or list says about itself.
///
/// **The fact, in the queue's own terms.** Only Today's sentence is drawn in the
/// design; the rest follow its shape — what this queue is *for*, said as an
/// absence — rather than repeating one neutral line five times, which would make
/// the four queues indistinguishable at the moment they most need telling apart.
/// A list says it is a list, because SPEC §8 makes a list a filter over all time
/// and *due* is not what is missing from it.
fn bucket_line(target: RailTarget) -> String {
    match target {
        RailTarget::Reminders => unreachable!(
            "the rail's Reminders row swaps the tab's view; a Tasks page never targets it"
        ),
        RailTarget::Bucket(Bucket::Today) => "Nothing due today.",
        RailTarget::Bucket(Bucket::Next) => "Nothing due this week.",
        RailTarget::Bucket(Bucket::Upcoming) => "Nothing due this month.",
        RailTarget::Bucket(Bucket::Later) => "Nothing waiting.",
        // An archive, and the one queue whose emptiness is not a gap to fill —
        // which is why it is also the one that offers no action.
        RailTarget::Bucket(Bucket::Completed) => "Nothing completed yet.",
        RailTarget::List(_) => "Nothing in this list yet.",
    }
    .to_owned()
}
