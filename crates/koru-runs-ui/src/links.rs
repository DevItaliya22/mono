// SPDX-License-Identifier: MPL-2.0

//! **The one thread that talks to the gateway, and the two channels either
//! side of it.**
//!
//! Every call this surface makes to `/v1/runs` is blocking — a named pipe,
//! an HTTP round trip, a worker that may be starting — so **none of them
//! may happen inside a frame**. A [`Command`] goes down, an [`Answer`]
//! comes back, and the surface picks answers up on whatever cadence it
//! already has. Nothing here draws and nothing above here connects.
//!
//! **Connecting is the worker's first job, not the constructor's.** Opening
//! a run view must not wait on a gateway that is still publishing.
//!
//! **The stream is a signal, not a feed.** `runs().stream` tells this
//! thread that something changed; the thread then re-reads the run. That
//! is one shape rather than two — a header whose economics, results and
//! rating move is read the same way as a timeline that grew — and it costs
//! one round trip per burst of change rather than one per entry, because
//! every signal waiting in the channel is drained before the read.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use koru_api_client::Client;
use koru_delegate_api::wire::{Failure, RateRunRequest, RunStreamItem, RunView};
use koru_delegate_core::{ActorRef, Rating, RatingNote, RunId, WorkspaceId};
use koru_markdown_view::{Content, MarkdownDocument};

use crate::action::Operation;
use crate::state::{PreambleView, SaveFailure};

/// The scopes a run surface needs: it reads runs, and it ends, resumes and
/// rates the one it is showing.
pub const RUN_SCOPES: &[&str] = &["runs.read", "runs.write"];

/// How many entries of history one read asks for. A window, not a run
/// (§7.9): the person browses newest-first and asks for more.
pub const WINDOW: u32 = 50;

/// The most history one surface will hold, however many pages are asked
/// for. A bound on what a frame walks, not on what a run may contain.
pub const MAX_HELD: u32 = 2_000;

/// What the surface asks the worker to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Follow this run. Replaces whatever was being followed.
    Open {
        run: RunId,
        workspace: Option<WorkspaceId>,
    },
    /// Read it again now.
    Refresh,
    /// Hold this many of the newest entries from here on.
    Window {
        entries: u32,
    },
    /// `POST /v1/runs/{run}/cancel` — D36's irreversible half.
    EndRun,
    Resume,
    Rate {
        rating: Rating,
        note: Option<RatingNote>,
        rated_by: ActorRef,
    },
    /// Read the delegated-agent instructions as they stand now. A file
    /// read, which is why it is a command and not a getter.
    ReadPreamble,
    /// Write edited instructions back to the file they were resolved from.
    /// A file write, on the same thread as the read — and it re-resolves
    /// first, so a copy that stopped being in force while the tab was open
    /// is never written over.
    SavePreamble {
        text: String,
    },
    /// Stop following anything.
    Close,
}

/// What the worker answers with.
#[derive(Debug)]
pub enum Answer {
    /// The gateway would not answer, or started answering again.
    Reachable(bool),
    /// A whole read of the run this surface is following.
    Read {
        run: RunId,
        workspace: Option<WorkspaceId>,
        view: Box<RunView>,
        /// The folder the answer named, looked at **here** rather than in a
        /// frame: whether it is still on this disk is a question only a
        /// thread that may touch disk can ask, and the surface has to know
        /// before it offers to open it.
        folder: Option<crate::state::RunFolder>,
    },
    /// An operation was refused, by its stable code.
    Refused { operation: Operation, code: String },
    /// The delegated-agent instructions, freshly resolved.
    Preamble(PreambleView),
    /// A save landed, with the instructions as they now stand — or it did
    /// not, and nothing was changed.
    PreambleSaved(Box<Result<PreambleView, SaveFailure>>),
}

/// The link to the gateway, or the inert one a test drives.
pub struct RunLinks {
    commands: Option<Sender<Command>>,
    answers: Receiver<Answer>,
}

impl RunLinks {
    /// A link with no worker behind it: every command is dropped and no
    /// answer ever arrives. What a test and a headless host hold.
    pub fn inert() -> Self {
        let (_, answers) = channel();
        Self {
            commands: None,
            answers,
        }
    }

