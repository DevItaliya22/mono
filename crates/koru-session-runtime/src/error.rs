// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportStage {
    UnsupportedPlatform,
    InvalidRequest,
    OpenPty,
    Spawn,
    AssignProcessTree,
    Resume,
    Read,
    Write,
    Resize,
    Wait,
    Terminate,
    Thread,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub stage: TransportStage,
    pub os_code: Option<i32>,
}

impl TransportError {
    pub(crate) fn at(stage: TransportStage, error: &std::io::Error) -> Self {
        Self {
            stage,
            os_code: error.raw_os_error(),
        }
    }

    pub(crate) fn stage(stage: TransportStage) -> Self {
        Self {
            stage,
            os_code: None,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "terminal transport failed at {:?}", self.stage)?;
        if let Some(code) = self.os_code {
            write!(formatter, " (OS code {code})")?;
        }
        Ok(())
    }
}

impl std::error::Error for TransportError {}

pub type TransportResult<T> = Result<T, TransportError>;
