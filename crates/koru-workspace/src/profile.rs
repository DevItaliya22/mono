// SPDX-License-Identifier: MPL-2.0

//! Persistence and peer sync for the profile document.
//!
//! The same shape as [`crate::session::Session`], one document up: local changes
//! are exported and appended to this instance's update file, and other
//! instances' files are polled and imported. Two windows on one profile folder
//! therefore agree about the workspace list without coordinating — window A
//! makes a workspace and window B's switcher grows a row.
//!
//! **Deliberately not generalised with `Session`.** The storage half already is
//! ([`koru_docstore::DocStore`]), which is where the risk lives — one recovery
//! path, one quarantine, one framed append. What is left here is *timing
//! policy*, and the two documents want different policies: a profile is written
//! when a workspace is added or renamed, which is a handful of times a session,
//! against a workspace that is written every time somebody stops typing. Sharing
//! the constants would mean polling a file that almost never changes at the rate
//! of a file that changes constantly.

use std::path::Path;
use std::time::{Duration, Instant};

use koru_core::ProfileId;
use koru_core::profile::ProfileDoc;
use koru_core::schema::Compatibility;
use koru_docstore::{Opened, ProfileStore};

use crate::session::{OpenError, Report};

/// How often to look for other instances' changes to the workspace list.
///
/// Far slower than the workspace's 300 ms: a new workspace appearing in the
/// switcher a couple of seconds late is nobody's problem, and this is a file
/// that changes a few times a session rather than a few times a second.
const POLL: Duration = Duration::from_secs(2);

/// How long after a change to compact to a snapshot. Short, because the changes
/// are discrete — adding a workspace, renaming one — rather than a stream of
/// keystrokes there is any point coalescing.
const IDLE_SAVE: Duration = Duration::from_millis(400);

pub struct ProfileSession {
    store: ProfileStore,
    pub peer: String,
    /// Set when the profile was written by a build newer than this one. An
    /// independent gate from the workspace's: a profile this build cannot write
    /// means the *list* is read-only, and each workspace still opens under its
    /// own check.
    read_only: Option<String>,
    last_poll: Instant,
    /// When the last local change landed, for the idle save.
    last_edit: Option<Instant>,
    unsent: bool,
}

impl ProfileSession {
    /// Open the profile document in `dir`, loading whatever is already there.
    ///
    /// Returns [`Opened`] alongside the usual [`Report`], because the caller has
    /// to know whether this was a genuine first run: that is the one condition
    /// under which a profile is seeded, and getting it wrong means seeding over
    /// somebody's workspaces.
    pub fn open(
        dir: &Path,
        id: ProfileId,
        name: &str,
        email: Option<&str>,
        peer: &str,
    ) -> Result<(Self, ProfileDoc, Report, Opened), OpenError> {
        let mut store =
            ProfileStore::open_at(dir, peer).map_err(|e| OpenError::Storage(e.to_string()))?;
        let (doc, opened) = store
            .load(id, name, email)
            .map_err(|e| OpenError::Unreadable(e.to_string()))?;

        let mut report = Report {
            damaged: store.damaged().to_vec(),
            ..Default::default()
        };
        match &opened {
            Opened::Fresh => report.fresh = true,
            Opened::Normal => {}
            Opened::Recovered(r) => report.recovery = Some(r.clone()),
        }
        if let Compatibility::TooNew { .. } = doc.compatibility() {
            // Named, because "this workspace was written by a newer version"
            // shown about a *profile* sends somebody looking at the wrong file.
            report.read_only = Some(doc.compatibility().describe("profile"));
        }

        let session = Self {
            store,
            peer: peer.to_string(),
            read_only: report.read_only.clone(),
            last_poll: Instant::now(),
            last_edit: None,
            unsent: false,
        };
        Ok((session, doc, report, opened))
    }

    pub fn read_only(&self) -> Option<&str> {
        self.read_only.as_deref()
    }

    pub fn root(&self) -> &Path {
        self.store.root()
    }

    /// Note that the profile changed locally.
    pub fn touch(&mut self) {
        self.unsent = true;
        self.last_edit = Some(Instant::now());
    }

    /// Write and publish **now**, and say whether it worked.
    ///
    /// The seeding path uses this: a profile whose first workspace has not
    /// reached disk before the window opens is a profile that seeds itself again
    /// on the next launch, and a second instance opening the same folder would
    /// find `Opened::Fresh` and seed its own. Unlike [`save`], a failure here is
    /// something the caller must be able to refuse to continue past.
    ///
    /// [`save`]: Self::save
    pub fn flush(&mut self, doc: &ProfileDoc) -> Result<(), String> {
        if self.read_only.is_some() {
            return Ok(());
        }
        self.store.publish_new(doc).map_err(|e| e.to_string())?;
        self.store.save_snapshot(doc).map_err(|e| e.to_string())?;
        self.unsent = false;
        self.last_edit = None;
        Ok(())
    }

    /// One turn of the loop. Returns true if the document changed underneath the
    /// caller, which means the switcher's list has to be rebuilt.
    pub fn tick(&mut self, doc: &mut ProfileDoc) -> bool {
        let now = Instant::now();
        let mut changed = false;

        if self.unsent {
            self.publish(doc);
        }
        if now.duration_since(self.last_poll) >= POLL {
            self.last_poll = now;
            match self.store.sync_in(doc) {
                Ok(c) => changed = c,
                Err(e) => eprintln!("warning: could not read profile updates ({e})"),
            }
        }
        if self
            .last_edit
            .is_some_and(|t| now.duration_since(t) >= IDLE_SAVE)
        {
            self.last_edit = None;
            self.save(doc);
        }
        changed
    }

    /// Compact to a snapshot. Also called on exit.
    pub fn save(&mut self, doc: &ProfileDoc) {
        if self.read_only.is_some() {
            return;
        }
        if self.unsent {
            self.publish(doc);
        }
        if let Err(e) = self.store.save_snapshot(doc) {
            eprintln!("warning: could not write the profile snapshot ({e})");
        }
    }

    fn publish(&mut self, doc: &ProfileDoc) {
        if self.read_only.is_some() {
            return;
        }
        match self.store.publish_new(doc) {
            Ok(_) => self.unsent = false,
            Err(e) => eprintln!("warning: could not publish profile updates ({e})"),
        }
    }
}
