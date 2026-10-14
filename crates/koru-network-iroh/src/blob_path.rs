// SPDX-License-Identifier: MPL-2.0

//! **D246's byte gate**: a blob byte may cross a replication connection only
//! while that connection's *selected* path is an IP path.
//!
//! This is deliberately **not** [`crate::path_kind`], and the difference is the
//! whole point of the file existing. `path_kind` answers `Direct` whenever any
//! IP path is *open*, whether or not a packet has ever gone over it — which is
//! the right answer for connection reporting and the wrong one for admitting a
//! person's bytes. `Path::is_selected()` is iroh's own words for *"currently
//! selected for application data transmission"*: the packets, not the
//! possibilities.
//!
//! ## What this promises, and what it does not
//!
//! - **Does:** no blob chunk is requested or served while the connection's
//!   selected path is a relay, and a selection change stops an in-flight
//!   transfer within one snapshot of [`iroh::endpoint::Connection::paths_stream`].
//! - **Does not:** packets already in flight when the selection switches may
//!   cross the relay. The exposure is bounded by the in-flight window of a
//!   single bounded chunk, not by the size of the file — and that bound is
//!   **measured at acceptance** against the relay's own byte counters, not
//!   asserted here.
//!
//! Nothing in this file claims more than that.

use futures_util::StreamExt as _;

/// The gate itself.
///
/// A snapshot, which is all `paths()` can be — the watcher below is what makes
/// a snapshot enough.
pub(crate) fn selected_path_is_ip(connection: &iroh::endpoint::Connection) -> bool {
    connection
        .paths()
        .iter()
        .any(|path| path.is_selected() && path.is_ip())
}

/// Resolves as soon as the selected path stops being an IP path.
///
/// Held for the life of one in-flight transfer and raced against it, so *"the
/// remaining chunks are not requested or served"* is true within one snapshot
/// rather than at the end of the current chunk.
///
/// The `Connection` is taken **by value** because `paths_stream()` borrows it
/// and the borrow cannot cross a task boundary; a clone is cheap and is what
/// iroh's own documentation says to do. A stream that ends means the connection
/// is gone, which is also a reason to stop.
pub(crate) async fn until_selected_path_leaves_ip(connection: iroh::endpoint::Connection) {
    let mut paths = connection.paths_stream();
    while let Some(snapshot) = paths.next().await {
        if !snapshot
            .iter()
            .any(|path| path.is_selected() && path.is_ip())
        {
            return;
        }
    }
}

/// The stable refusal a peer receives when its request would have crossed a
/// path this device will not send blob bytes over.
///
/// The contract's own code, so a peer reads the same name the Cloud plane
/// returns for the same idea, and an older peer meets a refusal shape it
/// already understands rather than a new frame.
pub(crate) const RELAY_PATH_REFUSED: &str = "relay_path_refused";
