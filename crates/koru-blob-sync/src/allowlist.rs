// SPDX-License-Identifier: MPL-2.0

//! Where a person's file may be sent.
//!
//! The contract's §2 ends with one sentence that this module is the whole of:
//! *"A control response is not authority to send a person's file anywhere."*
//! Allocation returns a byte URL and the client never constructs one — which
//! means the client is trusting a response with the bytes of somebody's
//! photograph, and a control plane that was talked into returning
//! `https://evil.example.com/collect` would be handed them.
//!
//! So a returned URL is admitted only when **its origin is one this device
//! already held**. Not its host — its origin: scheme, host and port together,
//! because `http://us.data.koru.do` and `https://us.data.koru.do` are two
//! different places to send a file and only one of them is encrypted.
//!
//! **The production set is compiled in, per region** (§2's table). It is not
//! configuration, because a configurable allowlist is an allowlist an attacker
//! configures. A local stack is the one exception and it is explicit: a person
//! running the object worker on their own machine names its origin with
//! `KORU_CLOUD_BLOB_BYTE_ORIGINS`, and that value admits only a loopback or
//! private address, never a public host.

use std::collections::BTreeSet;

use koru_account_core::workspace_authorization::Region;
use url::{Host, Url};

/// Why a returned byte URL was not admitted.
///
/// Its own type rather than a `BlobCode`: none of the contract's codes is
/// about this, because this refusal never leaves the device and is never a
/// statement about the server's answer being malformed. It is a statement
/// about *us* declining to go there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginRefused {
    /// Not a URL at all.
    Unparseable,
    /// `http`, `file`, or anything else. Only `https` reaches a real host.
    Scheme,
    /// The origin is not one this device holds for the account's region.
    NotAllowed,
    /// A URL carrying credentials, a query or a fragment. The contract's byte
    /// routes take none of those, so one that arrived came from somewhere that
    /// wanted something else to happen.
    Shape,
}

impl OriginRefused {
    /// Untranslated developer text. A person is shown their own sentence.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unparseable => "the returned byte URL is not a URL",
            Self::Scheme => "the returned byte URL is not https",
            Self::NotAllowed => "the returned byte URL names a host this device does not hold",
            Self::Shape => "the returned byte URL carries credentials, a query or a fragment",
        }
    }
}

/// A byte URL that has passed the allowlist.
///
/// **The only way to make one is [`ByteOrigins::admit`]**, and the object
/// plane takes nothing else. Forgetting the check is therefore a compile
/// error rather than a code review: a contract clause that reads *"a client
/// MUST refuse"* is worth a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedUrl(Url);

impl AdmittedUrl {
    pub fn as_url(&self) -> &Url {
        &self.0
    }
    pub fn into_url(self) -> Url {
        self.0
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The same URL with one more path segment.
    ///
    /// **Admission is a property of the origin**, and appending a segment
    /// cannot change one — so this is not a second door into the type, it is
    /// the only honest way to reach `…/uploads/{upload}/complete` from the
    /// `…/uploads/{upload}` allocation returned. The alternative was letting
    /// the object plane take a string again, which is the door.
    pub fn join_segment(&self, segment: &str) -> Self {
        let mut url = self.0.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().push(segment);
        }
        Self(url)
    }
}

impl std::fmt::Display for AdmittedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// The object hosts of one region, plus whatever local stack this device was
/// explicitly told about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteOrigins {
    origins: BTreeSet<String>,
}

/// `data.koru.do` is an alias for `us.data.koru.do` until EU exists, then
/// retired (D144's pattern). It is in the US set and in no other, because an
/// alias that resolved differently per region would be a second name for two
/// places.
const US_HOSTS: &[&str] = &["us.data.koru.do", "data.koru.do"];
const EU_HOSTS: &[&str] = &["eu.data.koru.do"];

impl ByteOrigins {
    /// The compiled-in set for one region. **`eu.data.koru.do` is the same
    /// worker in the EU deployment, trusting the EU hierarchy and nothing
    /// else**, so a US client never holds it: a capability minted in one
    /// region is refused by the other's worker anyway (C03/C04), and sending
    /// the bytes to find that out would already have sent them.
    pub fn for_region(region: Region) -> Self {
        let hosts = match region {
            Region::Us => US_HOSTS,
            Region::Eu => EU_HOSTS,
        };
        Self {
            origins: hosts.iter().map(|host| format!("https://{host}")).collect(),
        }
    }

    /// An empty set, for a test or a host that has not learned its region yet.
    /// Everything is refused, which is the safe direction: no transfer at all
    /// is better than one to an origin nobody chose.
    pub fn none() -> Self {
        Self {
            origins: BTreeSet::new(),
        }
    }

