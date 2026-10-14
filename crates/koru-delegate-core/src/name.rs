// SPDX-License-Identifier: MPL-2.0

//! **A run's short name** — two words a person can say out loud, derived
//! from the run's [`RunId`] and stored nowhere.
//!
//! A run's only name until now was its objective, which is a sentence: the
//! navigator drew *"Need to research most of the…"* and the tab drew
//! *Overview*. Neither is something you can point at across a desk. A
//! [`RunName`] is — *sunny-otter*, *copper-wren* — and because it is derived,
//! every device and every surface says the same one for the same run with
//! nothing to sync, nothing to migrate and nothing to keep in step.
//!
//! ## It is a label, never an identifier
//!
//! PRINCIPLES: *a stable identifier never derives from a name.* This derives
//! the other way round — the identifier is minted first and the name falls
//! out of it — which is the safe direction and the reason this is allowed to
//! exist at all. Nothing resolves a run by its name, nothing stores it,
//! nothing sends it, no route accepts it, and two runs that happen to share
//! one are two different runs with the same label. [`RunId`] remains the
//! identity everywhere.
//!
//! ## The mechanism, and what it costs
//!
//! The sixteen bytes of the id are folded with FNV-1a (written out here
//! because `std`'s hasher is explicitly not stable between releases, and a
//! name that changed when the toolchain moved would be worse than no name),
//! and the two words are indexed out of the folded value. Every byte of the
//! id is folded rather than two bytes taken directly, so a run whose id was
//! built from a small integer — every fixture in the suite — still gets a
//! name that varies.
//!
//! **Collisions.** 256 first words and 324 second words are 82,944 names. Two
//! runs in one workspace sharing a name reaches 1% at about 40 runs, 10% at
//! about 130, and even chances at about 340. A workspace with a few dozen
//! runs will see it as a curiosity; nothing depends on it not happening,
//! because nothing resolves a run by its name.

pub(crate) mod words;

use std::fmt;

use crate::RunId;

use words::{FIRST, SECOND};

/// One run's two-word name, as `first-second`.
///
/// Two `&'static str`s rather than a `String`: a navigator row and a tab
/// label ask for this every layout pass, and a name that allocated would be
/// an allocation per run per frame.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunName {
    first: &'static str,
    second: &'static str,
}

impl RunName {
    /// The name of a run. Total, deterministic, and the same on every
    /// device.
    pub fn of(run: RunId) -> Self {
        let folded = fold(run.as_uuid().as_bytes());
        let first = FIRST.len() as u64;
        Self {
            first: FIRST[(folded % first) as usize],
            second: SECOND[((folded / first) % SECOND.len() as u64) as usize],
        }
    }

    /// The two words, for a surface that wants to draw or measure them
    /// apart.
    pub fn words(self) -> (&'static str, &'static str) {
        (self.first, self.second)
    }
}

impl fmt::Display for RunName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.first, self.second)
    }
}

/// The same as [`Display`](fmt::Display). Written rather than derived so a
/// failing test prints `sunny-otter` and not `RunName { first: "sunny", … }`.
impl fmt::Debug for RunName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// FNV-1a over the id's sixteen bytes.
///
/// Written out because it must never change: `std::hash::DefaultHasher`
/// documents that its output may differ between Rust releases, and a name
/// that moved when the compiler was upgraded would be a name nobody could
/// use to refer to anything.
fn fold(bytes: &[u8; 16]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// The list lengths the module documentation quotes. A word added without
/// the sentence above being corrected fails here rather than leaving the doc
/// saying a number that stopped being true.
#[cfg(test)]
pub(crate) const DOCUMENTED: (usize, usize) = (256, 324);
