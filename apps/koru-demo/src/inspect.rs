// SPDX-License-Identifier: MPL-2.0

//! **The seam between asking a question and a COM apartment answering it.**
//!
//! Everything a UI Automation element is lives on one thread and is not `Send`:
//! the apartment, the `IUIAutomation`, every `IUIAutomationElement`. That is not
//! a detail to work around — it is the reason this module exists. The runner's
//! own logic (which control, how long to wait, what to do about two matches)
//! must be able to run, and be tested, with no COM anywhere; and the thread that
//! holds COM must be one the runner can **walk away from**, because a provider
//! in another process can stop answering and there is no call to cancel.
//!
//! So the seam is plain data in both directions, and an element is named by a
//! [`Handle`] the platform side minted rather than by a pointer:
//!
//! - [`Request`] and [`Response`] are `Send` and contain no COM type.
//! - [`Handle`] carries the **generation** of the worker that minted it, so a
//!   handle from a worker the supervisor abandoned is refused by its successor
//!   rather than silently answered about a different element.
//! - [`ElementFacts`] is what a client is allowed to know about a control, and
//!   it deliberately has **no name field**. See the module note in
//!   [`crate`]: §7.4 of the roadmap is absolute, and a diagnostic that quoted an
//!   accessible name would put a document title in a report the first time
//!   somebody ran the runner against real content.
//!
//! ## Why a name can be *matched* but never *reported*
//!
//! Role/name fallback is in the plan, and it needs names. The asymmetry is
//! deliberate: a name arrives in a [`Request::MatchesByName`] because the case
//! author typed it, and it is compared inside the apartment. It never comes
//! back through ordinary facts. Explicit prepared-fixture snapshots use a separate
//! content type and request; its Debug representation remains redacted. Default
//! diagnostics therefore cannot accidentally copy an accessible name or text.

use std::fmt;

use crate::locator::LocatorPattern;
mod snapshot;
pub use snapshot::ContentSnapshot;

/// A control the platform side is holding for us.
///
/// `generation` is the worker that minted it. A supervisor that gives up on a
/// wedged provider starts a fresh worker with a fresh generation, and handles
/// from the abandoned one are then answered with [`InspectError::StaleHandle`].
/// Without it, slot 3 of the new worker would quietly answer questions asked
/// about slot 3 of the old one — a different control, in a tree that has since
/// changed.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    pub generation: u64,
    pub slot: u64,
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}.{}", self.generation, self.slot)
    }
}

/// The control types this runner distinguishes, by UI Automation's own ids.
///
/// A small closed set plus [`Role::Other`], because the runner needs to *say*
/// what it found and a bare integer in a failure message is a lookup somebody
/// has to do by hand. Nothing branches on the closed set — it is vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Button,
    CheckBox,
    ComboBox,
    Edit,
    Document,
    Group,
    Hyperlink,
    List,
    ListItem,
    Menu,
    MenuItem,
    Pane,
    Tab,
    TabItem,
    Text,
    Tree,
    TreeItem,
    Window,
    Other(i32),
}

impl Role {
    /// UI Automation's `UIA_*ControlTypeId` numbers.
    ///
    /// Written out rather than imported so this module — and its tests — stay
    /// free of `windows`, which is the whole point of the seam.
    pub fn from_control_type(id: i32) -> Self {
        match id {
            50000 => Self::Button,
            50002 => Self::CheckBox,
            50003 => Self::ComboBox,
            50004 => Self::Edit,
            50005 => Self::Hyperlink,
            50007 => Self::ListItem,
            50008 => Self::List,
            50009 => Self::Menu,
            50011 => Self::MenuItem,
            50018 => Self::Tab,
            50019 => Self::TabItem,
            50020 => Self::Text,
            50023 => Self::Tree,
            50024 => Self::TreeItem,
            50026 => Self::Group,
            50030 => Self::Document,
            50032 => Self::Window,
            50033 => Self::Pane,
            other => Self::Other(other),
        }
    }

