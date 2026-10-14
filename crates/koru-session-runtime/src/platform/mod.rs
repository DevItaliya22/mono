// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};

use koru_session_protocol::GridSize;

use crate::{SpawnRequest, TransportResult};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as native;
#[cfg(windows)]
use windows as native;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessExit {
    pub code: u32,
    pub success: bool,
}

pub(crate) trait PlatformControl: Send {
    fn resize(&mut self, size: GridSize) -> TransportResult<()>;
    fn terminate_tree(&mut self) -> TransportResult<()>;
    fn close_pty(&mut self);
    fn process_id(&self) -> u32;
}

pub(crate) trait PlatformWaiter: Send {
    fn wait(self: Box<Self>) -> TransportResult<ProcessExit>;
}

pub(crate) struct SpawnedPty {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub control: Box<dyn PlatformControl>,
    pub waiter: Box<dyn PlatformWaiter>,
}

pub(crate) fn spawn(request: &SpawnRequest) -> TransportResult<SpawnedPty> {
    native::spawn(request)
}
