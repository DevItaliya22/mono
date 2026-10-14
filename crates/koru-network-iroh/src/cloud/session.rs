// SPDX-License-Identifier: MPL-2.0

//! Fence an entire dataset stream, including idle waits and pending admission.
//! No connection-wide close: unrelated dataset streams may keep serving.

use iroh::endpoint::{RecvStream, SendStream};
use koru_replication::cloud::Refusal;
use std::{future::Future, pin::Pin};

/// Run the stream's complete admission/serving lifetime under its owner's
/// retained fence signal. On fence, error, panic or caller cancellation, reset
/// only this stream. The handler still owns framing, authorization and durable
/// acknowledgements; a successful return does not add a flush or receipt.
pub async fn with_fence<T, F, Run>(
    writer: &mut SendStream,
    reader: &mut RecvStream,
    fenced: F,
    run: Run,
) -> Result<T, Refusal>
where
    F: Future<Output = Refusal> + Send,
    Run: for<'a> FnOnce(
        &'a mut SendStream,
        &'a mut RecvStream,
    ) -> Pin<Box<dyn Future<Output = Result<T, Refusal>> + Send + 'a>>,
{
    let mut guard = StreamGuard {
        writer,
        reader,
        armed: true,
    };
    let result = {
        let work = run(guard.writer, guard.reader);
        tokio::select! {
            biased;
            reason = fenced => Err(reason),
            result = work => result,
        }
    };
    if result.is_ok() {
        guard.armed = false;
    }
    result
}

struct StreamGuard<'a> {
    writer: &'a mut SendStream,
    reader: &'a mut RecvStream,
    armed: bool,
}
impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.writer.reset(1u32.into());
            let _ = self.reader.stop(1u32.into());
        }
    }
}

#[cfg(test)]
mod tests;