    /// Add the origins of a local stack, from the operator's own value.
    ///
    /// Comma-separated absolute origins — `http://127.0.0.1:8081` or
    /// `http://192.168.1.20:8081`. Each is admitted only if its host is a
    /// loopback or a private-range address, or the name `localhost`: a person
    /// testing two devices across a room needs a LAN address, and nothing here
    /// should let a misconfigured value point a fleet of clients at a public
    /// host over cleartext.
    ///
    /// A value that cannot be parsed is refused rather than ignored: an
    /// argument that is not understood is not run.
    pub fn with_development_origins(mut self, raw: &str) -> Result<Self, OriginRefused> {
        for entry in raw.split(',').map(str::trim).filter(|v| !v.is_empty()) {
            let url = Url::parse(entry).map_err(|_| OriginRefused::Unparseable)?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(OriginRefused::Scheme);
            }
            if url.path() != "/" && !url.path().is_empty() {
                return Err(OriginRefused::Shape);
            }
            if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
                return Err(OriginRefused::Shape);
            }
            let Some(host) = url.host() else {
                return Err(OriginRefused::Unparseable);
            };
            if !is_local(&host) {
                return Err(OriginRefused::NotAllowed);
            }
            self.origins.insert(origin_of(&url));
        }
        Ok(self)
    }

    /// Admit one URL a control response returned, and hand back the parsed
    /// value so a caller cannot accidentally use the unchecked string.
    ///
    /// The order is shape, then scheme, then membership — so a URL with a
    /// query on an allowed host is still refused, and a URL on a host nobody
    /// holds does not get to be reported as a scheme problem.
    ///
    /// **Every refusal names both sides**, via [`Self::refuse`]. A refusal
    /// that says only `byte_origin_refused` states that two strings differed
    /// and then withholds both of them; that is an afternoon of guessing, and
    /// it cost one on a physical phone where the answer turned out to be a
    /// port. The diagnostic is emitted here rather than at the three call
    /// sites because this is the only place holding the rejected URL and the
    /// admitted set at the same time — a call site that forgot it would be
    /// back to the afternoon.
    pub fn admit(&self, raw: &str) -> Result<AdmittedUrl, OriginRefused> {
        let Ok(url) = Url::parse(raw.trim()) else {
            return Err(self.refuse(raw, OriginRefused::Unparseable));
        };
        if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
            return Err(self.refuse(raw, OriginRefused::Shape));
        }
        if url.host().is_none() {
            return Err(self.refuse(raw, OriginRefused::Unparseable));
        }
        let origin = origin_of(&url);
        if !self.origins.contains(&origin) {
            // A cleartext URL on a host nobody holds is not a scheme problem
            // and not an allowlist problem separately; it is one refusal, and
            // naming the narrower one would be guessing at intent.
            return Err(self.refuse(
                raw,
                if url.scheme() == "https" {
                    OriginRefused::NotAllowed
                } else {
                    OriginRefused::Scheme
                },
            ));
        }
        Ok(AdmittedUrl(url))
    }

    /// Say out loud what was rejected and what would have been accepted.
    ///
    /// **`warn`, and unconditional.** A device declining to send somebody's
    /// file is never routine, and the two facts needed to act on it are the
    /// URL that arrived and the origins this device holds — the second of
    /// which exists nowhere else at runtime, since the production set is
    /// compiled in and the development one came from a `.env.local` the
    /// person may believe they set.
    ///
    /// The rejected value is **bounded and escaped**: it came from a control
    /// response, so it is precisely the string that must not be trusted, and
    /// a service answering with a megabyte of URL does not get to choose the
    /// size of a log line.
    fn refuse(&self, raw: &str, reason: OriginRefused) -> OriginRefused {
        let admitted = self.origins().collect::<Vec<_>>().join(", ");
        tracing::warn!(
            target: "koru_blob_sync",
            event = "cloud.blob.byte_origin_refused",
            code = "byte_origin_refused",
            reason = reason.message(),
            refused = %Rejected(raw),
            // Not a spelling mistake and not a missing setting: an empty set
            // means this device has not learned its region, so it holds
            // nothing and refuses everything. A different cause deserves
            // different words, or the reader spends the afternoon on the
            // `.env.local` that was never the problem.
            admits = %if admitted.is_empty() {
                "<none: this device holds no byte origins for its region>"
            } else {
                admitted.as_str()
            },
        );
        reason
    }

    /// What this device would admit, for the diagnostic. Origins only — there
    /// is nothing private in them and "the upload is stuck" has two very
    /// different causes once there are two services (§15).
    pub fn origins(&self) -> impl Iterator<Item = &str> {
        self.origins.iter().map(String::as_str)
    }
}

/// A refused byte URL on its way into a log line: bounded, and one line.
///
/// Untrusted text from a control response. A newline in it would forge a
/// second log record that reads as ours, and the length is not the service's
/// to choose — 256 characters is well past every URL this contract produces
/// and far short of anything that drowns a phone's logcat buffer.
struct Rejected<'a>(&'a str);

impl std::fmt::Display for Rejected<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const LIMIT: usize = 256;
        let trimmed = self.0.trim();
        if trimmed.is_empty() {
            // A control plane that returned no byte URL at all is a different
            // fault from one that returned the wrong host, and an empty field
            // reads as neither.
            return f.write_str("<empty>");
        }
        for character in trimmed.chars().take(LIMIT) {
            if character.is_control() {
                f.write_str("\u{fffd}")?;
            } else {
                write!(f, "{character}")?;
            }
        }
        if trimmed.chars().nth(LIMIT).is_some() {
            f.write_str("…")?;
        }
        Ok(())
    }
}

/// `scheme://host[:port]`, with the default port left off so one origin has
/// one spelling. Two spellings of one origin is two allowlist entries for one
/// place, and the day that matters is the day one of them is missing.
fn origin_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

/// Loopback, a private IPv4 range, a link-local or unique-local IPv6, or the
/// name `localhost` matched exactly — never as a suffix, or
/// `localhost.example.com` is a remote host wearing a local name.
fn is_local(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Host::Ipv6(address) => {
            address.is_loopback()
                // Unique-local (fc00::/7) and link-local (fe80::/10). Spelled
                // from the octets because the std predicates for both are
                // still unstable.
                || (address.octets()[0] & 0xfe) == 0xfc
                || (address.octets()[0] == 0xfe && (address.octets()[1] & 0xc0) == 0x80)
        }
    }
}

#[cfg(test)]
mod tests;
