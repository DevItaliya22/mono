# Rust, as this codebase uses it

For someone who can read code but has not written Rust. Every example is real —
copied from this repository, with a file to go and look at.

You do not need this to read the design docs. You need it to read
`crates/koru-core/src/schema.rs` without stopping every third line.

---

## 1. The one idea everything else follows from

**Every value has exactly one owner, and Rust knows at compile time who it is.**

There is no garbage collector. When the owner goes out of scope, the value is
freed. That is the whole memory model, and most of the syntax below exists to
say who owns what and who is merely looking.

```rust
let doc = WorkspaceDoc::create(id, "acme")?;   // `doc` owns the document
```

When `doc` goes out of scope the document is freed. Nothing else can free it,
because nothing else owns it.

### Moving

Passing a value to a function normally **gives it away**:

```rust
fn take(doc: WorkspaceDoc) { … }

take(doc);
// `doc` is no longer usable here — it was moved into `take`
```

That is why `Workspace::new` takes the document by value: it is taking
ownership, permanently.

```rust
// crates/koru-workspace/src/lib.rs
pub fn new(ws: WorkspaceDoc) -> Self {
    Self { ws, session: None }
}
```

### Borrowing

Usually you want to *look* at something without taking it. That is a borrow,
written `&`:

```rust
// crates/koru-core/src/schema.rs
pub fn read(&self, id: NodeId) -> Result<Node>
```

`&self` means "I need to look at the document, not consume it". The caller keeps
ownership; the function gets a temporary view.

To *change* something you borrow mutably, `&mut`:

```rust
pub fn set_done(&mut self, target: NodeId, done: bool) -> Result<()>
```

The rule the compiler enforces:

> **Either any number of `&` borrows, or exactly one `&mut`. Never both.**

This is why the whole "document lives outside the view" refactor happened. Two
views cannot each *own* a `WorkspaceDoc`, so instead they borrow it one at a
time:

```rust
// crates/koru-editor/src/editor.rs
pub fn key(&mut self, ws: &mut WorkspaceDoc, key: &Key, …) -> bool
```

Read that as: *this call needs to change the editor **and** change the document,
and while it runs, nobody else may touch either.*

### Why you see `&self.outline.rows[i]` and then a separate line

You will notice code that reads a value out, drops the borrow, and only then
mutates. That is not style — it is the compiler:

```rust
// crates/koru-editor/src/editor.rs
let (id, range) = (row.id, row.selection.text_range());   // copy what we need
// `row` borrow ends here
let _ = ws.delete_text(id, range);                        // now we may mutate
```

Holding `row` (a `&` borrow into the outline) *while* calling something that
needs `&mut` would be the "both at once" case, and would not compile.

---

## 2. Reading a function signature

```rust
pub fn walk_under(&self, root: NodeId) -> Result<Vec<(NodeId, usize)>> {
```

| Piece | Meaning |
| --- | --- |
| `pub` | callable from outside this module. Without it, private |
| `fn` | function |
| `&self` | borrows the thing it is called on, read-only |
| `root: NodeId` | a parameter — **name first, type after the colon** |
| `-> Result<…>` | what it returns |
| `Vec<T>` | a growable array of `T` |
| `(NodeId, usize)` | a *tuple* — two values glued together, no field names |
| `usize` | an unsigned integer the size of a pointer; the type used for lengths and indices |

Other shapes you will see:

```rust
fn parse(s: &str) -> Option<Self>        // no self at all: an "associated function",
                                         // called as Script::parse("sup")
fn feature(self) -> &'static str         // takes self *by value* — fine, because
                                         // Script is Copy (see §6)
```

`&str` is a borrowed string slice; `String` is an owned, growable one. You will
see `&str` in parameters and `String` in structs, for the same
borrow-vs-own reason as everything else.

`&'static str` means a string that lives for the whole program — a literal.

---

## 3. `Option` and `Result`: no null, no exceptions

Rust has neither `null` nor exceptions. Two enums replace both.

### `Option<T>` — there might not be one

```rust
pub icon: Option<String>,        // a node may or may not have an icon
```

It is either `Some(value)` or `None`, and the compiler will not let you use the
value without checking.

### `Result<T, E>` — it might have failed

```rust
pub fn read(&self, id: NodeId) -> Result<Node>
```

Either `Ok(node)` or `Err(problem)`. `Result<Node>` with one parameter is a
local shorthand this crate defines:

```rust
// crates/koru-core/src/schema.rs
pub type Result<T> = std::result::Result<T, StoreError>;
```

