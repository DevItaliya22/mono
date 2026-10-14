// SPDX-License-Identifier: MPL-2.0

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

use koru_events::TerminalSessionOutcome;
use koru_session_protocol::{ErrorCode, GridSize, ProcessState};

use crate::{PtyReader, PtyTransport, TransportError};

use super::render::apply_emulator_update;
use super::{Shared, failed};

pub(super) fn reader_loop(reader: PtyReader, shared: Arc<Shared>) {
    let result = catch_unwind(AssertUnwindSafe(|| reader.run(|bytes| shared.parse(bytes))));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => shared.raw_control().failed(error),
        Err(_) => shared.raw_control().panicked(),
    }
}

pub(super) fn wait_for_start(gate: &(Mutex<bool>, Condvar)) {
    let (started, ready) = gate;
    let mut started = started.lock().unwrap_or_else(|held| held.into_inner());
    while !*started {
        started = ready.wait(started).unwrap_or_else(|held| held.into_inner());
    }
}

pub(super) fn release_start(gate: &(Mutex<bool>, Condvar)) {
    let (started, ready) = gate;
    *started.lock().unwrap_or_else(|held| held.into_inner()) = true;
    ready.notify_all();
}

pub(super) fn control_loop(
    mut transport: PtyTransport,
    receiver: ControlReceiver,
    shared: Arc<Shared>,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        loop {
            match receiver.next() {
                ControlAction::Resize(size) => {
                    if transport.resize(size).is_err() {
                        let _ = transport.terminate();
                        shared.finish(
                            failed(ErrorCode::Internal, "PTY resize failed"),
                            TerminalSessionOutcome::Failed,
                        );
                        return;
                    }
                    let mut state = shared.lock();
                    let resized = {
                        let terminal = state
                            .terminal
                            .as_mut()
                            .expect("raw control owns terminal state");
                        let old_visible = terminal.emulator.visible_range();
                        terminal
                            .emulator
                            .resize(size)
                            .map(|update| (old_visible, update))
                    };
                    match resized {
                        Ok((old_visible, update)) => apply_emulator_update(
                            &mut state,
                            shared.id,
                            old_visible,
                            update,
                            true,
                            &shared.stats,
                        ),
                        Err(_) => {
                            drop(state);
                            let _ = transport.terminate();
                            shared.finish(
                                failed(ErrorCode::InvalidSize, "terminal resize was invalid"),
                                TerminalSessionOutcome::Failed,
                            );
                            return;
                        }
                    }
                }
                ControlAction::Exited(result) => match result.and_then(|_| transport.wait()) {
                    Ok(exit) => shared.finish(
                        ProcessState::Exited {
                            code: i32::try_from(exit.code).ok(),
                        },
                        TerminalSessionOutcome::Exited,
                    ),
                    Err(_) => shared.finish(
                        failed(ErrorCode::Internal, "process wait failed"),
                        TerminalSessionOutcome::Failed,
                    ),
                },
                ControlAction::Terminate => match transport.terminate() {
                    Ok(exit) => shared.finish(
                        ProcessState::Exited {
                            code: i32::try_from(exit.code).ok(),
                        },
                        TerminalSessionOutcome::Terminated,
                    ),
                    Err(_) => shared.finish(
                        failed(ErrorCode::Internal, "process termination failed"),
                        TerminalSessionOutcome::Failed,
                    ),
                },
                ControlAction::Failed => {
                    let _ = transport.terminate();
                    shared.finish(
                        failed(ErrorCode::Internal, "PTY reader failed"),
                        TerminalSessionOutcome::Failed,
                    );
                }
                ControlAction::Panicked => {
                    let _ = transport.terminate();
                    shared.finish(
                        failed(ErrorCode::Internal, "session worker panicked"),
                        TerminalSessionOutcome::Failed,
                    );
                }
            }
            if shared.ended.load(Ordering::Acquire) {
                return;
            }
        }
    }));
    if result.is_err() {
        shared.finish(
            failed(ErrorCode::Internal, "session control worker panicked"),
            TerminalSessionOutcome::Failed,
        );
    }
}

struct ControlState {
    resize: Option<GridSize>,
    terminal: Option<ControlAction>,
}

struct ControlShared {
    state: Mutex<ControlState>,
    ready: Condvar,
}

pub(super) struct ControlQueue {
    shared: Arc<ControlShared>,
}

impl Clone for ControlQueue {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

pub(super) struct ControlReceiver {
    shared: Arc<ControlShared>,
}

#[derive(Clone, Debug)]
enum ControlAction {
    Resize(GridSize),
    Exited(crate::TransportResult<crate::ProcessExit>),
    Terminate,
    Failed,
    Panicked,
}

pub(super) fn control_queue() -> (ControlQueue, ControlReceiver) {
    let shared = Arc::new(ControlShared {
        state: Mutex::new(ControlState {
            resize: None,
            terminal: None,
        }),
        ready: Condvar::new(),
    });
    (
        ControlQueue {
            shared: Arc::clone(&shared),
        },
        ControlReceiver { shared },
    )
}

impl ControlQueue {
    pub(super) fn resize(&self, size: GridSize) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if state.terminal.is_none() {
            state.resize = Some(size);
        }
        drop(state);
        self.shared.ready.notify_one();
    }

    pub(super) fn exited(&self, result: crate::TransportResult<crate::ProcessExit>) {
        self.terminal(ControlAction::Exited(result));
    }

    pub(super) fn terminate(&self) {
        self.terminal(ControlAction::Terminate);
    }

    pub(super) fn failed(&self, _error: TransportError) {
        self.terminal(ControlAction::Failed);
    }

    pub(super) fn panicked(&self) {
        self.terminal(ControlAction::Panicked);
    }

    fn terminal(&self, action: ControlAction) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if !matches!(state.terminal, Some(ControlAction::Terminate)) {
            state.terminal = Some(action);
            state.resize = None;
        }
        drop(state);
        self.shared.ready.notify_one();
    }
}

impl ControlReceiver {
    fn next(&self) -> ControlAction {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if let Some(action) = state.terminal.take() {
            return action;
        }
        if let Some(size) = state.resize.take() {
            return ControlAction::Resize(size);
        }
        loop {
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(|held| held.into_inner());
            if let Some(action) = state.terminal.take() {
                return action;
            }
            if let Some(size) = state.resize.take() {
                return ControlAction::Resize(size);
            }
        }
    }
}
