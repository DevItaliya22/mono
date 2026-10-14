# Working principles

Rules that are easy to violate under deadline pressure and expensive to rediscover.
This page explains how we choose implementations. Follow the
[proposal process](../../GOVERNANCE.md#proposals-and-decisions) for decisions
that need maintainer agreement.

## Web controls use the hand cursor and keep their shape while working

On the website, links and enabled buttons show `cursor: pointer`, including
download pills implemented as buttons. Apply this in the site's shared CSS so
new controls inherit it. The native desktop app retains its arrow cursor rule.
Checking a session must preserve a download pill's icon, label, version, size,
and dimensions. Disable duplicate clicks and expose busy state accessibly;
replacing the contents with progress text changes the layout unnecessarily.

## Start from the outcome, not the proposed mechanism

A request often arrives carrying both a desired outcome and a suggested means.
Treat the outcome as the requirement. Re-check the mechanism against the model,
the code already present, and the longer product direction before turning it
into architecture. Keep the useful parts; replace the rest without preserving
complexity merely because it appeared in the first proposal.

### Worked example — document history was not Git

The initial history proposal started from canonical Markdown files and Git-style
snapshots. The actual outcomes were simpler and broader: people and agents need
durable named states, meaningful comparisons, and a safe way to return to an
older state. Koru already has the authoritative typed document and exact CRDT
version identities. Projecting that state to Markdown first would have discarded
node identity, marks, metadata and non-text content, then made Git an accidental
runtime dependency.

The resulting design stores immutable Loro snapshots, compares the typed node
model, and restores by creating new forward-moving CRDT operations. A standard
MCP server exposes those capabilities directly to local AI clients. Git or a
text projection can still be added later as an adapter, but neither defines what
history means.

## Prefer the capability we already have

If a dependency we already link (Loro, parley, winit, …) supports the behaviour
we need, **use it**. Do not re-implement a parallel version “just for this call
site.” If the library’s API is awkward, fix our *grouping / wiring* so the
library path works — do not invent a second history, clipboard, or layout engine
beside it.

**If nothing we already use covers it:** look for a focused library before
rolling our own. Rolling our own needs a short written reason (missing
capability, size/latency, wrong license, or a hard product constraint the library
cannot meet) — in an ADR or a note here, not only in a commit message.

### Worked example — undo is Loro’s stack pointer

Image-paste undo once grew `last_image_pastes` / compensatory deletes beside
Loro’s `UndoManager`. That second stack polluted redo: after paste ×3 → undo ×3
→ redo ×3, one more redo replayed a compensatory delete and removed content.
The product model users expect is one history with a stack pointer — undo moves
back, redo moves forward and **stops at head**, a new edit after undo truncates
forward history. That **is** Loro when we stop inventing ops. The fix was
correct `checkpoint` / `begin_group` around paste so Loro’s top step *is* the
paste, not a cleverer app-side stack.

## Prefer SQL types over encoded strings

A database column uses the most specific native SQL type that represents its
domain. UUIDs are UUIDs, booleans are booleans, relationships are foreign keys,
and a closed application enum is a database enum—not `TEXT`, an integer code,
or a comment that asks every writer to agree. A protocol-defined epoch value
may truthfully remain an integer; the unit must be part of the modeled contract.
The database is one of the writers' shared authorities; invalid states should
be unrepresentable there as well as in Rust.

`TEXT` is for genuinely open text: user-authored values, opaque external
identifiers, locale/version strings whose vocabulary evolves independently,
and other domains for which the database has no truthful narrower type. A
cross-column invariant still belongs in a constraint or transaction; choosing
a native scalar type does not replace relational validation.

This choice includes its migration cost. Adding a value to a released native
enum requires a forward migration before a writer emits it. Removing, renaming,
or reordering values may require a replacement type and table conversion. That
cost is preferable to silently accepting a value the domain cannot decode. See
ADR-308.

## Instrument before guessing

When a bug has more than one plausible cause, put **logs** (or a failing e2e)
in place **first**, reproduce, read the data, **then** choose the fix. Do not
ship a stack of speculative “hard fixes” and see what sticks — that is how dual
undo stacks and mislabeled notices land.

Prefer:

- one env flag (`KORU_*_DEBUG`)
- structured one-line events on stderr
- a short decision table from the log patterns (in the plan or the PR)

### Worked example — silent Ctrl+Z after image paste

Plausible causes: key dropped because `!active`, or Loro stack empty after
paste, or Loro declined a step. Those need different fixes. `KORU_UNDO_DEBUG=1`
logs paste `can_undo`, key drops, and undo before/after — one repro classifies
the branch before any product change.

## Every component can be asked what it is doing

The principle above is about reacting to a bug. This one is standing: **a
component nobody can inspect is a component nobody can trust**, and by the time
somebody wants to look, it is too late to add the seam. Building the way to look
is part of building the thing — not a follow-up, and not something to be talked
into afterwards.

Three questions every component should be able to answer about itself:

- **What is it doing now?** Queue depth, what is pending, what failed and why,
  whether a long job is running and how much is left.
- **What did that cost?** A number somebody can reproduce, not an impression.
- **What is its internal state?** The derived thing it holds, in a form a person
  can read.

Four rules that make those answers worth having:

- **Free when off, cheap enough to leave on.** `KORU_FRAME_STATS` and
  `KORU_STARTUP_STATS` cost nothing when unset; the store's counters are a
  handful of integers and are always live. A diagnostic somebody has to
  *enable and rebuild* for is one that is never there during the incident.
- **A diagnostic must not depend on the health of what it diagnoses.**
  `koru-store-server projection` reads `projection.db` directly rather than asking the
  worker, because a Store that will not answer is exactly when its read model is
  worth looking at. `koru-agent status` inspects the public process boundary the
  same way: its `gateway:` line comes from dialling the gateway's own rendezvous
  with `koru_api_discovery::probe_at` rather than from asking the Agent about its
  child, and it is printed whether or not an agent answered. A supervisor whose
  supervision has wedged would report from a cached record and a supervisor that
  is not running would report nothing — which is exactly when somebody wants to
  know whether a gateway is still sitting on the rendezvous.
- **Measure ratios, not wall clocks.** `apps/koru-store-server/tests/perf.rs` asserts
  that editing one document costs work proportional to *that document* — a
  budget in milliseconds passes on a fast machine and fails on a busy one, so
  it gets relaxed until it asserts nothing.
- **Privacy-safe by construction.** A diagnostic record carries ids, counts and
  stages — never a name, a title, a document's text or a search query. See
  `[Koru][search-open]` in the search diagnostics. It names every
  identifier in a cross-profile open and none of the content.
- **Report news, not activity.** Everything fails eventually and the run that
  fails is the one nobody prepared for, so a component has to leave enough
  behind to explain itself — and that is not the same as saying something on
  every pass. A line that appears whether or not anything happened is a line
  people learn to scroll past, and a log full of them is one nobody reads and
  nobody can search. The test is whether a reader could have predicted the line
  without reading it: if so, it does not earn its place.

  Which gives the split:

  - **Unconditional** — rare, consequential, and about *their* data: work
    recovered from the outbox, a document that could not be applied, a
    recovery on open, a store that will not start. A switch here means the
    diagnostic was off during the incident.
  - **Behind a `KORU_*` flag** — anything routine or per-event: routing
    decisions (`KORU_SEARCH_DEBUG`), input (`KORU_INPUT_DEBUG`), the endpoint
    transcript (`KORU_IPC_DEBUG`), the sync conversation (`KORU_SYNC_DEBUG`),
    metrics and timings (`KORU_FRAME_STATS`, `KORU_STARTUP_STATS`,
    `KORU_JANK`). These are
    the ones to be generous with, because generosity costs nothing when the
    flag is off — a `OnceLock<bool>` and a branch — and a flag that exists is a
    question somebody can answer in one run instead of one afternoon.
  - **Never** — a line per frame, per keystroke or per tick. It changes what it
    measures and buries the two lines that mattered.

  A threshold is the same rule applied to a number: `KORU_JANK` reports stalls
  over a floor rather than every event, and `KORU_JANK_MS` moves the floor once
  the first log has been read.

### Worked examples

**A live room transcript existed because looking was impossible.** Two clients
that disagreed, an edit that seemed not to arrive, a room nobody was subscribed
to — each was a guess until the room could be observed live. The standalone
command was retired with the standalone Store; the regression it exposed is now
pinned by `apps/koru-store-server/tests/concurrency.rs`.

**A read model with no way to read it.** The projection layer shipped correct
and observable in the aggregate — counters on `GET /v1/debug/projection` — and
still failed this principle, because the only way to see *what it actually held*
was a hand-written SQLite query. `koru-store-server projection [<id>]` is the fix, and
the lesson is that "there are counters" is not the same as "somebody can look".

**Three lines per document, all of them saying nothing happened.** The outbox
replay reported what it found, what it applied and what the store then
confirmed — correctly, and unconditionally, on the argument that recovered work
is the one thing a person is entitled to see accounted for. In practice the
ordinary case is a *duplicate*: a buffer is removed only once the store
acknowledges a version covering it, so after a reconnect the live link usually
delivers the work first and the file replays into `0 op(s) applied`. Every
document opened printed three lines, all of them predictable, and the one case
worth reading was buried in them. The argument was right and the trigger was
wrong: it reports when operations were recovered or something could not be
applied, and is silent otherwise.

**A stall watch nobody would have thought to switch on.** The stall watchdog
existed for a year behind `KORU_STARTUP_STATS` — a variable named for the first
two seconds of a launch — and watched the idle loop but not event dispatch. So
the thing being reported ("it freezes while I type") was the one thing not
instrumented, under a flag nobody reporting it would guess. `KORU_JANK` is the
same code with a name that matches the question. Within a day it found a
7.9-second freeze that had been in every session for months: three settings
files rewritten at caret-blink rate.

**26% of a CPU core, invisible.** `about_to_wait` walked the whole document on a
timer to find image references. Nothing was slow enough to notice and nothing
reported it; it took a profile to find and one line to fix. What made it
findable was `KORU_FRAME_STATS` existing before anybody suspected a problem.

## A thread that answers a person never waits on the world

Every process has one thread whose job is to be *ready* — the window's frame
loop, the agent's event loop, anything that dispatches a click. That thread
does no I/O whose latency it does not own. Not "does it fast": **does not do
it.** The cost of a disk or network call is a property of the machine's state
at that moment, not of the call site, and the call site is where every
estimate is made.

- **`fsync` costs what the volume owes, not what you wrote.** Flushing a
  one-byte file waits behind every dirty page ahead of it — minutes, on a
  spinning disk a build just filled. `sync_all` belongs where durability *is
  the meaning of the operation* (the store's `Ack`, the updater's staging) and
  nowhere else. It is never a side effect of answering a question.
- **Do not prove more than the question asks.** "May the updater write here"
  is answered by creating and deleting a file; the fsync that rode along
  proved durability nothing had asked about, and that excess proof was the
  entire cost. When a check is expensive, first ask what it is actually
  checking.
- **Ask once, cache, and re-verify at the moment of consequence.** An answer
  that cannot change mid-session is a field, not a call; the operation that
  acts on it does its own live check, which is where a stale answer should
  fail. (The same shape as ADR-114: verify with evidence at the point of no
  return, not with the cooperation of what came before.)
- **A guard runs off the thread it guards.** The handoff's 120 s no-progress
  abort ran on the loop it was meant to outlive, so a blocked loop silenced
  its own alarm. Likewise a stall watch that times a call can only speak
  *after* the call returns — a hang that ends in process death names nothing.
  The watchdog that can accuse a stuck thread lives on another one.
- **The instrument must not travel inside the thing that is failing.** The
  agent's stall watch shipped in the build the stall kept preventing from
  installing; the bug guarded its own diagnosis for five traced occurrences.
- **A live hang is evidence — capture it before it releases.** One minidump
  of the blocked process named the exact call
  (`NtFlushBuffersFile ← File::sync_all ← is_writable_install`) after five
  fully-logged occurrences had only narrowed the neighbourhood. Logs say
  *when* and *around what*; a stack says *what*.

### Worked example — the update consent that took 157 seconds

Clicking *Restart and install* hung for 70–157 s, six times across one day,
because `update_report` — built on the agent's event loop for every update
push — probed the installation directory with a create-and-fsync. The install
shares an SMR hard disk with release builds, so the fsync queued behind
gigabytes of write-back exactly when an update was being tested. Every
downstream symptom followed: consents queued behind the blocked loop, the
window's timeout accused a live agent of not answering, and twice the install
handoff itself starved and died. The fix deleted the fsync and made the loop
read a cached field. The window had
already learned the same rule twice ("nothing writes to disk on a timer",
"a cheap-looking getter may be a round trip to the compositor") and the
lesson had not crossed the process boundary.

## Local-first is a promise, not a default

Koru works with no account, no server and no network, and that is a property to
be defended rather than a stage we are at.

- **No account, no cloud, no third party is required to use Koru.** Creating a
  profile is offline. An account may be *attached* later; it is never a
  precondition, and a `ProfileId` is not an account id.
- **All canonical state is local.** A server, when one exists, is a replica and
  a relay — never the original.
- **A snapshot is a checkpoint, not the truth.** The truth is the checkpoint
  plus the updates above it. Anything that treats a snapshot as authoritative
  loses whatever arrived after it.
- **Compare with version vectors. Never with timestamps.** Clocks disagree,
  drift, and go backwards. A version vector says what a replica has actually
  seen; a timestamp says what a machine believed the time was. Use clocks for
  *display* and nothing else.
- **Frontiers name a state; version vectors compare states.** Reach for a
  frontier when forking or naming a point in history, and for a version vector
  when asking who is ahead.
- **Every workspace and document has a globally unique, immutable id.** Ids are
  client-minted v4 UUIDs, never derived from a path, a folder, a machine or a
  name.
- **Sync is transport-independent.** LAN, peer-to-peer, a synchronised folder
  and a cloud service all exchange the same Loro state. A transport is an
  adapter; it never changes what an operation means.
- **Never invent a CRDT wire protocol.** Use the standard Loro Protocol. A
  private extension makes Koru an app with a channel instead of a platform with
  an API — see ADR-75.

### Worked example — an acknowledgement that was nearly a lie

The Koru Store answers an edit with a protocol `Ack`. It would have been easy to
send it when the update was *received*, and easy again to open SQLite with
`synchronous = NORMAL` like the search index does. Either would have made the
acknowledgement mean "probably", and a client that stops retrying on a
"probably" loses work on a power cut. The store commits at `synchronous = FULL`
and acknowledges after the commit returns; the cost — one fsync per
transaction — is paid down by group-committing, which changes how often the disk
is touched and never what an acknowledgement means.

## Internationalization is structural, not a translation pass

These constrain what is *stored and returned*, which is why they belong here
rather than in a UI checklist. Retrofitting any of them means rewriting data.

- **UTF-8 and Unicode throughout** — store, API, CRDT state. Text used as a key
  is NFC-normalised; **content never is**. Nothing assumes one byte is one
  character, and nothing truncates by byte offset.
- **A stable identifier never derives from a name.** Names are translatable and
  renameable; identities are not. Directories are named by id, blobs by content
  hash.
- **Locale belongs to the person, not the content.** BCP 47 on the profile.
  A workspace written by an `ar-EG` profile is fully readable by a `ja-JP` one.
- **Canonical state depends on no tokenizer and no collation.** Both are
  language-specific and both change between library versions; a stored order
  that shifts when a dependency is upgraded is data loss wearing a costume.
- **Persist semantic values, never translated labels.** A column holding a
  display string means something different in another language.
- **Timestamps and numbers are structural** — epoch milliseconds UTC, numbers
  with an explicit unit or ISO 4217 code. A formatted string has already chosen
  a calendar and a locale on the reader's behalf.
- **A filename is metadata; identity is content.** Arbitrary Unicode is fine in
  a filename and never fine in a path component.
- **APIs return stable codes, not prose.** `{ code, message }` where `code` is
  branched on and `message` is untranslated developer text a client must not
  show to a person.
- **Bidirectional text is preserved, never interpreted.** Logical order is kept
  and control characters pass through byte-exactly. Direction is a rendering
  decision, and no directional assumption belongs in persisted semantics.

**Supported from the beginning:** English, Spanish, Brazilian Portuguese,
German, French, Arabic, Simplified Chinese and Japanese. Test fixtures carry all
eight — `koru_store_core::fixtures` — and every text path is asserted to round
trip byte-exactly, including a mixed left-to-right/right-to-left line.

### Worked example — a length limit that was a different product per language

A name limit of 200 *bytes* is 200 letters in English, about 66 in Japanese and
about 100 in Arabic. It reads as one rule and behaves as three. `validate_name`
counts characters, and the test that pins it stores 200 Japanese characters —
600 bytes — and asserts they are accepted.

## Everything that can be localized is localized

The section above constrains what is stored; this one constrains what is
*shown*. Koru's goal is extremely good localization — not "translated
eventually", but built so that an unlocalized surface is visibly a defect:

- **Every string a person reads goes through `koru-i18n`.** Adding one means
  adding a table row in all eight languages — the crate makes a missing
  translation a compile error, and there is deliberately no runtime fall-back
  to English inside the table, because that is how an English word ends up
  mid-sentence in Japanese.
- **Formatting is localization.** A timestamp, a relative time, a number are
  each *rendered* per language — date order (`08/20` against `20.08.` against
  `8月20日`), unit words, digit grouping. `koru_i18n::time` is where that
  rendering lives; formatting a user-visible time with a hard-coded pattern in
  shared code is a bug, not a default. Epoch→wall-clock conversion stays with
  the caller so the crate stays a dependency-free leaf.
- **The stored locale is the person's; the shown language is the nearest
  supported** (ADR-87). Formatting follows the shown language, storage never
  follows either — see the section above.
- **Existing English-only surfaces are debt, not precedent.** The desktop
  chrome's copy predates this principle; the localization pass it is owed is
  tracked as technical debt. New code must follow the current localization rule.

## Push mechanism down; keep domain behaviour with its domain

When two components need the same thing, extract **the mechanism** — the part
that is true of anything, spelled in the vocabulary of the platform rather than
of the product — and leave with the original everything that is knowledge about
what it is *for*.

Two failure modes sit either side of this, and both are easy to reach honestly:

- **Too little goes down**, and the second component writes the mechanism again.
  A second copy is not merely duplication; it is a place the next platform fix
  will not be applied, and you find out when only one of them is broken.
- **Too much goes down**, and the shared crate quietly becomes the first
  component wearing a general name. Everything now depends on it, so everything
  now depends on the thing it knows.

**And never create a generic bucket merely to make an extraction easier.** A
crate whose members' only shared property is *"they were extracted at the same
afternoon"* has no charter, so nothing can ever be refused entry to it. The test
is whether you can write one sentence saying what it is for that excludes
something. If you cannot, the pieces belong beside their own subjects, however
many places that turns out to be.

### Worked example — an endpoint that could not start a store

`koru-agent` needed a per-user named pipe. One existed, inside
`koru-store-discovery`: the name derived from the OS user and the channel, the
security descriptor, the stale-socket takeover, the frame codec. All general;
all reachable only by depending on a crate that also knows how to launch
`koru-store-server.exe`.

What went down to `crates/koru-ipc` was the endpoint, the transport, the framing
and both listeners, with the endpoint parameterized over a *service* name. What
stayed was the product launcher because "the binary beside this executable" and
"a nightly build may not serve `stable`" are facts about a product. The
The Store-specific launcher is gone; process supervision belongs to Agent.
`koru-ipc` cannot start a process, and that is the line.

The cost of not having done this earlier was already in the tree:
`profile_activation.rs`, a second hand-rolled `CreateNamedPipeW` accept loop
with a hex-encoded line format and no security descriptor at all — written
because the only listener available required a Tokio runtime a winit loop must
not have.

The rejected alternative was a `crates/koru-desktop` for the leftovers —
`DesktopPreferences` and two `KORU_*` debug writers. They have nothing to do
with each other, so each goes beside its own subject instead:
`DesktopPreferences` to `koru-window`, where a global shortcut and a Run-key
entry are the platform traps that crate is for; `inputdebug` to `koru-ui`,
beside key normalisation; `search_debug` to the search surface's own crate. It
is more places, and each of them is a place somebody would look.

## Retire state; do not erase it

Koru is a peer-to-peer system. A fact that one replica deletes is a fact the
others still hold, will still gossip, and may still act on. Once the row is
gone there is nothing left to say *why* it is gone, *when*, or *on whose
authority* — so the next replica to arrive with the old fact cannot be told it
is stale, and the person asking "how did we get here" has nothing to read.
Deleting is how the origin and history of events become unrecoverable.

So a persisted fact is retired by **marking** it, not by removing it:

- **A state, not an absence.** `revoked`, `deleted`, `removed_at` — a value a
  reader can see and compare, in the same row the fact lived in. A row that is
  gone and a row that never existed look identical; a marked row does not.
- **A revision, not a timestamp, orders it.** The mark carries what a replica
  needs to decide whether it is newer than what it holds (see "Local-first is a
  promise"). Clocks are for display.
- **Recovery data stays.** Retiring the owner does not cascade into the
  content; ADR-305 keeps every Workspace tombstone under a locally removed
  Profile for exactly this reason. Physical deletion of content is its own
  explicit operation with its own review.
- **Erase only what must not be readable.** A tombstone may scrub a name, a
  locale, an email — the parts that are *about the person* — and keep the ids,
  the state, and the revision that let the system stay consistent. Minimal is
  not the same as gone.

The cost is small and boring: a column, a filter on the ordinary read path,
and a rule that upserts respect the mark. The alternative is discovered later,
in the form of a device that cannot rejoin because the row saying it ever held
the content was deleted, or a "delete" that a background sync quietly undoes.

### Worked example — device rows on Profile deletion

The first draft of cross-device Profile deletion removed every hosted
device/Profile row for the Profile. Those rows are how a new device learns
which Profiles are available and from where, and how enrollment finds an
existing device that can approve a new one. With the rows gone, restoring the
Profile left no device the system considered a source, so nothing could ever
join it again — and nobody could tell which devices had held it. The table
already had a `revoked` state. Marking the rows kept the history, kept restore
possible, and erased nothing a person cares about, because those rows hold
only ids and a state.

## Windows and Android first; macOS and iOS catch up

Every feature is built and verified on Windows and Android first. macOS and
iOS receive it afterwards through the mac branch. Maintainers track the
remaining Apple work separately. This keeps the initial implementation and
its platform evidence available when porting the feature.

What follows from it:

- **A roadmap's exit gate is Windows plus Android.** macOS and iOS parity is
  real work and is planned, but it is planned on the mac branch as catch-up,
  and a roadmap does not wait for Apple hardware to close.
- **Shared code carries the seam, not the port.** A change lands with the
  platform boundary the Apple side will need — a `cfg`, a port trait, a
  platform adapter — so that catching up is filling in an implementation, not
  redesigning. A Windows-only mechanism hard-coded where a seam belongs is a
  finding on the source branch, not a mac-side patch.
- **Every gap is written down when it is created.** The change that opens a
  gap describes what needs Apple work and what is deliberately deferred in the
  pull request. Maintainers carry that information into the platform tracker.
- **Live evidence is per platform.** Windows and Android rows in a test plan
  are the roadmap's; macOS and iOS rows are the catch-up's. Neither is
  inferred from the other.

Decided by the maintainers on 2026-09-06 while splitting the account roadmaps.
