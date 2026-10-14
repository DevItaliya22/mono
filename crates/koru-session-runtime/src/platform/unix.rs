// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::GridSize;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::{PlatformControl, PlatformWaiter, ProcessExit, SpawnedPty};
use crate::{SpawnRequest, TransportError, TransportResult, TransportStage};

pub(crate) fn spawn(request: &SpawnRequest) -> TransportResult<SpawnedPty> {
    request.validate()?;
    let size = pty_size(request.size);
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|_| TransportError::stage(TransportStage::OpenPty))?;

    let mut command = CommandBuilder::from_argv(request.argv.clone());
    command.cwd(request.cwd());
    command.env_clear();
    for (key, value) in request.environment() {
        command.env(key, value);
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| TransportError::stage(TransportStage::Spawn))?;
    drop(pair.slave);
    let process_id = child
        .process_id()
        .ok_or_else(|| TransportError::stage(TransportStage::Spawn))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| TransportError::stage(TransportStage::OpenPty))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|_| TransportError::stage(TransportStage::OpenPty))?;

    Ok(SpawnedPty {
        reader,
        writer,
        control: Box::new(UnixControl {
            master: Some(pair.master),
            process_id,
        }),
        waiter: Box::new(UnixWaiter { child }),
    })
}

struct UnixControl {
    master: Option<Box<dyn MasterPty + Send>>,
    process_id: u32,
}

impl Drop for UnixControl {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
        self.close_pty();
    }
}

impl PlatformControl for UnixControl {
    fn resize(&mut self, size: GridSize) -> TransportResult<()> {
        self.master
            .as_ref()
            .ok_or_else(|| TransportError::stage(TransportStage::Resize))?
            .resize(pty_size(size))
            .map_err(|_| TransportError::stage(TransportStage::Resize))
    }

    fn terminate_tree(&mut self) -> TransportResult<()> {
        let result = unsafe { libc::kill(-(self.process_id as libc::pid_t), libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(TransportError::at(TransportStage::Terminate, &error))
        }
    }

    fn close_pty(&mut self) {
        self.master.take();
    }

    fn process_id(&self) -> u32 {
        self.process_id
    }
}

struct UnixWaiter {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PlatformWaiter for UnixWaiter {
    fn wait(mut self: Box<Self>) -> TransportResult<ProcessExit> {
        self.child
            .wait()
            .map(exit_status)
            .map_err(|error| TransportError::at(TransportStage::Wait, &error))
    }
}

fn pty_size(size: GridSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn exit_status(status: portable_pty::ExitStatus) -> ProcessExit {
    ProcessExit {
        code: status.exit_code(),
        success: status.success(),
    }
}