    /// The number to hand a property condition.
    pub fn control_type(self) -> i32 {
        match self {
            Self::Button => 50000,
            Self::CheckBox => 50002,
            Self::ComboBox => 50003,
            Self::Edit => 50004,
            Self::Hyperlink => 50005,
            Self::ListItem => 50007,
            Self::List => 50008,
            Self::Menu => 50009,
            Self::MenuItem => 50011,
            Self::Tab => 50018,
            Self::TabItem => 50019,
            Self::Text => 50020,
            Self::Tree => 50023,
            Self::TreeItem => 50024,
            Self::Group => 50026,
            Self::Document => 50030,
            Self::Window => 50032,
            Self::Pane => 50033,
            Self::Other(id) => id,
        }
    }

    /// The name to print. A `Role` in a failure message is read by a person.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Button => "Button",
            Self::CheckBox => "CheckBox",
            Self::ComboBox => "ComboBox",
            Self::Edit => "Edit",
            Self::Document => "Document",
            Self::Group => "Group",
            Self::Hyperlink => "Hyperlink",
            Self::List => "List",
            Self::ListItem => "ListItem",
            Self::Menu => "Menu",
            Self::MenuItem => "MenuItem",
            Self::Pane => "Pane",
            Self::Tab => "Tab",
            Self::TabItem => "TabItem",
            Self::Text => "Text",
            Self::Tree => "Tree",
            Self::TreeItem => "TreeItem",
            Self::Window => "Window",
            Self::Other(_) => "control type",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other(id) => write!(f, "control type {id}"),
            named => f.write_str(named.as_str()),
        }
    }
}

/// A rectangle in physical screen pixels, as UI Automation reports one. The
/// type lives in `koru-recording`: capture geometry and UIA bounds share one
/// physical-pixel rectangle, so pairing never converts.
pub use koru_recording::Rect;

/// What a client is allowed to know about a control.
///
/// **There is no name here.** See the module note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementFacts {
    pub handle: Handle,
    /// The `koru:` locator, read from `UIA_AutomationIdPropertyId`. `None` for
    /// anything that is not ours — which is itself worth reporting, because a
    /// locator that matched something without one would be a bug in this
    /// runner rather than in Koru.
    pub locator: Option<String>,
    pub role: Role,
    pub enabled: bool,
    /// `UIA_IsOffscreenPropertyId`. An off-screen paragraph is a real control
    /// that a screen reader can reach, so this is reported and not filtered —
    /// unless a caller says it needs an on-screen one. See
    /// [`crate::select::Requirements`].
    pub offscreen: bool,
    pub bounds: Option<Rect>,
    /// The process the control belongs to, so a match can be proved to be in
    /// the instance the runner was pointed at rather than in a second Koru.
    pub process_id: Option<u32>,
}

