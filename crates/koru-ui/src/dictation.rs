// SPDX-License-Identifier: MPL-2.0

//! What a text surface has to answer for dictation to reach it.
//!
//! Voice is **an ephemeral composition routed to the focused editable
//! surface**, committed as one final logical edit and one undo step
//! ([ADR-248](../../../docs/decisions/ADR-226-250.md#adr-248-voicecomposition--dictation-is-an-ephemeral-composition-and-only-the-final-transcript-is-a-transaction)).
//! A recogniser revises itself several times a second — *"Create a new" →
//! "Create a new project" → "Create a new project plan"* — so the revisions are
//! provisional and only the last one is an edit. That is the same semantic model
//! a keyboard IME already uses, which is why the vocabulary below is
//! begin/update/commit/cancel rather than anything voice-shaped.
//!
//! ## Why a trait in `koru-ui` and not a crate
//!
//! The chrome text widgets and the document editor implement it without a
//! shared concrete type. `koru-ui` already sits under both, so a
//! module here is visible to both without a new crate in the graph. Adding one
//! would put a leaf beneath `koru-draw` whose whole content is five method
//! signatures, which is the kind of thing
//! [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) is about.
//!
//! ## Why the context is an associated type
//!
//! A field needs a [`TextEngine`](koru_draw::text::TextEngine) to lay a line
//! out; a document needs its `WorkspaceDoc` to write a node. Neither can be
//! named in one signature, and neither is storable on the implementor — the
//! engine is borrowed per frame and the document is borrowed per event. So the
//! trait names *what it must be handed* and leaves both to the caller who is
//! already holding it. A generic parameter on the trait would let one type
//! implement it twice with different contexts, which is not a capability
//! anything wants and is a source of inference failures at every call site.
//!
//! ## The coordinate space is the implementor's, and is declared per implementor
//!
//! [`composition_rect`](DictationTarget::composition_rect) is deliberately
//! *not* in window coordinates. A chrome field is drawn by whoever owns its
//! rectangle and does not know where that is; a leaf reports in its own view
//! space. Two surfaces answering in different spaces is fine as long as neither
//! caller can consume them interchangeably by accident, so the space is part of
//! each implementation's doc comment and the conversion is the host's.

use koru_draw::geometry::Rect;

/// A surface a dictation can be composed into.
///
/// The four verbs are a state machine with one state: *composing* or not.
/// `begin` opens it, `update` may be called any number of times including zero,
/// and exactly one of `commit` or `cancel` closes it. A caller that loses track
/// — the window goes away mid-sentence — must call `cancel`; **an interruption
/// discards**, because half a sentence landing in a document nobody was looking
/// at is worse than losing it.
pub trait DictationTarget {
    /// What this surface must be handed to be edited: a
    /// [`TextEngine`](koru_draw::text::TextEngine) for a field, a
    /// `WorkspaceDoc` for a document.
    type Cx<'a>;

    /// Whether speech may be composed into this surface at all.
    ///
    /// A device-link code field and a read-only document answer `false`. The
    /// default is `true` because refusing is the exception and a surface that
    /// forgot to opt *in* would be a bug you find by talking at it.
    fn accepts_dictation(&self) -> bool {
        true
    }

