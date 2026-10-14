// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{RawCommand, RowRange};

/// Terminal-specific payload carried by the shared session-surface boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalIntent {
    Raw(RawCommand),
    SetViewport {
        rows: RowRange,
        columns: u16,
        overscan_rows: u16,
    },
    RequestScrollback {
        range: RowRange,
    },
    RecoverSnapshot,
}