/// Everything the runner can ask the apartment for.
///
/// Deliberately coarse: each variant is one round trip, because every round
/// trip is a call that can wedge and a wedge costs a whole worker.
#[derive(Clone, PartialEq, Eq)]
pub enum Request {
    /// Bind to a window by its `HWND`, poll until it publishes a `koru:`
    /// locator, and return its root.
    ///
    /// The poll is not politeness. `accesskit_windows` serves a *placeholder*
    /// tree — one `Role::Window` node with `NodeId(0)` — until the window loop's
    /// next pass swaps in the real one, so a client that reads a property
    /// straight after `ElementFromHandle` reads the placeholder. Established in
    /// `guide/contributing/deps/ACCESSKIT.md`; every interactive proof in the tree polls for
    /// it and so does this.
    BindWindow {
        native_handle: i64,
    },
    /// Every element under `root` whose locator matches the pattern.
    Matches {
        root: Handle,
        pattern: LocatorPattern,
    },
    /// Role/name fallback, for a control with no Koru locator.
    ///
    /// `name` is the language the case author declared — §7.2 requires a
    /// human-readable name locator to say which language it is in, because the
    /// same control is called something else in the other seven.
    MatchesByName {
        root: Handle,
        role: Role,
        name: String,
        language: String,
    },
    /// Re-read one element. A control that has gone answers
    /// [`InspectError::ElementGone`], which is the only error that means
    /// *this handle will never work again* rather than *ask again later*.
    Facts {
        element: Handle,
    },
    MatchesPart {
        root: Handle,
        part: String,
    },
    Focused {
        root: Handle,
    },
    Check {
        element: Handle,
        condition: crate::input::Condition,
    },
    /// Explicit content opt-in for one model paragraph under the named window.
    Snapshot {
        root: Handle,
        element: Handle,
        max_utf16: u32,
    },
    /// Bounded fixture discovery, returning model facts only in tree order.
    FixtureChildren {
        root: Handle,
        part: String,
        max_nodes: u32,
    },
    Input(crate::input::Consequence),
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BindWindow { .. } => f.write_str("BindWindow"),
            Self::Matches { pattern, .. } => f.debug_tuple("Matches").field(pattern).finish(),
            Self::MatchesByName { role, .. } => f.debug_tuple("MatchesByName").field(role).finish(),
            Self::Facts { element } => f.debug_tuple("Facts").field(element).finish(),
            Self::MatchesPart { part, .. } => f.debug_tuple("MatchesPart").field(part).finish(),
            Self::Focused { .. } => f.write_str("Focused"),
            Self::Check { condition, .. } => f.debug_tuple("Check").field(condition).finish(),
            Self::FixtureChildren {
                part, max_nodes, ..
            } => f
                .debug_struct("FixtureChildren")
                .field("part", part)
                .field("max_nodes", max_nodes)
                .finish(),
            Self::Snapshot {
                element, max_utf16, ..
            } => f
                .debug_struct("Snapshot")
                .field("element", element)
                .field("max_utf16", max_utf16)
                .finish(),
            Self::Input(input) => f.debug_tuple("Input").field(input).finish(),
        }
    }
}

/// A window root, and the two things about it that are not element facts.
///
/// `class_name` and `framework` are how a window is proved to be one of ours —
/// `"KoruWindow"` from `Node::class_name` and `"Koru"` from
/// `Tree::toolkit_name`, both in `guide/contributing/deps/ACCESSKIT.md`'s property table.
/// They live here rather than on [`ElementFacts`] because they are asked once,
/// of one element, at the moment the runner decides whether to proceed at all —
/// and because keeping [`ElementFacts`] to exactly what a *search result* may
/// say is what makes the no-name rule checkable by reading it.
///
/// Neither is a name in the §7.4 sense: both are code keywords a build chose,
/// untranslated and unrenameable, which is the same test a locator passes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundWindow {
    pub facts: ElementFacts,
    pub class_name: Option<String>,
    pub framework: Option<String>,
}

/// What came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Bound(Box<BoundWindow>),
    Found(Vec<ElementFacts>),
    Facts(ElementFacts),
    Checked(bool),
    Snapshot(ContentSnapshot),
    Input(crate::input::Result),
    Failed(InspectError),
}

/// Why a question could not be answered.
///
/// The split follows the one `apps/koru-agent/src/global_input/windows.rs`
/// already makes and `guide/contributing/deps/WIN32.md` records: `UIA_E_ELEMENTNOTAVAILABLE`
/// is the element being **gone**, and every other provider failure is
/// **unavailable** and fails closed. Collapsing them would turn "this control
/// was destroyed" into "try again", which is how a runner ends up waiting out a
/// full timeout on a window that closed a second ago.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectError {
    /// The element this handle names has been destroyed.
    ElementGone,
    /// A handle minted by a worker that was abandoned. See [`Handle`].
    StaleHandle,
    /// The window never published a Koru locator within the bind deadline.
    NotKoru,
    /// The provider answered with something other than *not available*. Unknown
    /// is unsafe, so it is closed rather than retried as an ordinary miss.
    Unavailable(String),
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ElementGone => f.write_str("the element has been destroyed"),
            Self::StaleHandle => f.write_str("the handle belongs to a worker that was abandoned"),
            Self::NotKoru => {
                f.write_str("the window never published a koru: locator before the deadline")
            }
            Self::Unavailable(why) => write!(f, "the provider could not answer ({why})"),
        }
    }
}
