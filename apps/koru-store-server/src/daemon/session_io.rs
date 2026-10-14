// SPDX-License-Identifier: MPL-2.0

//! `AsyncRead`/`AsyncWrite` over one bridged [`TransportSession`]'s bounded
//! byte channels, so the Cloud client carrier can run on a stream the Agent
//! authenticated and forwarded over the private bridge exactly as it runs on
//! a QUIC stream in process.
//!
//! The adapter never inspects a byte. It owns no framing, no retry and no
//! buffer beyond the one chunk in flight in each direction: the channel's
//! capacity is the backpressure, the sender's drop is the half-close, and a
//! terminal reset or cancellation surfaces as an I/O error the carrier maps
//! to its own refusal.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use koru_network_edge::{ByteReceiver, ByteSender, EdgeError, TransportSession};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

type ReadPending = Pin<
    Box<dyn Future<Output = (ByteReceiver, Result<Option<Vec<u8>>, EdgeError>)> + Send + 'static>,
>;
type WritePending = Pin<Box<dyn Future<Output = Result<(), EdgeError>> + Send + 'static>>;

/// The receive half. `Ok(None)` from the channel is end of stream; a reset or
/// cancellation is `ConnectionReset`, so the carrier does not mistake a fence
/// for a clean close.
pub struct SessionReader {
    receiver: Option<ByteReceiver>,
    pending: Option<ReadPending>,
    buffer: Vec<u8>,
    offset: usize,
    done: bool,
}

/// The send half. Dropping it, or `poll_shutdown`, half-closes the stream:
/// the Agent's pump finishes the QUIC send side when the channel ends.
pub struct SessionWriter {
    sender: Option<ByteSender>,
    pending: Option<WritePending>,
}

/// Split one admitted session into its two halves. The session's control
/// handle is dropped with the session; a caller that needs out-of-band reset
/// takes [`TransportSession::control`] first.
pub fn split(session: TransportSession) -> (SessionReader, SessionWriter) {
    let TransportSession { send, receive, .. } = session;
    (
        SessionReader {
            receiver: Some(receive),
            pending: None,
            buffer: Vec::new(),
            offset: 0,
            done: false,
        },
        SessionWriter {
            sender: Some(send),
            pending: None,
        },
    )
}

fn map_edge(error: EdgeError) -> io::Error {
    match error {
        EdgeError::Cancelled | EdgeError::Reset(_) => {
            io::Error::new(io::ErrorKind::ConnectionReset, "session reset")
        }
        EdgeError::Closed | EdgeError::InvalidProtocol => {
            io::Error::new(io::ErrorKind::BrokenPipe, "session closed")
        }
    }
}

impl AsyncRead for SessionReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.offset < this.buffer.len() {
                let remaining = &this.buffer[this.offset..];
                let take = remaining.len().min(buf.remaining());
                buf.put_slice(&remaining[..take]);
                this.offset += take;
                if this.offset == this.buffer.len() {
                    this.buffer.clear();
                    this.offset = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.done {
                return Poll::Ready(Ok(()));
            }
            if this.pending.is_none() {
                let Some(mut receiver) = this.receiver.take() else {
                    return Poll::Ready(Err(map_edge(EdgeError::Closed)));
                };
                this.pending = Some(Box::pin(async move {
                    let result = receiver.recv().await;
                    (receiver, result)
                }));
            }
            let pending = this.pending.as_mut().expect("just installed");
            match pending.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready((receiver, result)) => {
                    this.pending = None;
                    this.receiver = Some(receiver);
                    match result {
                        Ok(Some(bytes)) => {
                            this.buffer = bytes;
                            this.offset = 0;
                        }
                        Ok(None) => this.done = true,
                        Err(error) => return Poll::Ready(Err(map_edge(error))),
                    }
                }
            }
        }
    }
}

impl SessionWriter {
    fn drive(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(pending) = self.pending.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        match pending.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(result.map_err(map_edge))
            }
        }
    }
}

impl AsyncWrite for SessionWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match this.drive(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let Some(sender) = this.sender.clone() else {
            return Poll::Ready(Err(map_edge(EdgeError::Closed)));
        };
        let bytes = buf.to_vec();
        let length = bytes.len();
        let mut pending: WritePending = Box::pin(async move { sender.send(bytes).await });
        match pending.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(length)),
            Poll::Ready(Err(error)) => Poll::Ready(Err(map_edge(error))),
            // The bytes are owned by the send in flight; the next write or
            // flush finishes it before anything else is queued, so order is
            // kept and the channel's capacity is the only buffer.
            Poll::Pending => {
                this.pending = Some(pending);
                Poll::Ready(Ok(length))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().drive(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match this.drive(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.sender = None;
                Poll::Ready(result)
            }
        }
    }
}

#[cfg(test)]
mod tests;
