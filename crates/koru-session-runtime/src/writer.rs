// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::{
    MAX_QUEUED_WRITE_BYTES, TransportError, TransportResult, TransportStage, WRITE_CHUNK_BYTES,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePriority {
    Input,
    Interrupt,
}

#[derive(Default)]
struct Queue {
    priority: VecDeque<Vec<u8>>,
    input: VecDeque<Vec<u8>>,
    queued_bytes: usize,
    closed: bool,
    failure: Option<TransportError>,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Clone)]
pub struct PtyWriter {
    shared: Arc<Shared>,
}

impl PtyWriter {
    pub fn queued_bytes(&self) -> usize {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .queued_bytes
    }

    pub fn write_bytes(&self, bytes: &[u8]) -> TransportResult<()> {
        self.enqueue(WritePriority::Input, bytes)
    }

    pub fn write_text(&self, text: &str) -> TransportResult<()> {
        self.write_bytes(text.as_bytes())
    }

    /// Write a key sequence already encoded for the session's current modes.
    pub fn write_key(&self, sequence: &[u8]) -> TransportResult<()> {
        self.write_bytes(sequence)
    }

    pub fn interrupt(&self) -> TransportResult<()> {
        self.enqueue(WritePriority::Interrupt, &[0x03])
    }

    /// Terminal protocol replies share the urgent lane with interrupt.
    pub fn write_protocol(&self, bytes: &[u8]) -> TransportResult<()> {
        self.enqueue(WritePriority::Interrupt, bytes)
    }

    fn enqueue(&self, priority: WritePriority, bytes: &[u8]) -> TransportResult<()> {
        if bytes.is_empty() {
            return Err(TransportError::stage(TransportStage::InvalidRequest));
        }
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if let Some(error) = &queue.failure {
            return Err(error.clone());
        }
        if queue.closed || queue.queued_bytes.saturating_add(bytes.len()) > MAX_QUEUED_WRITE_BYTES {
            return Err(TransportError::stage(TransportStage::Write));
        }

        for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
            let chunk = chunk.to_vec();
            queue.queued_bytes += chunk.len();
            match priority {
                WritePriority::Input => queue.input.push_back(chunk),
                WritePriority::Interrupt => queue.priority.push_back(chunk),
            }
        }
        drop(queue);
        self.shared.ready.notify_one();
        Ok(())
    }
}

pub(crate) struct WriterWorker {
    pub writer: PtyWriter,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl WriterWorker {
    pub fn start(mut output: Box<dyn Write + Send>) -> TransportResult<Self> {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("koru-pty-writer".to_owned())
            .stack_size(128 * 1024)
            .spawn(move || writer_loop(&mut output, &thread_shared))
            .map_err(|error| TransportError::at(TransportStage::Thread, &error))?;
        Ok(Self {
            writer: PtyWriter {
                shared: Arc::clone(&shared),
            },
            shared,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self, discard: bool) -> TransportResult<()> {
        {
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(|held| held.into_inner());
            queue.closed = true;
            if discard {
                queue.priority.clear();
                queue.input.clear();
                queue.queued_bytes = 0;
            }
        }
        self.shared.ready.notify_one();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| TransportError::stage(TransportStage::Thread))?;
        }
        let queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        queue.failure.clone().map_or(Ok(()), Err)
    }
}

fn writer_loop(output: &mut dyn Write, shared: &Shared) {
    loop {
        let chunk = {
            let mut queue = shared.queue.lock().unwrap_or_else(|held| held.into_inner());
            while queue.priority.is_empty() && queue.input.is_empty() && !queue.closed {
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(|held| held.into_inner());
            }
            let chunk = queue
                .priority
                .pop_front()
                .or_else(|| queue.input.pop_front());
            if let Some(chunk) = &chunk {
                queue.queued_bytes = queue.queued_bytes.saturating_sub(chunk.len());
            } else if queue.closed {
                return;
            }
            chunk
        };

        if let Some(chunk) = chunk
            && let Err(error) = output.write_all(&chunk).and_then(|()| output.flush())
        {
            let mut queue = shared.queue.lock().unwrap_or_else(|held| held.into_inner());
            queue.failure = Some(TransportError::at(TransportStage::Write, &error));
            queue.closed = true;
            queue.priority.clear();
            queue.input.clear();
            queue.queued_bytes = 0;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Condvar, Mutex};

    use super::{MAX_QUEUED_WRITE_BYTES, WriterWorker};

    #[derive(Default)]
    struct GateState {
        writes: Vec<Vec<u8>>,
        first_entered: bool,
        released: bool,
    }

    struct GateWriter {
        state: Arc<(Mutex<GateState>, Condvar)>,
    }

    impl Write for GateWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let (lock, ready) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.writes.push(buffer.to_vec());
            if state.writes.len() == 1 {
                state.first_entered = true;
                ready.notify_all();
                while !state.released {
                    state = ready.wait(state).unwrap();
                }
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupt_overtakes_queued_input_without_unbounding_the_queue() {
        let state = Arc::new((Mutex::new(GateState::default()), Condvar::new()));
        let mut worker = WriterWorker::start(Box::new(GateWriter {
            state: Arc::clone(&state),
        }))
        .unwrap();
        worker.writer.write_bytes(&vec![b'a'; 32 * 1024]).unwrap();

        let (lock, ready) = &*state;
        let mut gate = lock.lock().unwrap();
        while !gate.first_entered {
            gate = ready.wait(gate).unwrap();
        }
        drop(gate);
        worker.writer.interrupt().unwrap();
        assert!(
            worker
                .writer
                .write_bytes(&vec![0; MAX_QUEUED_WRITE_BYTES + 1])
                .is_err()
        );

        let mut gate = lock.lock().unwrap();
        gate.released = true;
        ready.notify_all();
        drop(gate);
        worker.stop(false).unwrap();

        let gate = lock.lock().unwrap();
        assert_eq!(gate.writes.len(), 3);
        assert_eq!(gate.writes[0], vec![b'a'; 16 * 1024]);
        assert_eq!(gate.writes[1], vec![0x03]);
        assert_eq!(gate.writes[2], vec![b'a'; 16 * 1024]);
    }

    #[test]
    #[ignore = "reproducible engineering measurement; see docs/contributing/operations/RUNBOOK.md#terminal-split-stack-probe"]
    fn writer_strategy_measurement() {
        use std::time::Instant;

        let started = Instant::now();
        let mut normal = WriterWorker::start(Box::new(io::sink())).unwrap();
        for _ in 0..1_000 {
            normal
                .writer
                .write_bytes(b"short interactive input")
                .unwrap();
        }
        normal.stop(false).unwrap();
        let normal_elapsed = started.elapsed();

        let started = Instant::now();
        let mut bulk = WriterWorker::start(Box::new(io::sink())).unwrap();
        bulk.writer
            .write_bytes(&vec![b'p'; MAX_QUEUED_WRITE_BYTES])
            .unwrap();
        bulk.stop(false).unwrap();
        let bulk_elapsed = started.elapsed();

        let started = Instant::now();
        let mut sessions: Vec<_> = (0..100)
            .map(|_| WriterWorker::start(Box::new(io::sink())).unwrap())
            .collect();
        for session in &sessions {
            session
                .writer
                .write_bytes(b"one input per session")
                .unwrap();
        }
        for session in &mut sessions {
            session.stop(false).unwrap();
        }
        let hundred_elapsed = started.elapsed();

        eprintln!(
            "normal_1000={normal_elapsed:?} bulk_1mib={bulk_elapsed:?} hundred_sessions={hundred_elapsed:?}"
        );
    }
}
