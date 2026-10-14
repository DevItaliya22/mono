// SPDX-License-Identifier: MPL-2.0

//! Email sign-in's domain: the issuer, the two digests, the code, and the
//! rules a challenge is judged by.
//!
//! Everything here is pure. The repository stores what these functions decide
//! and the API sequences them; no I/O and no clock of its own.

use zeroize::Zeroize;

use crate::{CryptoError, Entropy, KeyedDigest, UnixMillis};

/// The issuer half of every email identity's `(issuer, subject)` key.
///
/// Stable storage, and the only way the repository ever looks an auth identity
/// up. Deliberately a URN and not a URL: it is not dereferenceable, and it must
/// never track `KORU_PUBLIC_API_ORIGIN` — an issuer carrying a hostname reads
/// as wrong the day the API moves, and "fixing" it would strand every row
/// written with the old one. Google sign-in has to accept both a current and
/// a legacy issuer, with a normalizer between them, because exactly that drift
/// has already happened once.
///
/// Changing it later is recoverable but deliberate: every email row also
/// carries `provider = 'email'`, so it is one `UPDATE` in one migration. A
/// tidy-up, not a trap — and not a thing to do casually.
pub const KORU_EMAIL_ISSUER: &str = "urn:koru:auth:email";

/// Domain separation for the address digest. Changing this string orphans
/// every `email_credentials.address_digest` already written, which is the same
/// class of change as [`KORU_EMAIL_ISSUER`] and needs the same care.
pub const EMAIL_ADDRESS_DIGEST_PURPOSE: &str = "account.email-address.v1";

/// Domain separation for the code digest. Cheap to change: challenges live ten
/// minutes, so a deployment that changes it invalidates only the codes in
/// flight.
pub const EMAIL_CODE_DIGEST_PURPOSE: &str = "account.email-code.v1";

/// Six digits, ten minutes, five wrong guesses, sixty seconds between sends.
///
/// Five guesses against a million codes is a 1-in-200,000 chance before the
/// challenge dies, which is what makes six digits enough — and six digits is
/// what a person can carry from their phone to their laptop without a mistake.
pub const CODE_DIGITS: u32 = 6;
pub const CODE_LIFETIME_MILLIS: UnixMillis = 10 * 60 * 1_000;
pub const MAX_CODE_ATTEMPTS: i32 = 5;
pub const RESEND_COOLDOWN_MILLIS: UnixMillis = 60 * 1_000;

/// The longest address any RFC permits, and the bound `resolver.rs`
/// (`MAX_EMAIL_BYTES`) already applies to a provider's verified email.
pub const MAX_ADDRESS_BYTES: usize = 320;

/// An address in the one form Koru stores.
#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedAddress(String);

impl NormalizedAddress {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// Redacted: an address is personal data, and this type exists precisely so it
/// travels through code that must not log it.
impl std::fmt::Debug for NormalizedAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NormalizedAddress(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressRejection {
    Empty,
    TooLong,
    /// Not one local part and one domain, or whitespace inside, or a domain
    /// with no dot.
    Shape,
}

/// Put an address in the one form Koru compares and digests.
///
/// **`trim().to_lowercase()`, and not one step more, because that is exactly
/// what the provider path already does** (`normalize_and_validate` in
/// `resolver.rs`, and the hosted service refuses anything else).
/// `auth_identities.verified_email_normalized` is what `classify()` reads to
/// find an existing account at an address, so if email sign-up normalized even
/// slightly differently — NFC, punycode, anything — the same address written by
/// the two paths would be two different strings, `classify()` would report
/// `Unseen`, and the same person would get a second Koru account. That is the
/// exact failure the collision rules exist to prevent, arriving through the
/// back door of a normalization mismatch.
///
/// So this deliberately does **not**:
///
/// - strip `+tags` or dots, which would map two addresses a provider treats as
///   different onto one credential, and differs per provider anyway;
/// - convert an internationalized domain to punycode, which would need a new
///   dependency and diverge from the Google form;
/// - NFC-normalize, for the same divergence reason. (A password is normalized,
///   because there both spellings must hash alike and nothing else compares
///   them. An address is compared against a string another code path wrote.)
///
/// The shape check is deliberately loose. Refusing a technically-valid address
/// is worse than accepting an undeliverable one: an address only becomes an
/// account after a code sent to it comes back, so delivery is the real
/// validator and this only rejects what cannot be an address at all.
pub fn normalize_address(raw: &str) -> Result<NormalizedAddress, AddressRejection> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AddressRejection::Empty);
    }
    if trimmed.len() > MAX_ADDRESS_BYTES {
        return Err(AddressRejection::TooLong);
    }
    let normalized = trimmed.to_lowercase();
    // Re-check after lowercasing: a few characters grow a byte when lowercased,
    // and the stored column is bounded by the post-normalization length.
    if normalized.len() > MAX_ADDRESS_BYTES {
        return Err(AddressRejection::TooLong);
    }
    let mut parts = normalized.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(AddressRejection::Shape);
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || normalized.chars().any(char::is_whitespace)
    {
        return Err(AddressRejection::Shape);
    }
    Ok(NormalizedAddress(normalized))
}