so `Result<Node>` means `Result<Node, StoreError>`.

### The four ways to handle them

**`?` — "if this failed, return the failure to my caller".** By far the most
common thing you will see:

```rust
pub fn set_done(&mut self, target: NodeId, done: bool) -> Result<()> {
    let tid = self.tree_id(target)?;                    // <- if Err, return it
    self.tree_of(target)?.get_meta(tid)?.insert(K_DONE, done)?;
    self.doc.commit();
    Ok(())
}
```

`Ok(())` at the end means "succeeded, with no value to hand back". `()` is the
empty tuple — Rust's `void`.

`?` only works in a function that itself returns `Result` (or `Option`).

**`let … else` — "if there is nothing, bail out now".** Used constantly for the
happy-path-first style in this codebase:

```rust
// crates/koru-editor/src/editor.rs
let Some(row) = self.outline.focused() else {
    return;
};
// from here down, `row` is the value, not an Option
```

**`if let` — "do this only when there is something".**

```rust
if let Some(s) = &mut self.session {
    s.touch();
}
```

**`match` — handle every case, and the compiler checks you did.**

```rust
// crates/koru-core/src/node.rs
pub fn as_str(self) -> &'static str {
    match self {
        Self::Start => "start",
        Self::Center => "center",
        Self::End => "end",
    }
}
```

Adding a fourth `Align` variant makes this fail to compile until you handle it.
That is the point: **the compiler finds every place a new case needs
attention.** Several changes in this project were safe precisely because of it.

### Ignoring a failure on purpose

```rust
let _ = ws.set_mark(id, range.clone(), mark, on);
```

`let _ =` says "I know this returns a `Result` and I am deliberately not
checking it". Rust warns about unused `Result`s, so this is how you say the
warning is unwanted here — in the editor, a failed mark is not worth aborting a
keystroke over.

---

## 4. Struct, enum, impl

```rust
// crates/koru-workspace/src/lib.rs
pub struct Workspace {
    pub ws: WorkspaceDoc,
    pub session: Option<Session>,
}

impl Workspace {
    pub fn new(ws: WorkspaceDoc) -> Self {
        Self { ws, session: None }
    }
}
```

- `struct` — fields, like a class with no methods.
- `impl` — a separate block holding the methods. Data and behaviour are written
  apart.
- `Self` (capital) — the type being implemented. `self` (lowercase) — the
  instance.
- `Self { ws, session: None }` — field shorthand: `ws` alone means `ws: ws`.

An `enum` in Rust is much stronger than in most languages: **each variant can
carry different data.**

```rust
// crates/koru-core/src/node.rs
pub enum Content {
    Text { text: String, attrs: Vec<Attr> },
    Blob { hash: BlobHash, mime: String, … },
    Embed { target: Ref },
}
```

A node's content is *exactly one* of these, and you cannot read `text` without
first proving it is a `Text`. That is why the model can say "an image has no
text" and have it be true rather than merely documented.

### Derives

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Mark { Bold, Italic, Underline, Strike }
```

`#[derive(…)]` asks the compiler to write boilerplate:

| | |
| --- | --- |
| `Clone` | can be explicitly duplicated with `.clone()` |
| `Copy` | so cheap it is duplicated automatically on assignment (small, no heap) |
| `PartialEq` / `Eq` | `==` works |
| `Hash` | usable as a `HashMap` key |
| `Debug` | printable with `{:?}`, for tests and diagnostics |
| `Default` | there is a sensible zero value |

Note `TextRun` derives `PartialEq` but **not** `Eq` — it contains an `f32`, and
floats are not fully equatable (`NaN != NaN`). The compiler enforces that
distinction.

---

## 5. Iterators: the functional part

Rust's loops are usually written as chains. Nothing runs until something
consumes the chain, so this allocates once, not five times:

```rust
// crates/koru-core/src/schema.rs — backlinks()
node.content
    .attrs()                       // a slice of Attr
    .iter()                        // -> an iterator of &Attr
    .filter_map(|a| a.as_ref_target())   // keep the Some values, unwrap them
    .any(|r| r.node == target)     // true if any matches
```

`|a| …` is a closure — a lambda. `|a|` is its parameter list.

The ones used most here:

