# koru-ui

**Composed widgets: the pieces of interface that more than one view needs.**

A table, a toast, a confirmation dialog, a button. Things that have a shape, a
hover state, a hit test and a way of being drawn — and no opinion whatsoever
about what they are showing.

## The rule

> **This crate names no node, no workspace and no document.**
>
> A widget takes plain data in and reports a plain outcome out. Anything that
> needs a `NodeId` to work is not a widget; it belongs to the view that owns it.

The manifest enforces it: `koru-ui` depends on `koru-draw`, `vello`, `winit`,
the two `text_field` brought with it — `parley`, because it drives a
`PlainEditor`, and mechanism-only `koru-clipboard`, for plain text — and the two
`date_grid` brought with it: `koru-when`, the natural-language date grammar,
and `chrono`, whose `NaiveDate` is the day the grid answers. If it ever grows an
`koru-core` dependency, the rule has already been broken — go and find which
widget reached for a node and give the id back to its caller.

`date_grid` is the worked example of the rule: it draws a calendar and answers a
`Pick` — a day, a phrase carrying a clock, a request to clear, or nothing.
Turning that into `koru.due` is `koru_editor::components::date_picker`'s, and
that adapter is where every `NodeId` lives (ADR-196).

It is not an aesthetic preference. Two things fall out of it:

- **A widget is constructible in a test without a document.** Almost every test
  in here is pure geometry: build the thing, ask where it drew a control, ask
  what a click at that point hits, assert the two agree. No workspace, no
  fixture, no font.
- **A widget can be rendered on its own.** That is what makes the component
  gallery in [`docs/contributing/techdebt/README.md`](../../docs/contributing/techdebt/README.md) (#3) possible at all —
  a gallery that had to depend on the container to draw a button would be the
  app again, with extra steps. Being a crate rather than a module inside
  `apps/koru` is the whole of that difference.

## Why this crate and not `koru-draw`

`koru-draw` is **primitives**: the palette, the type scale, glyphs, geometry,
the scene helpers that put shapes and text on a vello `Scene`. It knows vello and
parley. It owns no state and draws no *thing* — `scene::floating_panel` will draw
you a panel, but it does not know that the panel is a menu.

`koru-ui` is **the things**. A `Table` remembers its metrics and its scroll and
can tell you which row is at a point. A `Toast` knows when it appeared and
whether it is still fading. A `Confirm` knows which of its two buttons has the
keyboard.

The dividing question, when you are unsure where something goes: *does it have a
state, or is it a function of its arguments?* `draw_button` is a function of its
arguments and lives here only because the three dialogs that call it are here
too; `Table` has state and could live nowhere else.

## What is in it

| module | what |
| --- | --- |
| `table.rs` | Columns, bands of rows, a reserved trailing lane. One function owns the row arithmetic. |
| `button.rs` | Labelled buttons: dialog answers plus compact Primary/Ghost actions with an optional icon. |
| `chip.rs` | 28px semantic choices with shared selected, hover, focus, border, and overflow treatment; optional wrapping rows preserve measured widths. |
| `icon_button.rs` | Shared rest, hover, active and disabled treatment with standard and compact metrics. |
| `caption.rs` | Neutral application-drawn window controls: one geometry answer for draw/hit testing, host-owned window requests. |
| `divider.rs` | Divider lanes whose geometry owns equal spacing around the line. |
| `toast.rs` | A message with an optional action; transient by default, persistent while it composes progress. |
| `progress_bar.rs` | Determinate and indeterminate progress with a reusable animation contract and caller-owned layout. |
| `file_icon.rs` | MIME-first file glyph selection with extension fallback for entries created before MIME storage. |
| `copy_affordance.rs` | Copy icon that writes plain text, confirms with a check, and reports `Copied` so the caller can raise the existing toast. |
| `confirm.rs` | A modal question with a destructive answer that is never the default. |
| `selectable_list.rs` | Keyboard selection, scrolling, and row hit geometry shared by search-style lists. |
| `highlighted_text.rs` | UTF-8-safe multi-span elision and measured regular/bold run painting that preserves authored whitespace. |
| `popover.rs` | An anchored panel shell that aligns, flips, clamps, and frames menus without knowing their content. |
| `modal.rs` | A modal root: the full-window scrim and content callback, hoisted to lead the inspector tree though it paints last. |
| `prompt.rs` | A confirmation surface — title, wrapped body, right-aligned KoruButton actions — generic over the host's action type, with draw and hit geometry from one layout. |
| `tooltip.rs` | Flat semantic tooltips with the shared hover delay, warm-window, keyboard-focus, fade and translation state machine. |
| `exit_list.rs` | Content-agnostic timing for strike-and-collapse list exits without owning the rows. |
| `text_field.rs` | One line of editable text: undo, word motion, horizontal scroll, and the dictation composition. Moved out of `koru-editor`, which re-exports it. Also the panel search box's geometry, which four panels in two crates share. |
| `dictation.rs` | `trait DictationTarget` — begin, update, commit, cancel: what a text surface answers so speech can reach it. A module rather than a crate because both implementors (`TextField` here, the document in `koru-editor`) already sit at or above this one (ADR-248). |
| `date_grid.rs` | A month grid, the phrase box over `koru-when` that refines its selection, and a Clear row. Answers a `Pick`; what a day means is the host's. |
| `dates.rs` | Calendar arithmetic: month start and length, the month shift with its short-month clamp, the Monday-first column, month and weekday names. |
| `text_input.rs` | A `TextField` paired with hit geometry and box/caret styling. |
| `keys.rs` | `ctrl_letter` — which letter a Ctrl chord is actually on, across two platform encodings. |
| `clipboard.rs` | One line of plain text on and off the system clipboard. Deliberately not the editor's rich payload. |

## Two conventions worth knowing before you add a fifth

**Rows are passed in on every call, never owned.** The caller's list of rows is a
pure function of the caller's own data; a widget that cached it would have a
second copy to keep in step. `Table::row_rect(bands, i)` takes the bands.

**The trailing lane is reserved whether or not a control is showing.** A button
that took its space only on hover would reflow the text beside it as the pointer
moved down a list, which reads as the list twitching rather than as a button
arriving. Same rule the explore column's workspace rows already follow.

**A toast never hides its message.** It grows at the message's natural width up
to 500 px, then wraps instead of eliding. Its action follows the paragraph in
the same flow rather than taking a separate column. Author toast copy to fit
within three lines and under a five-second read; information that needs more
belongs in a prompt, alert, modal, or durable notice rather than being squeezed
into transient feedback.