    /// Open a composition over the current selection, or at the caret when it
    /// is collapsed — **dictation replaces a selection exactly as typing
    /// does**.
    ///
    /// Answers whether one is now open. `false` means the surface refused:
    /// it does not accept dictation, or one is already open and the caller has
    /// lost track of it.
    fn begin_composition(&mut self, cx: Self::Cx<'_>) -> bool;

    /// Replace the provisional text. Called at most about ten times a second.
    ///
    /// **No undo entry, ever.** The whole point of the composition is that a
    /// revision is not an edit.
    fn update_composition(&mut self, cx: Self::Cx<'_>, text: &str);

    /// Close the composition by making `text` real: one logical edit, one undo
    /// step, and the composition is no longer open.
    fn commit_composition(&mut self, cx: Self::Cx<'_>, text: &str);

    /// Close the composition by throwing it away: no edit, no undo entry, no
    /// trace. The surface is byte-identical to what it was before `begin`.
    fn cancel_composition(&mut self, cx: Self::Cx<'_>);

    /// Where the provisional text is, **in this implementor's own coordinate
    /// space** — see the module note. `None` when nothing is being composed.
    ///
    /// The caller converts. A chrome field and a leaf report in different
    /// spaces and must not be consumed interchangeably.
    fn composition_rect(&self) -> Option<Rect>;
}

/// **One of this crate's text surfaces, handed out with the engine that
/// lays it out**, so a host can route a dictation into whichever it owns
/// without a `match` at every call site.
///
/// [`DictationTarget`] cannot be a `dyn` trait — its context is an associated
/// type, and a trait with a GAT is not object safe — so the implementations
/// are named here instead. That is a closed set on purpose: another text
/// widget would be another arm rather than another crate.
///
/// The engine and the field are two `&mut` borrows of **disjoint fields** of
/// whatever owns them, which is why the surfaces above answer with a pair
/// rather than with `&mut self`.
pub enum ComposeInto<'a> {
    Area(&'a mut crate::TextArea, &'a mut koru_draw::text::TextEngine),
    Field(
        &'a mut crate::TextField,
        &'a mut koru_draw::text::TextEngine,
    ),
    Input(
        &'a mut crate::TextInput,
        &'a mut koru_draw::text::TextEngine,
    ),
    /// A surface that **already holds what it is edited through** — the AI
    /// chat's composer, which is an editor over a private document it owns
    /// (AGENTUX I6). Its context is `()`, so it needs no borrowed engine or
    /// workspace beside it, and it can travel as a trait object where a
    /// [`DictationTarget`] cannot.
    Held(&'a mut dyn HeldComposition),
}

/// A [`DictationTarget`] whose context is `()`, as a trait object.
///
/// [`DictationTarget`] is not object safe — its context is a generic
/// associated type — so `koru-ui` cannot name a surface that lives in a crate
/// above it. A surface that owns its own document can answer with `()`, and
/// the blanket impl below turns that into this object-safe mirror, so a host
/// routes it through [`ComposeInto::Held`] without this crate learning what it
/// is.
pub trait HeldComposition {
    fn begin(&mut self) -> bool;
    fn update(&mut self, text: &str);
    fn commit(&mut self, text: &str);
    fn cancel(&mut self);
}

impl<T> HeldComposition for T
where
    T: for<'a> DictationTarget<Cx<'a> = ()>,
{
    fn begin(&mut self) -> bool {
        self.begin_composition(())
    }

    fn update(&mut self, text: &str) {
        self.update_composition((), text);
    }

    fn commit(&mut self, text: &str) {
        self.commit_composition((), text);
    }

    fn cancel(&mut self) {
        self.cancel_composition(());
    }
}

impl ComposeInto<'_> {
    /// Open a composition. See [`DictationTarget::begin_composition`].
    pub fn begin(&mut self) -> bool {
        match self {
            Self::Area(area, engine) => area.begin_composition(engine),
            Self::Field(field, engine) => field.begin_composition(engine),
            Self::Input(input, engine) => input.begin_composition(engine),
            Self::Held(held) => held.begin(),
        }
    }

    /// Replace the provisional text. See
    /// [`DictationTarget::update_composition`].
    pub fn update(&mut self, text: &str) {
        match self {
            Self::Area(area, engine) => area.update_composition(engine, text),
            Self::Field(field, engine) => field.update_composition(engine, text),
            Self::Input(input, engine) => input.update_composition(engine, text),
            Self::Held(held) => held.update(text),
        }
    }

    /// One edit, one undo step. See [`DictationTarget::commit_composition`].
    pub fn commit(&mut self, text: &str) {
        match self {
            Self::Area(area, engine) => area.commit_composition(engine, text),
            Self::Field(field, engine) => field.commit_composition(engine, text),
            Self::Input(input, engine) => input.commit_composition(engine, text),
            Self::Held(held) => held.commit(text),
        }
    }

    /// No edit, no undo entry, no trace. See
    /// [`DictationTarget::cancel_composition`].
    pub fn cancel(&mut self) {
        match self {
            Self::Area(area, engine) => area.cancel_composition(engine),
            Self::Field(field, engine) => field.cancel_composition(engine),
            Self::Input(input, engine) => input.cancel_composition(engine),
            Self::Held(held) => held.cancel(),
        }
    }
}
