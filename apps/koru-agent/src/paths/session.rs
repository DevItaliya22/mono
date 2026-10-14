// SPDX-License-Identifier: MPL-2.0

//! Agent-facing re-export of the shared, resolved session path contract.

pub use koru_session_paths::{
    AtomicPath, CleanupAuthority, CleanupPredicate, CleanupReport, SessionPaths,
    VerifiedAttachmentExtension,
};
