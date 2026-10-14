// SPDX-License-Identifier: MPL-2.0

//! One retained read and write, polled together without restarting partial I/O.
use super::*;
use std::{future::Future, pin::Pin};

type Read<'a, R> = Pin<Box<dyn Future<Output = (R, Result<Received, Error>)> + Send + 'a>>;
type Write<'a, W> = Pin<Box<dyn Future<Output = (W, Result<(), Error>)> + Send + 'a>>;

pub enum Event {
    Received(Received),
    /// Local framing/flush completed, not a remote durable acknowledgement.
    Written,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub reading: bool,
    pub writing: bool,
    pub closed: Option<Error>,
}

/// Used inside the dataset stream's existing reset/fence guard. This owns I/O
/// futures, not session authorization, transfer policy, or backend completion.
/// There is no queue: the serving owner retains any not-yet-started response.
pub struct Duplex<'a, R, W> {
    reading: Option<Read<'a, R>>,
    writing: Option<Write<'a, W>>,
    writer: Option<W>,
    session_deadline: Instant,
    budget: MemoryBudget,
    closed: Option<Error>,
}

impl<'a, R, W> Duplex<'a, R, W>
where
    R: AsyncRead + Unpin + Send + 'a,
    W: AsyncWrite + Unpin + Send + 'a,
{
    pub fn new(reader: R, writer: W, session_deadline: Instant, budget: MemoryBudget) -> Self {
        let reading = Some(Self::read_next(reader, session_deadline, budget.clone()));
        Self {
            reading,
            writing: None,
            writer: Some(writer),
            session_deadline,
            budget,
            closed: None,
        }
    }

    fn read_next(mut reader: R, deadline: Instant, budget: MemoryBudget) -> Read<'a, R> {
        Box::pin(async move {
            let result = read(&mut reader, deadline, &budget).await;
            (reader, result)
        })
    }

    pub fn status(&self) -> Status {
        Status {
            reading: self.reading.is_some(),
            writing: self.writing.is_some(),
            closed: self.closed,
        }
    }

    pub fn can_send(&self) -> bool {
        self.closed.is_none() && self.writer.is_some()
    }

    /// Consume and retain the original receipt/export/fragment while borrowing
    /// its record. No cloned payload and no early release when next() is canceled.
    /// A busy/closed refusal consumes and drops value; check can_send first.
    pub fn send<T: Send + 'a>(
        &mut self,
        value: T,
        record: fn(&T) -> &Record,
        deadline: Instant,
    ) -> Result<(), Error> {
        if let Some(error) = self.closed {
            return Err(error);
        }
        let deadline = deadline.min(self.session_deadline);
        if Instant::now() >= deadline {
            return Err(self.cancel(Error::Timeout));
        }
        let mut writer = self.writer.take().ok_or(Error::Unavailable)?;
        let budget = self.budget.clone();
        self.writing = Some(Box::pin(async move {
            let result = write(&mut writer, record(&value), deadline, &budget).await;
            // value lives through the exact write/flush, even if the event waiter
            // was canceled. Destruction is local ownership, not a remote Ack.
            drop(value);
            (writer, result)
        }));
        Ok(())
    }

    /// Cancel-safe event wait: losing an outer select never discards a partial
    /// prefix/payload or pending write. The caller must still race live fences
    /// and transfer deadlines, and cancel this owner when any of them expires.
    pub async fn next(&mut self) -> Result<Event, Error> {
        if let Some(error) = self.closed {
            return Err(error);
        }
        tokio::select! {
            // Fair selection: a ready read must not starve a blocked write's
            // progress (and vice versa) during simultaneous catch-up.
            (reader, result) = self.reading.as_mut().expect("open read") => {
                self.reading = None;
                match result {
                    Ok(received) => {
                        self.reading = Some(Self::read_next(reader, self.session_deadline, self.budget.clone()));
                        Ok(Event::Received(received))
                    }
                    Err(error) => Err(self.cancel(error)),
                }
            }
            (writer, result) = async { self.writing.as_mut().expect("guarded write").await }, if self.writing.is_some() => {
                self.finish_write(writer, result)
            }
        }
    }

    /// Drain the current write without polling the retained read. Used when
    /// an already-received next start waits for the previous receipt's flush.
    /// Canceling this wait preserves both original I/O futures.
    pub async fn next_write(&mut self) -> Result<Event, Error> {
        if let Some(error) = self.closed {
            return Err(error);
        }
        let (writer, result) = self.writing.as_mut().ok_or(Error::Unavailable)?.await;
        self.finish_write(writer, result)
    }
    fn finish_write(&mut self, writer: W, result: Result<(), Error>) -> Result<Event, Error> {
        self.writing = None;
        match result {
            Ok(()) => {
                self.writer = Some(writer);
                Ok(Event::Written)
            }
            Err(error) => Err(self.cancel(error)),
        }
    }

    /// Releases only adapter-owned values. The outer Iroh guard resets the
    /// affected stream; an already handed-off actor job remains actor-owned.
    pub fn cancel(&mut self, error: Error) -> Error {
        let error = *self.closed.get_or_insert(error);
        self.reading = None;
        self.writing = None;
        self.writer = None;
        error
    }
}

#[cfg(test)]
mod tests;