| | |
| --- | --- |
| `.iter()` | borrow each element (`&T`) |
| `.into_iter()` | consume the collection, yielding owned `T` |
| `.map(f)` | transform each |
| `.filter(p)` | keep the ones matching |
| `.filter_map(f)` | transform, dropping the `None`s — map and filter in one |
| `.find(p)` | first match, as `Option` |
| `.position(p)` | *index* of the first match |
| `.any(p)` / `.all(p)` | a bool |
| `.collect()` | build a `Vec`, `String`, `HashMap`… |
| `.chain(other)` | run through one then the other |

A real example of `collect` inferring what to build from the return type:

```rust
// crates/koru-editor/src/editor.rs
let before: Vec<String> = e.outline.rows.iter().map(|r| r.text.clone()).collect();
```

And `?` inside a chain does not work — which is why you will sometimes see a
plain `for` loop where a chain would need to propagate an error:

```rust
for (id, depth) in walk {
    let Ok(node) = ws.read(id) else { continue };
    …
}
```

---

## 6. Small things that look strange at first

**`&` and `*`.** `&x` borrows; `*x` follows a borrow back to the value.
`|(id, _)| *id == target` dereferences because the closure received a reference.

**`_`** means "I do not care about this". As a variable name it silences the
unused warning; in a `match` arm it is the catch-all; in `let (_ws, o) = …` it
says "I must bind this to keep it alive, but I will not use it".

**`::` vs `.`** — `::` reaches into a type or module (`Script::parse`,
`theme::p()`), `.` calls a method on a value (`row.is_task()`).

**`impl Trait` in a parameter** means "anything that can do this":

```rust
pub fn task(text: impl Into<String>) -> Result<Self, ShapeError>
```

Callers may pass `&str` or `String`; both can turn into `String`.

**Lifetimes, `<'a>`.** Occasionally a type has to say how long a borrow inside
it lives:

```rust
pub fn covering<'a>(runs: impl IntoIterator<Item = &'a TextRun>) -> Self
```

Read `'a` as "some lifetime"; the signature says the borrowed runs must outlive
the call. You will rarely need to write these — the compiler infers most.

**Turbofish, `::<>`.** Occasionally needed to tell `collect` what to build:
`.collect::<Vec<_>>()`. The `_` says "work the element type out yourself".

**Attributes**, `#[...]`, are annotations:

| | |
| --- | --- |
| `#[test]` | this function is a test |
| `#[derive(…)]` | generate boilerplate |
| `#[cfg(test)]` | only compile this when testing |
| `#[allow(dead_code)]` | do not warn about unused items here |

**`//!` vs `///`.** `//!` documents the *file it is in* (always at the top);
`///` documents the *next item down*. Both are the real documentation in this
project. The public `guide/` folder explains usage and implementation rules;
doc comments explain code.

---

## 7. Modules and crates

- A **crate** is a compilation unit and a dependency. This repo has three of its
  own: `koru-core`, `koru-docstore`, `koru` (the app).
- A **module** is a file or a block. `mod outline;` in `main.rs` pulls in
  `outline.rs`.
- `use` brings a name into scope: `use koru_core::schema::WorkspaceDoc;`.
- `crate::` means "from the root of *this* crate"; `super::` means "one module
  up" — you see that at the top of every test block:

```rust
#[cfg(test)]
mod tests {
    use super::*;      // everything from the file this test block is inside
```

Note the naming flip: the crate `koru-core` is written `koru_core` in code.
Hyphens in the manifest, underscores in Rust.

---

## 8. Tests live next to the code

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_keys_are_stable() {
        assert_eq!(Mark::Bold.key(), "b");
    }
}
```

`#[cfg(test)]` means the block is compiled only when running tests, so it costs
nothing in the shipped binary — and because it is *inside* the file, it can
reach private functions. Integration tests in `crates/koru-core/tests/` are
separate crates and see only the public API.

`assert_eq!(a, b, "message")` — the `!` means macro, not function. Macros can do
things functions cannot, like printing both values when they differ.

---

## 9. Where to look next

Reading order that will make sense:

1. `crates/koru-core/src/text.rs` — small, self-contained, lots of enums and
   iterators.
2. `crates/koru-core/src/node.rs` — the sum type and its shape rules.
3. `crates/koru-editor/src/body.rs` — borrowing in anger: a view that does not own
   its data.
4. `crates/koru-core/src/schema.rs` — the big one. Now readable.

The libraries have their own primers: [LORO.md](deps/LORO.md),
[PARLEY.md](deps/PARLEY.md), [VELLO.md](deps/VELLO.md),
[WINIT.md](deps/WINIT.md).

Beyond this file, [*The Rust Book*](https://doc.rust-lang.org/book/) is genuinely
good, and chapter 4 (ownership) is the one that matters.
