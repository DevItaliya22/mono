# Adding a new block kind

A **block** is a node `Kind` — `Task`, `Paragraph`, `Document`, `File`, `Table`, `Image`,
`Canvas`, `Contact`, `Divider`. This is the pre-flight checklist for adding one.
It exists so a new block is a **deliberate** change rather than a silent one: the
codebase forces the first three layers via exhaustive `match`es with no `_` arm,
so a new variant is a compile error until someone decides each case.

The `Divider` kind (a horizontal rule) is the worked example — see
ADR-44 for the
decision that established this checklist.

## Layers every new block touches

1. **Model** — `crates/koru-core/src/node.rs`
   - Add the variant to `Kind`.
   - `shape` (Text / Blob), `accepts_children`, `is_task_like`.
   - `as_str` (the **stable storage string** — never rename after a release) and
     `parse`.
   - A convenience constructor on `Node` (like `Node::divider()`).
   - Any extra `Node::check` enforcement (e.g. a Divider must carry empty text).
2. **Schema version** — `crates/koru-core/src/schema.rs`,
   `crates/koru-core/tests/schema_version.rs`
   - Bump `SCHEMA_VERSION` and update the canary. Decide the migration stance:
     usually a new kind needs **no data migration** (old docs contain none), and
     the bump's only purpose is so an older build opens a newer doc **read-only**
     rather than re-exporting it without the unknown nodes.
3. **Explore visibility** — `apps/koru/src/panes/explore.rs`,
   `crates/koru-core/src/query.rs`
   - `is_landmark`: does it earn a row in the explore tree, or is it body content
     read only inside its document? `icon_for`, `label_for`, `query::label_of`.
4. **Render** — `crates/koru-editor/src/draw.rs`, `rowlayout.rs`, `paragraph.rs`
   - A draw branch for the new row (mirror the image-row or divider branch), a
     row height, and geometry. `Paragraph` already carries `kind`.
5. **Input** — `crates/koru-editor/src/editor.rs`
   - Enter / Backspace / type-into behaviour for a non-text block (mirror the
     image path: `enter_after_image`, the Backspace arm).
6. **Slash command** — `crates/koru-editor/src/components/command_menu.rs`,
   `trigger.rs`, `editor.rs::choose_command`
   - `Command` variant, the view-gate flag, and a `commit_*` that turns the
     command text into the block.
7. **Markdown shorthand** — `crates/koru-editor/src/editor.rs::type_text`
   - If the block has a natural trigger sequence (like `# ` for heading or `---`
     for divider), add a `*_shorthand` mirroring `heading_shorthand`.
8. **View gating** — `crates/koru-editor/src/view_state.rs`
   - A `*_enabled` flag, set true only in the views that offer the block.
9. **Clipboard** — `crates/koru-editor/src/clipboard.rs`
   - `CopiedRow` carry, `paste_rows` handling, and the plain-text `render()`
     flavour out.
10. **External interop** — `crates/koru-workspace-host/src/clipboard_html.rs`,
    `clipboard_rtf.rs`, `lib.rs` (`RichBlock`)
    - Only when the HTML/RTF **writer** exists. Until then, record the touch
      points and the deferral in the proposal or pull request.
11. **MCP projection and commands** — `crates/koru-mcp-server/src/lib.rs`,
    [`mcp/README.md`](../../crates/koru-mcp-server/MCP.md#extending-the-model-through-mcp)
    - Reads must expose the real stable kind token; never degrade an unknown
      block to a paragraph.
    - Decide explicitly whether the kind belongs in writable `InsertKind`, and
      whether it is meaningful in both `EditOperation` and `InitialOperation`.
    - Keep model behavior in `WorkspaceDoc`; MCP translates semantic input and
      does not construct Loro state itself.
    - Update and pin the fully inlined tool schema when the kind is writable.
12. **Tests** — beside the module each layer lives in (per `AGENTS.md`), plus a
    convergence case in `crates/koru-core/tests/convergence.rs`.
13. **Docs** — update the model's doc comments,
    [MCP contract](../../crates/koru-mcp-server/MCP.md),
    [user guide](../user/USERGUIDE.md), and any affected
    [generated references](../../reference/README.md). Explain storage changes
    and deferred behavior in the pull request so a maintainer can also update
    the internal data model, ADRs, and operational notes.

## Decisions to make up front

- **Kind vs. block property.** A genuinely different thing (no text, no
  children, distinct semantics) is a `Kind`. A presentation variant of prose
  (heading, alignment) is a `NodeMeta` property. See
  ADR-44.
- **Where it is offered.** Document-only, or in the outline too? Gate with the
  `*_enabled` flag rather than a code path.
- **Root-only kinds.** `File` is the precedent: it is an opaque Store document
  root and never body content. Decide whether the constructor is reachable only
  through a manifest/import seam rather than adding editor input/render paths.
- **External round-trip.** Does it map to a Word / Google Docs block (`<hr>`,
  `<img>`, `<p>`)? If so, decide whether the writer (which does not exist yet)
  is in scope or deferred.