    /// Open a worker for this build's gateway. Returns immediately; the
    /// connection is the worker's first job.
    pub fn open(segment: &str, root: Option<std::path::PathBuf>) -> Self {
        let (commands, inbox) = channel::<Command>();
        let (outbox, answers) = channel::<Answer>();
        let segment = segment.to_owned();
        let signals = commands.clone();
        let spawned = std::thread::Builder::new()
            .name("koru-run-view".into())
            .stack_size(512 * 1024)
            .spawn(move || Worker::new(segment, root, outbox, signals).run(inbox));
        match spawned {
            Ok(_) => Self {
                commands: Some(commands),
                answers,
            },
            Err(_) => Self::inert(),
        }
    }

    pub fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Everything the worker has said since the last look. Never blocks.
    pub fn drain(&self) -> Vec<Answer> {
        let mut answers = Vec::new();
        while let Ok(answer) = self.answers.try_recv() {
            answers.push(answer);
        }
        answers
    }

    /// Whether a worker is behind this link at all.
    pub fn is_live(&self) -> bool {
        self.commands.is_some()
    }

    /// **A link with a script instead of a gateway.** The commands the
    /// surface sends arrive on the receiver and the answers a caller sends
    /// arrive at the surface, so a whole run can be driven with no
    /// process, no socket and no disk. The suite uses it; so would a host
    /// that reached runs some other way.
    pub fn scripted() -> (Self, Scripted) {
        let (commands, inbox) = channel::<Command>();
        let (outbox, answers) = channel::<Answer>();
        (
            Self {
                commands: Some(commands),
                answers,
            },
            Scripted {
                commands: inbox,
                answers: outbox,
            },
        )
    }
}

/// The other end of a [`RunLinks::scripted`] pair.
pub struct Scripted {
    commands: Receiver<Command>,
    answers: Sender<Answer>,
}

impl Scripted {
    /// Everything the surface has asked for since the last look.
    pub fn commands(&self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            out.push(command);
        }
        out
    }

    pub fn answer(&self, answer: Answer) {
        let _ = self.answers.send(answer);
    }
}

impl Default for RunLinks {
    fn default() -> Self {
        Self::inert()
    }
}

/// The installation's *data* directory — where a release puts `docs/`.
///
/// Beside the executables on Windows and Android, and the bundle's
/// `Resources/` on macOS, where codesign's seal keeps data out of
/// `Contents/MacOS/` (`koru_update::DATA_PREFIX`). The delegate host
/// resolves the preamble from exactly this directory, and the tab shows
/// what a run would be given, so the two must agree. A development build
/// needs no branch here: `resolve` reaches the repository's own copy when
/// there is no `docs/` beside the executable.
fn installation_dir() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = exe_dir.parent()
            && contents.join("Resources").is_dir()
        {
            return contents.join("Resources");
        }
    }
    exe_dir
}

/// The instructions as they stand, with the document already built — the
/// parse is proportional to the file, so it belongs on this thread.
fn resolved_preamble() -> PreambleView {
    let resolved = koru_delegate_preamble::resolve(&installation_dir());
    let content = match MarkdownDocument::of(&resolved.text) {
        Ok(document) => Content::Document(Box::new(document)),
        Err(_) => Content::Unreadable,
    };
    PreambleView {
        source: resolved.source,
        notice: resolved.notice,
        savable: resolved.path.is_some(),
        content,
    }
}

/// How long the worker waits for a command before looking at its own
/// state. It does no work on the timeout — it exists so a `Close` while a
/// stream is open is noticed.
const IDLE: Duration = Duration::from_millis(500);

struct Worker {
    segment: String,
    root: Option<std::path::PathBuf>,
    client: Option<Client>,
    reachable: bool,
    out: Sender<Answer>,
    /// The worker's own sender, handed to the stream so a change wakes it.
    signals: Sender<Command>,
    following: Option<Following>,
}

struct Following {
    run: RunId,
    workspace: Option<WorkspaceId>,
    entries: u32,
    total: u64,
    /// Dropped when the run changes, which stops the reader.
    subscription: Option<koru_api_client::RunSubscription>,
}

impl Worker {
    fn new(
        segment: String,
        root: Option<std::path::PathBuf>,
        out: Sender<Answer>,
        signals: Sender<Command>,
    ) -> Self {
        Self {
            segment,
            root,
            client: None,
            reachable: false,
            out,
            signals,
            following: None,
        }
    }

