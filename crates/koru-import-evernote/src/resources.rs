// SPDX-License-Identifier: MPL-2.0

#[cfg(not(test))]
use std::fs::File;
#[cfg(test)]
use std::io::Cursor;
#[cfg(not(test))]
use std::io::Write;
use std::io::{self, BufRead, Read};
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use koru_import::{ImportError, ImportProgress, MAX_IMPORT_BLOB_BYTES};

const BASE64_CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct ResourceData {
    pub note: usize,
    pub resource: usize,
    pub md5: Option<String>,
    pub spool: Option<usize>,
    pub detected_mime: Option<&'static str>,
    pub invalid: bool,
}

/// Every decoded payload for one ENEX source. File-backed entries own their
/// `NamedTempFile`, so every exit path removes them without a cleanup branch.
pub(crate) struct SpoolSet {
    spools: Vec<Spool>,
    backend: SpoolBackend,
}

enum Spool {
    #[cfg(not(test))]
    File(tempfile::NamedTempFile),
    #[cfg(test)]
    Memory(Arc<[u8]>),
}

enum PendingSpool {
    #[cfg(not(test))]
    File(tempfile::NamedTempFile),
    #[cfg(test)]
    Memory(MemoryPending),
}

#[cfg(test)]
struct MemoryPending {
    bytes: Vec<u8>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl Drop for MemoryPending {
    fn drop(&mut self) {
        self.pending
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

enum SpoolBackend {
    #[cfg(not(test))]
    Temp,
    #[cfg(test)]
    Memory {
        fail_on_begin: Option<usize>,
        begins: usize,
        fail_on_write: Option<usize>,
        writes: Arc<std::sync::atomic::AtomicUsize>,
        max_write: Arc<std::sync::atomic::AtomicUsize>,
        live: Arc<std::sync::atomic::AtomicUsize>,
        pending: Arc<std::sync::atomic::AtomicUsize>,
    },
}

impl Default for SpoolSet {
    fn default() -> Self {
        #[cfg(not(test))]
        {
            Self {
                spools: Vec::new(),
                backend: SpoolBackend::Temp,
            }
        }
        #[cfg(test)]
        {
            Self::memory().0
        }
    }
}

impl SpoolSet {
    fn begin(&mut self) -> io::Result<PendingSpool> {
        match &mut self.backend {
            #[cfg(not(test))]
            SpoolBackend::Temp => tempfile::NamedTempFile::new().map(PendingSpool::File),
            #[cfg(test)]
            SpoolBackend::Memory {
                fail_on_begin,
                begins,
                pending,
                ..
            } => {
                let current = *begins;
                *begins += 1;
                if *fail_on_begin == Some(current) {
                    return Err(io::Error::new(
                        io::ErrorKind::StorageFull,
                        "injected disk full",
                    ));
                }
                pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(PendingSpool::Memory(MemoryPending {
                    bytes: Vec::new(),
                    pending: Arc::clone(pending),
                }))
            }
        }
    }

    fn write(&self, pending: &mut PendingSpool, bytes: &[u8]) -> io::Result<()> {
        #[cfg(test)]
        {
            let SpoolBackend::Memory {
                fail_on_write,
                writes,
                max_write,
                ..
            } = &self.backend;
            max_write.fetch_max(bytes.len(), std::sync::atomic::Ordering::SeqCst);
            let write = writes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if *fail_on_write == Some(write) {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected write disk full",
                ));
            }
        }
        match pending {
            #[cfg(not(test))]
            PendingSpool::File(file) => file.write_all(bytes),
            #[cfg(test)]
            PendingSpool::Memory(memory) => {
                memory.bytes.extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    fn finish(&mut self, pending: PendingSpool) -> io::Result<usize> {
        let spool = match pending {
            #[cfg(not(test))]
            PendingSpool::File(mut file) => {
                file.flush()?;
                Spool::File(file)
            }
            #[cfg(test)]
            PendingSpool::Memory(mut memory) => {
                let bytes = std::mem::take(&mut memory.bytes);
                drop(memory);
                Spool::Memory(Arc::from(bytes))
            }
        };
        let index = self.spools.len();
        self.spools.push(spool);
        #[cfg(test)]
        {
            let SpoolBackend::Memory { live, .. } = &self.backend;
            live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(index)
    }

    pub(crate) fn reader_factory(
        &self,
        index: usize,
    ) -> Box<dyn Fn() -> Box<dyn Read + Send> + Send + Sync> {
        match &self.spools[index] {
            #[cfg(not(test))]
            Spool::File(file) => {
                let path = file.path().to_path_buf();
                Box::new(move || match File::open(&path) {
                    Ok(file) => Box::new(file),
                    Err(error) => Box::new(ErrorReader(Some(error))),
                })
            }
            #[cfg(test)]
            Spool::Memory(bytes) => {
                let bytes = Arc::clone(bytes);
                Box::new(move || Box::new(Cursor::new(Arc::clone(&bytes))))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn memory() -> (Self, SpoolProbe) {
        Self::memory_failing(None)
    }

    #[cfg(test)]
    pub(crate) fn memory_failing(fail_on_begin: Option<usize>) -> (Self, SpoolProbe) {
        Self::memory_with_failures(fail_on_begin, None)
    }

    #[cfg(test)]
    pub(crate) fn memory_write_failing(fail_on_write: Option<usize>) -> (Self, SpoolProbe) {
        Self::memory_with_failures(None, fail_on_write)
    }

    #[cfg(test)]
    fn memory_with_failures(
        fail_on_begin: Option<usize>,
        fail_on_write: Option<usize>,
    ) -> (Self, SpoolProbe) {
        let max_write = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Self {
                spools: Vec::new(),
                backend: SpoolBackend::Memory {
                    fail_on_begin,
                    begins: 0,
                    fail_on_write,
                    writes,
                    max_write: Arc::clone(&max_write),
                    live: Arc::clone(&live),
                    pending: Arc::clone(&pending),
                },
            },
            SpoolProbe {
                max_write,
                live,
                pending,
            },
        )
    }
}

impl Drop for SpoolSet {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            let SpoolBackend::Memory { live, .. } = &self.backend;
            live.fetch_sub(self.spools.len(), std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(not(test))]
struct ErrorReader(Option<io::Error>);

#[cfg(not(test))]
impl Read for ErrorReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(self.0.take().unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "resource spool is unavailable")
        }))
    }
}

#[cfg(test)]
pub(crate) struct SpoolProbe {
    max_write: Arc<std::sync::atomic::AtomicUsize>,
    live: Arc<std::sync::atomic::AtomicUsize>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl SpoolProbe {
    pub(crate) fn max_write(&self) -> usize {
        self.max_write.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn live(&self) -> usize {
        self.live.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn pending(&self) -> usize {
        self.pending.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Scan only outer ENEX resource payloads. This tokenizer deliberately owns
/// Base64 text rather than asking quick-xml for a text event: quick-xml's
/// `read_event_into` must grow one buffer until the closing `<`, which would
/// make a single resource one allocation.
pub(crate) fn spool_resource_data<R: BufRead>(
    input: R,
    source: &Path,
    progress: Option<&dyn ImportProgress>,
    spools: &mut SpoolSet,
) -> Result<Vec<ResourceData>, ImportError> {
    let mut scanner = Scanner::new(input, source, progress);
    let mut results = Vec::new();
    let mut note = None;
    let mut next_note = 0usize;
    let mut resource = None;
    let mut next_resource = 0usize;

    while let Some(tag) = scanner.next_tag()? {
        let (closing, name, empty) = tag_shape(&tag);
        match (closing, name) {
            (false, b"note") if note.is_none() => {
                note = Some(next_note);
                next_note += 1;
                next_resource = 0;
            }
            (true, b"note") => {
                note = None;
                resource = None;
            }
            (false, b"resource") if note.is_some() => {
                resource = Some(next_resource);
                next_resource += 1;
            }
            (true, b"resource") => resource = None,
            (false, b"data") if note.is_some() && resource.is_some() && !empty => {
                let value = scanner.decode_data(spools)?;
                results.push(ResourceData {
                    note: note.expect("data is inside note"),
                    resource: resource.expect("data is inside resource"),
                    md5: value.as_ref().map(|value| value.0.clone()),
                    spool: value.as_ref().map(|value| value.1),
                    detected_mime: value.as_ref().and_then(|value| value.2),
                    invalid: value.is_none(),
                });
            }
            _ => {}
        }
        if empty && name == b"note" {
            note = None;
        }
    }
    Ok(results)
}

struct Scanner<'a, R> {
    input: R,
    source: &'a Path,
    progress: Option<&'a dyn ImportProgress>,
}

impl<'a, R: BufRead> Scanner<'a, R> {
    fn new(input: R, source: &'a Path, progress: Option<&'a dyn ImportProgress>) -> Self {
        Self {
            input,
            source,
            progress,
        }
    }

    fn check_cancel(&self) -> Result<(), ImportError> {
        if self.progress.is_some_and(ImportProgress::cancelled) {
            Err(ImportError::cancelled())
        } else {
            Ok(())
        }
    }

    fn next_tag(&mut self) -> Result<Option<Vec<u8>>, ImportError> {
        loop {
            self.check_cancel()?;
            let available = match self.input.fill_buf() {
                Ok(available) => available,
                Err(error) => return Err(read_error(self.source, error)),
            };
            if available.is_empty() {
                return Ok(None);
            }
            if let Some(position) = available.iter().position(|byte| *byte == b'<') {
                self.input.consume(position + 1);
                let tag = self.read_markup()?;
                if let Some(tag) = tag {
                    return Ok(Some(tag));
                }
            } else {
                let count = available.len();
                self.input.consume(count);
            }
        }
    }

    fn read_markup(&mut self) -> Result<Option<Vec<u8>>, ImportError> {
        let mut tag = Vec::new();
        let mut quote = None;
        loop {
            let Some(byte) = self.read_byte()? else {
                return Ok(None);
            };
            tag.push(byte);
            if tag == b"![CDATA[" {
                self.skip_until(b"]]>")?;
                return Ok(None);
            }
            if tag == b"!--" {
                self.skip_until(b"-->")?;
                return Ok(None);
            }
            if tag == b"?" {
                self.skip_until(b"?>")?;
                return Ok(None);
            }
            match byte {
                b'\'' | b'"' if quote.is_none() => quote = Some(byte),
                value if quote == Some(value) => quote = None,
                b'>' if quote.is_none() => {
                    tag.pop();
                    return Ok(Some(tag));
                }
                _ => {}
            }
            if tag.len() > 1024 * 1024 {
                return Err(ImportError::new("XML tag exceeds import security limit"));
            }
        }
    }

    fn decode_data(
        &mut self,
        spools: &mut SpoolSet,
    ) -> Result<Option<(String, usize, Option<&'static str>)>, ImportError> {
        let pending = spools.begin().map_err(|error| {
            ImportError::new(format!(
                "could not spool a resource from {}: {error}",
                self.source.display()
            ))
        })?;
        let mut decoder = ChunkDecoder::new(pending);
        let mut invalid = false;
        loop {
            self.check_cancel()?;
            let available = match self.input.fill_buf() {
                Ok(available) => available,
                Err(error) => return Err(read_error(self.source, error)),
            };
            if available.is_empty() {
                invalid = true;
                break;
            }
            let available = &available[..available.len().min(BASE64_CHUNK)];
            if let Some(position) = available.iter().position(|byte| *byte == b'<') {
                if !invalid && let Err(error) = decoder.push(&available[..position], spools) {
                    if error.kind() == io::ErrorKind::InvalidData {
                        invalid = true;
                    } else {
                        return Err(spool_error(self.source, error));
                    }
                }
                self.input.consume(position + 1);
                let Some(tag) = self.read_markup()? else {
                    invalid = true;
                    continue;
                };
                let (closing, name, _) = tag_shape(&tag);
                if closing && name.eq_ignore_ascii_case(b"data") {
                    break;
                }
                invalid = true;
            } else {
                if !invalid && let Err(error) = decoder.push(available, spools) {
                    if error.kind() == io::ErrorKind::InvalidData {
                        invalid = true;
                    } else {
                        return Err(spool_error(self.source, error));
                    }
                }
                let count = available.len();
                self.input.consume(count);
            }
        }
        if invalid {
            return Ok(None);
        }
        decoder.finish(spools).map(Some).or_else(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                Ok(None)
            } else {
                Err(spool_error(self.source, error))
            }
        })
    }

    fn skip_until(&mut self, sequence: &[u8]) -> Result<(), ImportError> {
        let mut matched = 0usize;
        while let Some(byte) = self.read_byte()? {
            matched = if byte == sequence[matched] {
                matched + 1
            } else {
                usize::from(byte == sequence[0])
            };
            if matched == sequence.len() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn read_byte(&mut self) -> Result<Option<u8>, ImportError> {
        self.check_cancel()?;
        let available = match self.input.fill_buf() {
            Ok(available) => available,
            Err(error) => return Err(read_error(self.source, error)),
        };
        let value = available.first().copied();
        if value.is_some() {
            self.input.consume(1);
        }
        Ok(value)
    }
}

fn read_error(source: &Path, error: io::Error) -> ImportError {
    ImportError::new(format!("could not read {}: {error}", source.display()))
}

fn spool_error(source: &Path, error: io::Error) -> ImportError {
    ImportError::new(format!(
        "could not spool a resource from {}: {error}",
        source.display()
    ))
}

struct ChunkDecoder {
    pending: PendingSpool,
    compact: Vec<u8>,
    output: Vec<u8>,
    digest: md5::Context,
    total: u64,
    prefix: Vec<u8>,
}

impl ChunkDecoder {
    fn new(pending: PendingSpool) -> Self {
        Self {
            pending,
            compact: Vec::with_capacity(BASE64_CHUNK + 4),
            output: vec![0; BASE64_CHUNK],
            digest: md5::Context::new(),
            total: 0,
            prefix: Vec::with_capacity(16),
        }
    }

    fn push(&mut self, input: &[u8], spools: &SpoolSet) -> io::Result<()> {
        self.compact.extend(
            input
                .iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace()),
        );
        // Retain one quartet until the closing tag. Padding is legal only in
        // that final quartet, and decoding it early would make a following
        // chunk look like a second Base64 value.
        let complete = self.compact.len().saturating_sub(4) / 4 * 4;
        if complete == 0 {
            return Ok(());
        }
        self.decode_prefix(complete, spools)
    }

    fn finish(
        mut self,
        spools: &mut SpoolSet,
    ) -> io::Result<(String, usize, Option<&'static str>)> {
        let complete = self.compact.len();
        if complete % 4 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Base64 resource",
            ));
        }
        self.decode_prefix(complete, spools)?;
        let digest = std::mem::replace(&mut self.digest, md5::Context::new()).compute();
        let detected = sniff_mime(&self.prefix);
        let spool = spools.finish(self.pending)?;
        Ok((format!("{digest:x}"), spool, detected))
    }

    fn decode_prefix(&mut self, count: usize, spools: &SpoolSet) -> io::Result<()> {
        let decoded = STANDARD
            .decode_slice(&self.compact[..count], &mut self.output)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Base64 resource"))?;
        self.total = self.total.saturating_add(decoded as u64);
        if self.total > MAX_IMPORT_BLOB_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resource exceeds 2 GiB",
            ));
        }
        spools.write(&mut self.pending, &self.output[..decoded])?;
        let prefix_bytes = (16 - self.prefix.len()).min(decoded);
        self.prefix.extend_from_slice(&self.output[..prefix_bytes]);
        self.digest.consume(&self.output[..decoded]);
        self.compact.drain(..count);
        Ok(())
    }
}

fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"PK\x03\x04") {
        Some("application/zip")
    } else {
        None
    }
}

fn tag_shape(mut tag: &[u8]) -> (bool, &[u8], bool) {
    tag = trim_ascii(tag);
    let closing = tag.first() == Some(&b'/');
    if closing {
        tag = trim_ascii(&tag[1..]);
    }
    let empty = tag.last() == Some(&b'/');
    let end = tag
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || *byte == b'/')
        .unwrap_or(tag.len());
    (closing, &tag[..end], empty)
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests;