/// A verification code, in memory only.
///
/// Never stored, never logged, never returned over the wire — only the keyed
/// digest is persisted. It zeroizes on drop and its `Debug` is redacted, so a
/// stray `{:?}` in a trace cannot print it.
#[derive(Clone, PartialEq, Eq)]
pub struct VerificationCode(String);

impl VerificationCode {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Accept what a person typed: digits only, spaces and dashes forgiven.
    ///
    /// People paste `123 456` out of a mail client, and refusing that is a
    /// support ticket for no security gain — the code is the same code.
    pub fn from_input(raw: &str) -> Option<Self> {
        let digits: String = raw
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        (digits.len() == CODE_DIGITS as usize && digits.chars().all(|c| c.is_ascii_digit()))
            .then_some(Self(digits))
    }
}

impl std::fmt::Debug for VerificationCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerificationCode(<redacted>)")
    }
}

impl Drop for VerificationCode {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Mint a code through the entropy port.
///
/// Eight bytes reduced modulo a million. The modulo bias is `10^6 / 2^64`,
/// which is about one part in eighteen thousand million million — far below
/// anything that matters against a five-attempt bound, and simpler to read
/// than rejection sampling, which is the other correct answer.
pub fn generate_code(entropy: &dyn Entropy) -> Result<VerificationCode, CryptoError> {
    let mut bytes = [0_u8; 8];
    entropy.fill(&mut bytes)?;
    let value = u64::from_le_bytes(bytes) % 10_u64.pow(CODE_DIGITS);
    bytes.zeroize();
    Ok(VerificationCode(format!(
        "{value:0width$}",
        width = CODE_DIGITS as usize
    )))
}

/// What a challenge looks like to the rules below. The repository fills this
/// from `email_challenges` joined to its flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeState {
    /// The address this challenge was raised for, keyed. Callers compare a
    /// supplied address against it rather than the flow storing one.
    pub address_digest: KeyedDigest,
    pub code_digest: KeyedDigest,
    pub attempts: i32,
    pub last_sent_at_millis: UnixMillis,
    pub expires_at_millis: UnixMillis,
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeVerdict {
    Accepted,
    /// Wrong, and the challenge is still alive.
    Wrong {
        attempts_left: i32,
    },
    /// The fifth wrong guess. The challenge is dead and a new code must be
    /// requested — this is a terminal state, not a slower retry.
    Exhausted,
    Expired,
    /// Already used. A one-use code that is replayed is never accepted, even
    /// within its lifetime.
    AlreadyConsumed,
}

/// Judge one attempt.
///
/// **Order matters and is not arbitrary.** Consumption and expiry are checked
/// before the digest, so a replayed or stale code cannot spend an attempt — and
/// more importantly cannot be distinguished by *which* rejection it gets on the
/// strength of a correct guess. The attempt bound is checked before the
/// comparison for the same reason: past five, the answer is `Exhausted`
/// whether the sixth guess was right or wrong.
pub fn verify_code(
    state: &ChallengeState,
    supplied_digest: &KeyedDigest,
    now_millis: UnixMillis,
) -> CodeVerdict {
    if state.consumed {
        return CodeVerdict::AlreadyConsumed;
    }
    if now_millis >= state.expires_at_millis {
        return CodeVerdict::Expired;
    }
    if state.attempts >= MAX_CODE_ATTEMPTS {
        return CodeVerdict::Exhausted;
    }
    if constant_time_eq(&state.code_digest.0, &supplied_digest.0) {
        return CodeVerdict::Accepted;
    }
    let attempts_left = MAX_CODE_ATTEMPTS - (state.attempts + 1);
    if attempts_left <= 0 {
        CodeVerdict::Exhausted
    } else {
        CodeVerdict::Wrong { attempts_left }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResendVerdict {
    Send,
    TooSoon { retry_after_millis: UnixMillis },
}

/// Whether another code may be sent yet.
///
/// The cooldown is what stops our own resend button being used to flood a
/// stranger's inbox, so it is measured from the last *send* rather than from
/// the last request.
pub fn resend_decision(state: &ChallengeState, now_millis: UnixMillis) -> ResendVerdict {
    let ready_at = state
        .last_sent_at_millis
        .saturating_add(RESEND_COOLDOWN_MILLIS);
    if now_millis >= ready_at {
        ResendVerdict::Send
    } else {
        ResendVerdict::TooSoon {
            retry_after_millis: ready_at - now_millis,
        }
    }
}

/// Compare two digests without leaking where they first differ.
///
/// An attacker does not control a keyed digest directly, so this is defence in
/// depth rather than the primary barrier — but it costs three lines.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// What became of one message, in the only terms the domain may know.
///
/// Deliberately not a vendor response: the core decides whether to retry from
/// this, and a JMAP error type or an HTTP body in here would put provider
/// vocabulary into domain decisions — and into logs that must not carry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    /// Quota, rate limit, a timeout, a 5xx. Worth another attempt.
    RetryableFailure,
    /// A rejected sender, a malformed recipient, a bad credential. The next
    /// attempt fails the same way, and usually every other message is failing
    /// too — so this is the one a deployment needs told about.
    PermanentFailure,
}

/// One rendered message, ready to send.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundMail {
    pub to: NormalizedAddress,
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl std::fmt::Debug for OutboundMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The body carries the code and the recipient is personal data, so
        // there is nothing here a log may have beyond the fact of a message.
        f.write_str("OutboundMail(<redacted>)")
    }
}

