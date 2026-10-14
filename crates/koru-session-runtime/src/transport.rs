// SPDX-License-Identifier: MPL-2.0

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use koru_session_protocol::GridSize;

use crate::platform::{self, PlatformControl, PlatformWaiter};
use crate::{
    PtyReader, PtyWriter, SpawnRequest, TransportError, TransportResult, TransportStage,
    WriterWorker,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: u32,
    pub success: bool,
}

pub struct PtyTransport {
    control: Box<dyn PlatformControl>,
    writer: WriterWorker,
    exit: Arc<(Mutex<ExitState>, Condvar)>,
    waiter: Option<JoinHandle<()>>,
    finished: bool,
}

type ExitCallback = Arc<dyn Fn(TransportResult<ProcessExit>) + Send + Sync>;

#[derive(Default)]
struct ExitState {
    result: Option<TransportResult<ProcessExit>>,
    callback: Option<ExitCallback>,
}

impl PtyTransport {
    pub fn spawn(request: SpawnRequest) -> TransportResult<(Self, PtyReader)> {
        let spawned = platform::spawn(&request)?;
        Self::from_parts(
            spawned.control,
            spawned.waiter,
            spawned.reader,
            spawned.writer,
        )
    }

    pub(crate) fn from_parts(
        control: Box<dyn PlatformControl>,
        waiter: Box<dyn PlatformWaiter>,
        reader: Box<dyn std::io::Read + Send>,
        writer: Box<dyn std::io::Write + Send>,
    ) -> TransportResult<(Self, PtyReader)> {
        let mut writer = WriterWorker::start(writer)?;
        let exit = Arc::new((Mutex::new(ExitState::default()), Condvar::new()));
        let thread_exit = Arc::clone(&exit);
        let waiter = match std::thread::Builder::new()
            .name("koru-pty-waiter".to_owned())
            .stack_size(128 * 1024)
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    waiter.wait().map(Into::into)
                }))
                .unwrap_or_else(|_| Err(TransportError::stage(TransportStage::Thread)));
                let (lock, ready) = &*thread_exit;
                let callback = {
                    let mut state = lock.lock().unwrap_or_else(|held| held.into_inner());
                    state.result = Some(result.clone());
                    state.callback.clone()
                };
                ready.notify_all();
                if let Some(callback) = callback {
                    callback(result);
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                let _ = writer.stop(true);
                return Err(TransportError::at(TransportStage::Thread, &error));
            }
        };
        Ok((
            Self {
                control,
                writer,
                exit,
                waiter: Some(waiter),
                finished: false,
            },
            PtyReader::new(reader),
        ))
    }

    pub fn writer(&self) -> PtyWriter {
        self.writer.writer.clone()
    }

    pub fn process_id(&self) -> u32 {
        self.control.process_id()
    }

    pub fn resize(&mut self, size: GridSize) -> TransportResult<()> {
        size.validate()
            .map_err(|_| crate::TransportError::stage(crate::TransportStage::InvalidRequest))?;
        self.control.resize(size)
    }

    pub(crate) fn set_exit_callback(&mut self, callback: ExitCallback) {
        let result = {
            let mut state = self.exit.0.lock().unwrap_or_else(|held| held.into_inner());
            state.callback = Some(Arc::clone(&callback));
            state.result.clone()
        };
        if let Some(result) = result {
            callback(result);
        }
    }

    pub fn try_wait(&mut self) -> TransportResult<Option<ProcessExit>> {
        match self
            .exit
            .0
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .result
            .clone()
        {
            Some(Ok(exit)) => Ok(Some(exit)),
            Some(Err(error)) => Err(error),
            None => Ok(None),
        }
    }

    pub fn wait(&mut self) -> TransportResult<ProcessExit> {
        let exit = self.wait_for_exit()?;
        // The primary process may exit while a descendant remains alive.
        // Session completion still owns and reclaims the complete tree.
        let terminate_result = self.control.terminate_tree();
        self.control.close_pty();
        let writer_result = self.writer.stop(true);
        self.finished = terminate_result.is_ok();
        terminate_result?;
        writer_result?;
        self.join_waiter()?;
        Ok(exit)
    }

    pub fn terminate(&mut self) -> TransportResult<ProcessExit> {
        let terminate_result = self.control.terminate_tree();
        self.control.close_pty();
        let writer_result = self.writer.stop(true);
        let wait_result = self.wait_for_exit_within(TERMINATION_GRACE);
        self.finished = terminate_result.is_ok();
        terminate_result?;
        // A blocked OS write commonly reports broken-pipe after intentional
        // process-tree termination. Cleanup has priority over that stale input.
        let _ = writer_result;
        let Ok(exit) = wait_result else {
            // The kill was not reported in time, so the waiter is still inside
            // the OS call and must not be joined.
            self.detach_waiter();
            return wait_result;
        };
        self.join_waiter()?;
        Ok(exit)
    }

    /// Wait for the process to end **on its own**, however long that takes.
    ///
    /// This one is deliberately unbounded: it backs [`Self::wait`], whose whole
    /// contract is "tell me when this shell exits", and a shell may sit at a
    /// prompt for a day. Teardown uses [`Self::wait_for_exit_within`] instead.
    fn wait_for_exit(&self) -> TransportResult<ProcessExit> {
        let (lock, ready) = &*self.exit;
        let mut state = lock.lock().unwrap_or_else(|held| held.into_inner());
        while state.result.is_none() {
            state = ready.wait(state).unwrap_or_else(|held| held.into_inner());
        }
        state.result.clone().expect("loop proved result")
    }

    /// Wait for a process this transport has just **killed**, and give up.
    ///
    /// **Nothing in teardown may be able to park forever.** Both waits here are
    /// on another thread that is inside the OS's process-wait — `wait4` on unix,
    /// the job object on Windows — and a descendant the kernel will not deliver
    /// a signal to never lets that return. On this Mac that produced a test
    /// wedged for thirty-two minutes with both ends parked: the dropping thread
    /// on this condvar, `koru-pty-waiter` in `wait4`, and a `sleep` in
    /// uninterruptible state that had outlived its own duration. Windows cannot
    /// reach it — its adapter kills through a Job Object and the kernel tears
    /// the tree down atomically — which is why an unbounded wait survived here.
    ///
    /// A deadline does not make the process die. It makes the failure *finite*,
    /// which is the difference between a suite that reports and a suite that
    /// never finishes, and it is what makes the remaining work
    /// ([TECHDEBT 92](../../../docs/contributing/techdebt/README.md) — the `ESRCH` that reads
    /// as success, and the teardown order) something a person can iterate on.
    fn wait_for_exit_within(&self, grace: Duration) -> TransportResult<ProcessExit> {
        let (lock, ready) = &*self.exit;
        let deadline = Instant::now() + grace;
        let mut state = lock.lock().unwrap_or_else(|held| held.into_inner());
        while state.result.is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(TransportError::stage(TransportStage::Wait));
            };
            let (next, timeout) = ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|held| held.into_inner());
            state = next;
            if timeout.timed_out() && state.result.is_none() {
                return Err(TransportError::stage(TransportStage::Wait));
            }
        }
        state.result.clone().expect("loop proved result")
    }

    fn join_waiter(&mut self) -> TransportResult<()> {
        if let Some(waiter) = self.waiter.take() {
            waiter
                .join()
                .map_err(|_| TransportError::stage(TransportStage::Thread))?;
        }
        Ok(())
    }

    /// Let the waiter thread go without joining it.
    ///
    /// **Only after [`Self::wait_for_exit_within`] has given up**, and it is the
    /// other half of that deadline: the thread is parked in the OS's
    /// process-wait, so `join` would reintroduce precisely the hang the deadline
    /// exists to prevent. Dropping the handle detaches it. That leaks one thread
    /// per session whose tree could not be reclaimed — a bounded, observable
    /// cost, and a far smaller one than a process that cannot exit.
    fn detach_waiter(&mut self) {
        self.waiter.take();
    }
}

/// How long teardown waits for the OS to report a process it has already killed.
///
/// **A liveness deadline, not a performance budget** — the distinction the
/// runbook draws, and the reason this may be as generous as it likes: nothing is
/// claimed by it, and a kill that has not been reported in ten seconds is not
/// slow, it is wedged.
const TERMINATION_GRACE: Duration = Duration::from_secs(10);

impl Drop for PtyTransport {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.control.terminate_tree();
            self.control.close_pty();
            let _ = self.writer.stop(true);
            if self.wait_for_exit_within(TERMINATION_GRACE).is_ok() {
                let _ = self.join_waiter();
            } else {
                self.detach_waiter();
            }
            self.finished = true;
        }
    }
}

impl From<platform::ProcessExit> for ProcessExit {
    fn from(exit: platform::ProcessExit) -> Self {
        Self {
            code: exit.code,
            success: exit.success,
        }
    }
}
