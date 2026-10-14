// SPDX-License-Identifier: MPL-2.0

use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{AttentionState, ErrorCode, SessionId};

macro_rules! positive_counter {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, ErrorCode> {
                (value > 0).then_some(Self(value)).ok_or(ErrorCode::InvalidRequest)
            }

            pub fn get(self) -> u64 { self.0 }

            pub fn next(self) -> Option<Self> { self.0.checked_add(1).map(Self) }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where S: Serializer { serializer.serialize_u64(self.0) }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where D: Deserializer<'de> {
                let value = u64::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { Cow::Borrowed(stringify!($name)) }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schemars::json_schema!({ "type": "integer", "format": "uint64", "minimum": 1 })
            }
        }
    };
}

positive_counter!(
    /// Emulator/cache epoch; changes when retained terminal state is replaced.
    TerminalGeneration
);
positive_counter!(
    /// Client viewport-request epoch echoed by snapshots, pages, and deltas.
    ViewportGeneration
);
positive_counter!(
    /// Active session catalogue compare-and-consent epoch.
    SessionCatalogGeneration
);
positive_counter!(
    /// Ordered position within one session/render stream epoch.
    StreamSequence
);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct RowIndex(pub u64);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ColumnIndex(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GridSize {
    pub columns: u16,
    pub rows: u16,
}

impl GridSize {
    pub fn new(columns: u16, rows: u16) -> Result<Self, ErrorCode> {
        (columns > 0 && rows > 0)
            .then_some(Self { columns, rows })
            .ok_or(ErrorCode::InvalidSize)
    }

    pub fn validate(self) -> Result<(), ErrorCode> {
        Self::new(self.columns, self.rows).map(drop)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RowRange {
    pub first: RowIndex,
    pub count: u16,
}

impl RowRange {
    pub fn validate(self) -> Result<(), ErrorCode> {
        (self.count > 0).then_some(()).ok_or(ErrorCode::InvalidSize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewportRequest {
    pub terminal_generation: TerminalGeneration,
    pub viewport_generation: ViewportGeneration,
    pub rows: RowRange,
    pub columns: u16,
    pub overscan_rows: u16,
}

impl ViewportRequest {
    pub fn validate(self) -> Result<(), ErrorCode> {
        self.rows.validate()?;
        (self.columns > 0)
            .then_some(())
            .ok_or(ErrorCode::InvalidSize)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalColor {
    #[default]
    DefaultForeground,
    DefaultBackground,
    Indexed {
        index: u8,
    },
    Rgb {
        red: u8,
        green: u8,
        blue: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CellStyle {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub inverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            foreground: TerminalColor::DefaultForeground,
            background: TerminalColor::DefaultBackground,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            strikeout: false,
            inverse: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CellWidth {
    Zero,
    One,
    Two,
}

impl CellWidth {
    pub fn columns(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalCell {
    pub column: ColumnIndex,
    /// One grapheme or an empty string for a wide-cell continuation.
    pub text: String,
    pub width: CellWidth,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalRow {
    /// Stable retained-row coordinate, never an emulator-relative line index.
    pub index: RowIndex,
    pub cells: Vec<TerminalCell>,
    /// The terminal's logical line continues directly into the next grid row.
    #[serde(default)]
    pub wrapped_to_next: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CursorState {
    pub row: RowIndex,
    pub column: ColumnIndex,
    pub shape: CursorShape,
    pub blinking: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseTrackingMode {
    #[default]
    Off,
    Click,
    Drag,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseEncoding {
    #[default]
    Legacy,
    Utf8,
    Sgr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalModes {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    /// Compatibility summary retained for existing clients. New input paths
    /// use `mouse_tracking` and `mouse_encoding` below.
    pub mouse_reporting: bool,
    pub mouse_tracking: MouseTrackingMode,
    pub mouse_encoding: MouseEncoding,
    pub focus_reporting: bool,
    pub origin_mode: bool,
    pub auto_wrap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderSnapshot {
    pub session_id: SessionId,
    pub terminal_generation: TerminalGeneration,
    pub viewport_generation: ViewportGeneration,
    /// Last event incorporated in this bounded snapshot.
    pub sequence: StreamSequence,
    pub viewport: RowRange,
    pub size: GridSize,
    pub rows: Vec<TerminalRow>,
    pub cursor: CursorState,
    pub modes: TerminalModes,
    pub attention: AttentionState,
}

/// One atomic observation for a client recovering a multiplexed public
/// stream. Render, lifecycle-event, and raw-output sequences are independent;
/// none may be inferred from another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionObservation {
    pub snapshot: RenderSnapshot,
    pub event_sequence: StreamSequence,
    pub output_sequence: StreamSequence,
}

impl RenderSnapshot {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        self.viewport.validate()?;
        self.size.validate()?;
        if self.rows.len() > usize::from(self.viewport.count) {
            return Err(ErrorCode::InvalidSize);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderDelta {
    pub session_id: SessionId,
    pub terminal_generation: TerminalGeneration,
    pub viewport_generation: ViewportGeneration,
    pub sequence: StreamSequence,
    /// Complete replacements for damaged rows intersecting the viewport only.
    pub rows: Vec<TerminalRow>,
    pub cursor: Option<CursorState>,
    pub modes: Option<TerminalModes>,
    pub attention: Option<AttentionState>,
}

impl RenderDelta {
    pub fn follows(&self, snapshot: &RenderSnapshot) -> Result<(), ErrorCode> {
        if self.session_id != snapshot.session_id
            || self.terminal_generation != snapshot.terminal_generation
            || self.viewport_generation != snapshot.viewport_generation
        {
            return Err(ErrorCode::StaleGeneration);
        }
        if snapshot.sequence.next() != Some(self.sequence) {
            return Err(ErrorCode::StreamGap);
        }
        let end = snapshot.viewport.first.0 + u64::from(snapshot.viewport.count);
        if self
            .rows
            .iter()
            .any(|row| row.index.0 < snapshot.viewport.first.0 || row.index.0 >= end)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScrollbackPage {
    pub session_id: SessionId,
    pub terminal_generation: TerminalGeneration,
    pub viewport_generation: ViewportGeneration,
    pub sequence: StreamSequence,
    pub range: RowRange,
    pub rows: Vec<TerminalRow>,
    pub has_earlier: bool,
    pub has_later: bool,
}

impl ScrollbackPage {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        self.range.validate()?;
        (self.rows.len() <= usize::from(self.range.count))
            .then_some(())
            .ok_or(ErrorCode::InvalidSize)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StreamGap {
    pub session_id: SessionId,
    pub terminal_generation: TerminalGeneration,
    pub viewport_generation: ViewportGeneration,
    pub expected: StreamSequence,
    pub actual: StreamSequence,
    pub resnapshot_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderUpdate {
    Snapshot { snapshot: RenderSnapshot },
    Delta { delta: RenderDelta },
    Gap { gap: StreamGap },
}

impl fmt::Display for StreamSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
