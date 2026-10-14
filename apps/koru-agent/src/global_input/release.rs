// SPDX-License-Identifier: MPL-2.0

use std::io;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The one registered key a hold observer is allowed to inspect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualKey(u16);

impl VirtualKey {
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
    /// The active input desktop cannot be proved to be Agent's desktop. A zero
    /// key bit must never be interpreted as release in this state.
    Unavailable,
}

/// A source sees exactly the virtual key supplied by the active hold. The
/// observer never calls it while idle.
pub trait KeyStateSource: Send + 'static {
    fn state(&mut self, key: VirtualKey) -> KeyState;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressError {
    Unavailable,
}

/// Fakeable ownership of RegisterHotKey. The Windows implementation keeps its
/// registration token thread-bound; tests substitute an in-memory registrar.
pub trait PressRegistrar {
    type Chord;
    type Registration;

    fn register(
        &mut self,
        action: i32,
        chord: Self::Chord,
    ) -> Result<Self::Registration, PressError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    Explicit,
    SecureDesktop,
    Resume,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldOutcome {
    Released,
    Cancelled(CancelReason),
    TimedOut,
}

/// Coordinator-minted identity for one armed push-to-talk hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldFlight(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldNotice {
    Pressed {
        flight: HoldFlight,
    },
    Finished {
        flight: HoldFlight,
        outcome: HoldOutcome,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ObserverConfig {
    pub poll_interval: Duration,
    pub max_hold: Duration,
    /// A sampling gap this large is treated as sleep/resume. Cancelling is
    /// safer than committing into whichever control regains focus.
    pub resume_gap: Duration,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(15),
            max_hold: koru_dictation::HOLD_CAP,
            resume_gap: Duration::from_secs(1),
        }
    }
}

enum Command {
    Press(VirtualKey, HoldFlight),
    Cancel,
    Resume,
    Shutdown,
}

struct Flight {
    id: HoldFlight,
    key: VirtualKey,
    started: Instant,
    sampled: Instant,
}

/// One dedicated observer thread. Its idle state is a blocking channel receive,
/// so there is no timer, Windows call, or polling work until press.
pub struct ReleaseObserver {
    sender: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl ReleaseObserver {
    pub fn spawn<S>(source: S, emit: impl Fn(HoldNotice) + Send + 'static) -> io::Result<Self>
    where
        S: KeyStateSource,
    {
        Self::spawn_with_config(source, ObserverConfig::default(), emit)
    }

    pub fn spawn_with_config<S>(
        mut source: S,
        config: ObserverConfig,
        emit: impl Fn(HoldNotice) + Send + 'static,
    ) -> io::Result<Self>
    where
        S: KeyStateSource,
    {
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("koru-agent dictation release".into())
            .spawn(move || {
                let mut flight: Option<Flight> = None;
                loop {
                    let command = match &flight {
                        None => receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                        Some(_) => receiver.recv_timeout(config.poll_interval),
                    };
                    match command {
                        Ok(Command::Press(key, id)) if flight.is_none() => {
                            let now = Instant::now();
                            flight = Some(Flight {
                                id,
                                key,
                                started: now,
                                sampled: now,
                            });
                            emit(HoldNotice::Pressed { flight: id });
                        }
                        Ok(Command::Press(_, _)) => {
                            // RegisterHotKey repeat is not a second hold.
                        }
                        Ok(Command::Cancel) => finish(
                            &mut flight,
                            HoldOutcome::Cancelled(CancelReason::Explicit),
                            &emit,
                        ),
                        Ok(Command::Resume) => finish(
                            &mut flight,
                            HoldOutcome::Cancelled(CancelReason::Resume),
                            &emit,
                        ),
                        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            finish(
                                &mut flight,
                                HoldOutcome::Cancelled(CancelReason::Shutdown),
                                &emit,
                            );
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let Some(active) = flight.as_mut() else {
                                continue;
                            };
                            let now = Instant::now();
                            let outcome = if now.duration_since(active.sampled) >= config.resume_gap
                            {
                                Some(HoldOutcome::Cancelled(CancelReason::Resume))
                            } else if now.duration_since(active.started) >= config.max_hold {
                                Some(HoldOutcome::TimedOut)
                            } else {
                                match source.state(active.key) {
                                    KeyState::Down => {
                                        active.sampled = now;
                                        None
                                    }
                                    KeyState::Up => Some(HoldOutcome::Released),
                                    KeyState::Unavailable => {
                                        Some(HoldOutcome::Cancelled(CancelReason::SecureDesktop))
                                    }
                                }
                            };
                            if let Some(outcome) = outcome {
                                finish(&mut flight, outcome, &emit);
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    pub fn press(&self, key: VirtualKey, flight: HoldFlight) {
        let _ = self.sender.send(Command::Press(key, flight));
    }

    pub fn cancel(&self) {
        let _ = self.sender.send(Command::Cancel);
    }

    /// Called when Agent receives the platform resume notification. The gap
    /// detector is the second line of defence when that notification is late.
    pub fn resumed(&self) {
        let _ = self.sender.send(Command::Resume);
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ReleaseObserver {
    fn drop(&mut self) {
        self.stop();
    }
}

fn finish(flight: &mut Option<Flight>, outcome: HoldOutcome, emit: &impl Fn(HoldNotice)) {
    if let Some(flight) = flight.take() {
        emit(HoldNotice::Finished {
            flight: flight.id,
            outcome,
        });
    }
}
