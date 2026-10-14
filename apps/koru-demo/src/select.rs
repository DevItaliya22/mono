// SPDX-License-Identifier: MPL-2.0

//! **Exactly one, or a failure that says what the others were.**
//!
//! This is the smallest module in the crate and the one the plan is most
//! explicit about: *"Multiple matches fail with candidates."* The reason is
//! worth writing down, because "take the first" is the shape every automation
//! library ships with and it looks like a convenience:
//!
//! **A runner that silently picks the first match is a runner that passes on the
//! wrong control.** Two tabs showing one document, two windows over one
//! workspace, a menu that reopened — the projection gives each of those a
//! distinct occurrence precisely so a client can tell them apart, and then a
//! `FindFirst` throws that away and returns whichever the provider happened to
//! walk to first. The green result is indistinguishable from the right one.
//!
//! So there are four outcomes and no default:
//!
//! - one match that meets the requirements — the only success;
//! - **none**;
//! - **several**, with every candidate listed;
//! - **one, but not usable** — disabled, or off-screen when the caller said it
//!   needed an on-screen one. That is deliberately not "none": *the control you
//!   named is there and is in a state you did not ask for* and *the control you
//!   named does not exist* send a person to different places.
//!
//! ## What a candidate is allowed to say
//!
//! Its locator, its control type, its state, its rectangle, its process. **Not
//! its name.** §7.4 of the roadmap is absolute about document text and titles,
//! and a name is the field a title arrives in. The constraint is structural
//! rather than remembered: [`crate::inspect::ElementFacts`] has no name field,
//! so there is nothing here to leak.

use std::fmt;

use crate::inspect::{ElementFacts, Rect, Role};

/// What the caller needs of the control, beyond it being the one it named.
///
/// Both default to *false*, which is the inspecting default: an off-screen
/// paragraph is a real control a screen reader reaches, and reporting it is the
/// point. A caller that is about to *act* on a control asks for both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Requirements {
    pub must_be_enabled: bool,
    pub must_be_on_screen: bool,
}

impl Requirements {
    /// Nothing beyond identity — for reading the tree.
    pub const INSPECT: Self = Self {
        must_be_enabled: false,
        must_be_on_screen: false,
    };

    /// What a control has to be before anything is done to it.
    pub const ACTIONABLE: Self = Self {
        must_be_enabled: true,
        must_be_on_screen: true,
    };
}

/// Why the single match cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unusable {
    Disabled,
    OffScreen,
    /// A control with an empty rectangle is not "off-screen"; it is a control
    /// the provider could not place at all, and a click derived from it would
    /// land at the origin of the primary display.
    NoBounds,
}

impl fmt::Display for Unusable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("it is disabled"),
            Self::OffScreen => f.write_str("it is off-screen"),
            Self::NoBounds => f.write_str("it has no bounding rectangle"),
        }
    }
}

/// One of several controls a locator turned out to name.
///
/// Content-free by construction — see the module note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub locator: Option<String>,
    pub role: Role,
    pub enabled: bool,
    pub offscreen: bool,
    pub bounds: Option<Rect>,
    pub process_id: Option<u32>,
}

impl From<&ElementFacts> for Candidate {
    fn from(facts: &ElementFacts) -> Self {
        Self {
            locator: facts.locator.clone(),
            role: facts.role,
            enabled: facts.enabled,
            offscreen: facts.offscreen,
            bounds: facts.bounds,
            process_id: facts.process_id,
        }
    }
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            self.locator.as_deref().unwrap_or("<no koru locator>"),
            self.role
        )?;
        if !self.enabled {
            f.write_str(" disabled")?;
        }
        if self.offscreen {
            f.write_str(" off-screen")?;
        }
        match self.bounds {
            Some(r) => write!(
                f,
                " at {},{} {}x{}",
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top
            )?,
            None => f.write_str(" with no rectangle")?,
        }
        if let Some(pid) = self.process_id {
            write!(f, " in pid {pid}")?;
        }
        Ok(())
    }
}

/// What a search came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    One(ElementFacts),
    NotFound,
    Ambiguous(Vec<Candidate>),
    Unusable {
        candidate: Candidate,
        why: Unusable,
    },
    /// Matches, but in a process the runner was not pointed at.
    ///
    /// Its own arm rather than "not found", because a second Koru answering a
    /// locator is a fact about the machine the person running this needs to be
    /// told — §7.5's *no attaching to the user's ordinary window*, caught at the
    /// moment it would have happened.
    WrongProcess {
        wanted: u32,
        candidates: Vec<Candidate>,
    },
}

impl fmt::Display for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One(found) => write!(f, "one match: {}", Candidate::from(found)),
            Self::NotFound => f.write_str("no control has that locator"),
            Self::Ambiguous(candidates) => {
                writeln!(f, "{} controls have that locator:", candidates.len())?;
                for candidate in candidates {
                    writeln!(f, "  {candidate}")?;
                }
                f.write_str(
                    "  a locator that names more than one control is a defect in the case or in \
                     the projection; the runner will not choose for you",
                )
            }
            Self::Unusable { candidate, why } => {
                write!(f, "the control is there and {why}: {candidate}")
            }
            Self::WrongProcess { wanted, candidates } => {
                writeln!(
                    f,
                    "the locator matched in another process; this run is pointed at pid {wanted}:"
                )?;
                for candidate in candidates {
                    writeln!(f, "  {candidate}")?;
                }
                f.write_str("  check --pid, or close the second Koru")
            }
        }
    }
}

impl Selection {
    /// The one control, or nothing.
    pub fn found(&self) -> Option<&ElementFacts> {
        match self {
            Self::One(facts) => Some(facts),
            _ => None,
        }
    }
}

/// Apply the rule.
///
/// `process` is the instance this run is pointed at. `None` only in tests that
/// are not about process identity; a real run always knows.
pub fn select(
    matches: Vec<ElementFacts>,
    process: Option<u32>,
    requirements: Requirements,
) -> Selection {
    if matches.is_empty() {
        return Selection::NotFound;
    }

    // Process first, and before the count. Two Koru windows in two instances
    // answering one locator is not an ambiguity to report as a case defect — it
    // is the runner about to act on somebody else's window.
    let (ours, theirs): (Vec<_>, Vec<_>) = match process {
        Some(wanted) => matches
            .into_iter()
            .partition(|facts| facts.process_id == Some(wanted)),
        None => (matches, Vec::new()),
    };
    if ours.is_empty() {
        return Selection::WrongProcess {
            wanted: process.unwrap_or_default(),
            candidates: theirs.iter().map(Candidate::from).collect(),
        };
    }
    if ours.len() > 1 {
        return Selection::Ambiguous(ours.iter().map(Candidate::from).collect());
    }

    let found = ours.into_iter().next().expect("exactly one remains");
    if requirements.must_be_enabled && !found.enabled {
        return Selection::Unusable {
            candidate: Candidate::from(&found),
            why: Unusable::Disabled,
        };
    }
    if requirements.must_be_on_screen {
        if found.offscreen {
            return Selection::Unusable {
                candidate: Candidate::from(&found),
                why: Unusable::OffScreen,
            };
        }
        if found.bounds.is_none_or(|bounds| bounds.is_empty()) {
            return Selection::Unusable {
                candidate: Candidate::from(&found),
                why: Unusable::NoBounds,
            };
        }
    }
    Selection::One(found)
}

#[cfg(test)]
mod tests;