/// Whether a message was taken for delivery.
///
/// Not whether it arrived: the caller is a request handler and must not wait
/// on the network, so the most it can honestly learn is that the message was
/// accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailAccepted {
    Queued,
    /// The queue is full, which means either an outage or an attack. The
    /// caller must **not** report success: a person sent to a code-entry
    /// screen for a code that will never arrive is stuck, where a person told
    /// to try again in a moment is not.
    Saturated,
}

/// Handing a message over for delivery.
///
/// **This is the port a request handler calls, and it does not send.** It
/// takes the message and returns immediately; delivery happens after the
/// response has gone out. That split is the whole reason there are two traits
/// here: a handler that could accidentally call [`MailTransport::deliver`]
/// would be one `await` away from putting a Fastmail round trip inside a
/// person's sign-up.
pub trait Mailer: Send + Sync {
    fn enqueue(&self, mail: OutboundMail) -> MailAccepted;
}

/// Actually putting one message on the wire.
///
/// Implemented by the transport and called only by whatever drains the queue.
/// It returns a [`DeliveryOutcome`] and never a vendor error, so the adapter
/// owns every detail of the protocol and the domain owns the decision about
/// what to do next.
pub trait MailTransport: Send + Sync {
    fn deliver(&self, mail: &OutboundMail) -> DeliveryOutcome;
}

#[cfg(test)]
mod tests;
