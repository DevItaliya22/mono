// SPDX-License-Identifier: MPL-2.0

//! **What the surface is handed, and what it hands back.**
//!
//! Plain values. The container reads its document and builds a [`Scene`];
//! this crate draws it and answers input with [`CanvasRequest`]s the
//! container performs against the document. Neither type names anything in
//! Koru's model — a key is a UUID, a position is a pair of integers — so the
//! surface can be tested without a store, a workspace or a window.

use uuid::Uuid;

/// Which object on the canvas. The container uses the node's own id, so a
/// request names exactly the node it is about.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ObjectKey(pub Uuid);

/// What an object is. A card and an image are the two things a person
/// arranges; start, agent and output are the three workflow nodes a chain of
/// agents is built from.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ObjectKind {
    Card,
    Image,
    Start,
    Agent,
    Output,
}

impl ObjectKind {
    /// A workflow node — the three kinds with a header and a status.
    pub fn is_node(self) -> bool {
        matches!(self, Self::Start | Self::Agent | Self::Output)
    }

    /// Whether this kind has a Run button. An output node is written by the
    /// chain; it is not run on its own.
    pub fn runs(self) -> bool {
        matches!(self, Self::Start | Self::Agent)
    }

    /// Whether this kind belongs to the workflow half — the nodes a chain
    /// is made of — rather than to the plain canvas of cards and images.
    pub fn is_workflow(self) -> bool {
        matches!(self, Self::Start | Self::Agent | Self::Output)
    }
}

/// The picture an image object shows: the blob hash it is drawn from and its
/// natural size, which fixes the aspect ratio every width keeps.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SceneImage {
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

/// Where a workflow node stands. The words for the dynamic parts — what a
/// running agent is doing, how long it has been at it, why it failed — are
/// the caller's; the fixed words are this crate's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NodeStatus {
    Idle,
    Running {
        activity: String,
        elapsed: String,
    },
    /// An active run waiting for an approval or a reply.
    NeedsResponse,
    Done,
    Failed(String),
    Stopped,
    NotStarted,
}

impl NodeStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. } | Self::NeedsResponse)
    }
}

/// One object on the canvas.
///
/// **Positions are absolute canvas coordinates in whole logical pixels at
/// 100 % zoom.** Height is never stored: a card's follows its text at its
/// width, an image's follows its aspect ratio.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SceneObject {
    pub key: ObjectKey,
    pub kind: ObjectKind,
    pub x: i32,
    pub y: i32,
    /// Zero means "not set yet"; the surface draws [`crate::DEFAULT_WIDTH`].
    pub w: i32,
    /// The first line: a card's text, a node's title or instruction.
    pub text: String,
    /// The further lines, drawn beneath the first.
    pub lines: Vec<String>,
    pub image: Option<SceneImage>,
    /// An agent node's agent, by display name. `None` is "not chosen yet",
    /// and the header offers **Choose agent…** instead.
    pub agent: Option<String>,
    /// Start, agent and output nodes. `None` draws no status line.
    pub status: Option<NodeStatus>,
    /// An agent node's result preview, one entry per line. The surface shows
    /// the first five.
    pub result: Vec<String>,
    /// An output node's page, by title, once there is one.
    pub link: Option<String>,
}

impl SceneObject {
    /// A plain object of `kind` with nothing but its place and first line —
    /// the shape a test or a first draft builds.
    pub fn new(key: ObjectKey, kind: ObjectKind, x: i32, y: i32, w: i32, text: &str) -> Self {
        Self {
            key,
            kind,
            x,
            y,
            w,
            text: text.to_owned(),
            lines: Vec::new(),
            image: None,
            agent: None,
            status: None,
            result: Vec::new(),
            link: None,
        }
    }
}

/// A reference arrow from one object to another, with an optional label.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SceneArrow {
    pub key: ObjectKey,
    pub from: ObjectKey,
    pub to: ObjectKey,
    pub label: String,
}

/// Everything on one canvas.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Scene {
    /// In stacking order: the last one is drawn on top.
    pub objects: Vec<SceneObject>,
    /// Objects with no position yet — a line added on the phone, by an AI
    /// tool or by an older build. Drawn in a column to the right of the
    /// content, in the order given, until somebody moves one. Their `x` and
    /// `y` are ignored.
    pub arrivals: Vec<SceneObject>,
    pub arrows: Vec<SceneArrow>,
}

impl Scene {
    /// Nothing to draw but the empty state.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.arrivals.is_empty()
    }
}

/// What only the container can do, because only it holds the document.
///
/// **A gesture is one request, sent when it ends**: a drag is one `Move` per
/// object on release, a resize one `SetWidth`, a held arrow key one `Move`
/// per object when it is let go. Nothing is sent for a gesture that was
/// cancelled — Esc, or the window losing focus.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CanvasRequest {
    /// A new card or workflow node with its top-left corner at this canvas
    /// point. Never `Image`: an image arrives through [`Self::InsertImage`].
    AddObject {
        kind: ObjectKind,
        x: i32,
        y: i32,
    },
    /// Ask the person for an image and put it here.
    InsertImage {
        x: i32,
        y: i32,
    },
    Move {
        key: ObjectKey,
        x: i32,
        y: i32,
    },
    SetWidth {
        key: ObjectKey,
        w: i32,
    },
    Connect {
        from: ObjectKey,
        to: ObjectKey,
    },
    Delete {
        keys: Vec<ObjectKey>,
    },
    DeleteArrow {
        key: ObjectKey,
    },
    /// Open this object's text for editing — the container shows the dialog.
    EditText {
        key: ObjectKey,
    },
    Run {
        key: ObjectKey,
    },
    Stop {
        key: ObjectKey,
    },
    /// Open the agent's pending question and response controls.
    Respond {
        key: ObjectKey,
    },
    /// Show the agent menu for this agent node.
    ChooseAgent {
        key: ObjectKey,
    },
    /// Put this agent node's result on the canvas as cards.
    PlaceResult {
        key: ObjectKey,
    },
    /// Open the page this output node wrote.
    OpenLink {
        key: ObjectKey,
    },
    BringToFront {
        key: ObjectKey,
    },
    SendToBack {
        key: ObjectKey,
    },
}