    fn run(mut self, inbox: Receiver<Command>) {
        loop {
            let Ok(first) = inbox.recv_timeout(IDLE) else {
                match inbox.try_recv() {
                    Err(TryRecvError::Disconnected) => return,
                    _ => continue,
                }
            };
            // Drain the burst: a run that wrote twenty entries in a second
            // is read once, not twenty times.
            let mut batch = vec![first];
            loop {
                match inbox.try_recv() {
                    Ok(command) => batch.push(command),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let mut refresh = false;
            for command in batch {
                match command {
                    Command::Open { run, workspace } => {
                        self.follow(run, workspace);
                        refresh = true;
                    }
                    Command::Refresh => refresh = true,
                    Command::Window { entries } => {
                        if let Some(following) = &mut self.following {
                            following.entries = entries.clamp(1, MAX_HELD);
                            refresh = true;
                        }
                    }
                    Command::EndRun => refresh |= self.end_run(),
                    Command::Resume => refresh |= self.resume(),
                    Command::Rate {
                        rating,
                        note,
                        rated_by,
                    } => refresh |= self.rate(rating, note, rated_by),
                    Command::ReadPreamble => self.read_preamble(),
                    Command::SavePreamble { text } => self.save_preamble(&text),
                    Command::Close => {
                        self.following = None;
                    }
                }
            }
            if refresh {
                self.read();
            }
        }
    }

    /// The gateway session, opened on first need and kept. A failure is
    /// news the surface shows, and the next command tries again.
    fn client(&mut self) -> Option<&Client> {
        if self.client.is_none() {
            let opened = match &self.root {
                Some(root) => Client::connect_at_with_scopes(&self.segment, root, RUN_SCOPES),
                None => Client::connect_with_scopes(&self.segment, RUN_SCOPES),
            };
            match opened {
                Ok(client) => {
                    self.client = Some(client);
                    self.say_reachable(true);
                }
                Err(_) => {
                    self.say_reachable(false);
                    return None;
                }
            }
        }
        self.client.as_ref()
    }

    fn say_reachable(&mut self, reachable: bool) {
        if self.reachable != reachable {
            self.reachable = reachable;
            let _ = self.out.send(Answer::Reachable(reachable));
        }
    }

    /// A refusal the gateway named. A transport failure drops the client so
    /// the next command reconnects — a worker restart is invisible to the
    /// call after it.
    fn refused(&mut self, operation: Operation, failure: Failure) {
        if failure.code == "gateway_unavailable" || failure.code == "delegate_unavailable" {
            self.client = None;
            self.say_reachable(false);
        }
        let _ = self.out.send(Answer::Refused {
            operation,
            code: failure.code,
        });
    }

    fn follow(&mut self, run: RunId, workspace: Option<WorkspaceId>) {
        self.following = Some(Following {
            run,
            workspace,
            entries: WINDOW,
            total: 0,
            subscription: None,
        });
    }

    /// Read the run and hand the whole answer up.
    ///
    /// The window is the newest `entries`, which the API indexes from the
    /// oldest — so the first read of a long run corrects itself once, from
    /// the `total` the answer carries, and every read after it starts in
    /// the right place.
    fn read(&mut self) {
        let Some(Following {
            run,
            workspace,
            entries,
            total,
            ..
        }) = self.following.as_ref().map(|f| Following {
            run: f.run,
            workspace: f.workspace,
            entries: f.entries,
            total: f.total,
            subscription: None,
        })
        else {
            return;
        };
        let mut believed = total;
        for _ in 0..2 {
            let from = believed.saturating_sub(u64::from(entries));
            let Some(client) = self.client() else { return };
            match client.runs().run(run, workspace, from, entries) {
                Ok(view) => {
                    let corrected = view.timeline.total;
                    if let Some(following) = &mut self.following {
                        following.total = corrected;
                    }
                    // The window landed where the run actually ends: hand
                    // it up. Otherwise ask once more from the right place.
                    if corrected == believed || corrected <= u64::from(entries) {
                        self.say_reachable(true);
                        let folder =
                            crate::state::RunFolder::look(view.execution.run_workspace.as_deref());

                        let _ = self.out.send(Answer::Read {
                            run,
                            workspace,
                            view: Box::new(view),
                            folder,
                        });
                        self.watch(run, workspace);
                        return;
                    }
                    believed = corrected;
                }
                Err(failure) => {
                    self.refused(Operation::Read, failure);
                    return;
                }
            }
        }
        // Two reads and the run is still growing under us: hand up what
        // the second one said rather than looping against a live run.
        let from = believed.saturating_sub(u64::from(entries));
        let Some(client) = self.client() else { return };
        match client.runs().run(run, workspace, from, entries) {
            Ok(view) => {
                self.say_reachable(true);
                let folder = crate::state::RunFolder::look(view.execution.run_workspace.as_deref());

                let _ = self.out.send(Answer::Read {
                    run,
                    workspace,
                    view: Box::new(view),
                    folder,
                });
                self.watch(run, workspace);
            }
            Err(failure) => self.refused(Operation::Read, failure),
        }
    }

    /// Follow the run's stream, once, purely as a change signal.
    fn watch(&mut self, run: RunId, workspace: Option<WorkspaceId>) {
        if self
            .following
            .as_ref()
            .is_some_and(|f| f.subscription.is_some())
        {
            return;
        }
        let signals = self.signals.clone();
        let Some(client) = self.client() else { return };
        let subscription = client.runs().stream(run, workspace, None, move |item| {
            // Every kind of item means the same thing here — *look again* —
            // so the worker reads the run rather than trying to apply an
            // entry to a header that may have moved for other reasons.
            if matches!(
                item,
                Ok(RunStreamItem::Entry { .. }) | Ok(RunStreamItem::Status { .. })
            ) {
                let _ = signals.send(Command::Refresh);
            }
        });
        if let (Ok(subscription), Some(following)) = (subscription, self.following.as_mut()) {
            following.subscription = Some(subscription);
        }
    }

    /// **The instructions as they stand now**, from the same place the
    /// delegate worker reads them at the start of every run — so what the
    /// tab shows is what the next run would be given, edited file and all.
    /// A disk read, on this thread, which is the whole reason the surface
    /// asks for it rather than reading it itself.
    fn read_preamble(&mut self) {
        let _ = self.out.send(Answer::Preamble(resolved_preamble()));
    }

    /// **Write the file that is in force, and only then.**
    ///
    /// The path is re-resolved rather than remembered: between the tab
    /// opening and Save being pressed, the copy beside the executables may
    /// have been deleted or broken, and the built-in copy is what a run
    /// would now get. Writing a remembered path then would put a file back
    /// that nobody asked to restore.
    ///
    /// What is written is the *document's* Markdown rather than the
    /// composer's text, so the file on disk is what the tab was drawing.
    fn save_preamble(&mut self, text: &str) {
        let answer = self.write_preamble(text);
        let _ = self.out.send(Answer::PreambleSaved(Box::new(answer)));
    }

    fn write_preamble(&mut self, text: &str) -> Result<PreambleView, SaveFailure> {
        let document = MarkdownDocument::of(text).map_err(|_| SaveFailure::NotADocument)?;
        let resolved = koru_delegate_preamble::resolve(&installation_dir());
        let path = resolved.path.ok_or(SaveFailure::NoFile)?;
        koru_delegate_preamble::write(&path, document.markdown()).map_err(SaveFailure::Write)?;
        Ok(resolved_preamble())
    }

    fn end_run(&mut self) -> bool {
        let Some(run) = self.following.as_ref().map(|f| f.run) else {
            return false;
        };
        let Some(client) = self.client() else {
            return false;
        };
        match client.runs().cancel(run) {
            Ok(_) => true,
            Err(failure) => {
                self.refused(Operation::EndRun, failure);
                false
            }
        }
    }

    fn resume(&mut self) -> bool {
        let Some(run) = self.following.as_ref().map(|f| f.run) else {
            return false;
        };
        let Some(client) = self.client() else {
            return false;
        };
        match client.runs().resume(run) {
            Ok(_) => true,
            Err(failure) => {
                self.refused(Operation::Resume, failure);
                false
            }
        }
    }

    fn rate(&mut self, rating: Rating, note: Option<RatingNote>, rated_by: ActorRef) -> bool {
        let Some(run) = self.following.as_ref().map(|f| f.run) else {
            return false;
        };
        let request = RateRunRequest {
            rating,
            note,
            rated_by,
        };
        let Some(client) = self.client() else {
            return false;
        };
        match client.runs().rate(run, &request) {
            Ok(_) => true,
            Err(failure) => {
                self.refused(Operation::Rate, failure);
                false
            }
        }
    }
}
