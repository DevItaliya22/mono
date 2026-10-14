// SPDX-License-Identifier: MPL-2.0

//! Bounded Cloud records on the existing dataset stream. No endpoint/listener.
//! The serialized owner supplies the earliest transfer/session/authority deadline
//! and cancels its Stream on every error, EOF, reset or dropped read/write task.
//!
//! **An I/O failure is [`Error::Closed`], never [`Error::Invalid`].** Every read
//! and write here used to answer `Invalid`, so a peer that simply hung up was
//! reported as one that sent a malformed record — which sent two investigations
//! into the codec looking for a fault that was never there. `Invalid` is now
//! produced only by `payload_length` and `decode_record`, which are the only
//! places that actually inspect what the peer sent.
use koru_replication::cloud::transfer::{
    CONTROL_WAIT_MS, Error, MAX_CONTROL, MAX_PAYLOAD, MemoryBudget, Record, Reservation,
    decode_record, encode_record, payload_length,
};
use std::{future::Future, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, sleep_until},
};

mod duplex;
pub use duplex::{Duplex, Event, Status};

/// An elapsed absolute deadline wins even when I/O is already ready. The
/// ordinary timeout wrapper can poll its operation first after a delayed wake.
async fn before_deadline<T>(
    deadline: Instant,
    operation: impl Future<Output = T>,
) -> Result<T, Error> {
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    tokio::select! {
        biased;
        _ = sleep_until(deadline) => Err(Error::Timeout),
        result = operation => Ok(result),
    }
}

pub struct Received {
    pub first_byte: Instant,
    record: Record,
    _reservation: Reservation,
}
impl Received {
    /// The dispatcher retains the reservation while queued metadata or any
    /// other decoded values from this record remain live.
    pub fn into_parts(self) -> (Instant, Record, Reservation) {
        (self.first_byte, self.record, self._reservation)
    }
    /// The memory charge survives state dispatch/handoff rather than ending
    /// immediately after parsing expanded byte arrays.
    pub fn record(&self) -> &Record {
        &self.record
    }
}

pub async fn read<R: AsyncRead + Unpin>(
    reader: &mut R,
    deadline: Instant,
    budget: &MemoryBudget,
) -> Result<Received, Error> {
    let first = before_deadline(deadline, reader.read_u8())
        .await?
        .map_err(|_| Error::Closed)?;
    let first_byte = Instant::now();
    let control_deadline = deadline.min(first_byte + Duration::from_millis(CONTROL_WAIT_MS));
    let (tag, length) = before_deadline(control_deadline, async {
        let mut prefix = [first, 0, 0, 0];
        reader
            .read_exact(&mut prefix[1..])
            .await
            .map_err(|_| Error::Closed)?;
        let tag = reader.read_u8().await.map_err(|_| Error::Closed)?;
        Ok::<_, Error>((tag, payload_length(u32::from_be_bytes(prefix), tag)?))
    })
    .await??;
    // Header/tag checked before this reservation, and reservation before Vec.
    let payload_deadline = if tag == 1 { control_deadline } else { deadline };
    let reservation =
        before_deadline(payload_deadline, budget.reserve_record_wait(length)).await??;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::Unavailable)?;
    bytes.resize(length, 0);
    before_deadline(payload_deadline, reader.read_exact(&mut bytes))
        .await?
        .map_err(|_| Error::Closed)?;
    let record = decode_record(tag, &bytes)?;
    Ok(Received {
        first_byte,
        record,
        _reservation: reservation,
    })
}

pub async fn write<W: AsyncWrite + Unpin>(
    writer: &mut W,
    record: &Record,
    deadline: Instant,
    budget: &MemoryBudget,
) -> Result<(), Error> {
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    // Serialization is bounded itself. Reserve expansion before invoking it.
    let cap = match record {
        Record::Control(_) => MAX_CONTROL,
        Record::Peer(
            koru_replication::protocol::PeerFrame::SyncRequest
            | koru_replication::protocol::PeerFrame::SyncAck,
        ) => 256,
        Record::Peer(koru_replication::protocol::PeerFrame::LoroBatch { messages, .. }) => {
            // Decimal JSON bytes need at most four characters each. Include
            // array separators plus the fixed UUID/tag envelope before encoding.
            // Charging the frozen 16 MiB ceiling for each 1 MiB batch would
            // unnecessarily serialize otherwise independent transfers.
            messages
                .iter()
                .fold(256usize, |size, message| {
                    size.saturating_add(message.len().saturating_mul(4).saturating_add(3))
                })
                .min(MAX_PAYLOAD)
        }
        Record::Peer(_) => MAX_PAYLOAD,
    };
    let _reservation = before_deadline(deadline, budget.reserve_wait(cap * 2 + 4096)).await??;
    let bytes = encode_record(record)?;
    before_deadline(deadline, async {
        writer.write_all(&bytes).await.map_err(|_| Error::Closed)?;
        writer.flush().await.map_err(|_| Error::Closed)
    })
    .await?
}

#[cfg(test)]
mod tests;
