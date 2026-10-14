// SPDX-License-Identifier: MPL-2.0

//! The delegation family's vocabulary, protocol v14.
//!
//! Delegation is handing a piece of work to an agent and coming back to a
//! result. What lives here is only what the session wire needs to make that
//! recoverable and observable: the answer to a non-creating lookup of a
//! creation key, the reason a turn is being cancelled, where a controller
//! handover stands, and which model a session asked for and got. The run
//! itself — its document, its status, its history — is not session
//! vocabulary and is deliberately absent.

use std::collections::BTreeSet;
use std::{fmt, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{SessionSummary, TokenParseError};

/// The most tools one delegated launch may allow. A run gets a handful of
/// Koru tools, never a catalogue.
pub const MAX_MCP_TOOLS: usize = 32;
/// The longest endpoint or resume token the wire carries. Both are opaque
/// to Koru and neither is ever printed.
pub const MAX_OPAQUE_LAUNCH_BYTES: usize = 512;

fn valid_opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_OPAQUE_LAUNCH_BYTES
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Where a delegated provider reaches Koru's MCP tools.
///
/// An `http://` or `https://` URL the adapter renders privately into the
/// provider's own configuration form. It names a run's endpoint, so it is
/// redacted from `Debug` the way a lease token is: an id in a URL is still
/// an id somebody could replay. Neither quote character is allowed, so an
/// adapter can embed it in any configuration syntax without escaping.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct McpEndpoint(String);

impl McpEndpoint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for McpEndpoint {
    type Err = TokenParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let scheme_ok = value.starts_with("http://") || value.starts_with("https://");
        (scheme_ok && valid_opaque(value) && !value.contains(['"', '\'']))
            .then(|| Self(value.to_owned()))
            .ok_or(TokenParseError("invalid MCP endpoint"))
    }
}

impl TryFrom<String> for McpEndpoint {
    type Error = TokenParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<McpEndpoint> for String {
    fn from(value: McpEndpoint) -> Self {
        value.0
    }
}

impl fmt::Debug for McpEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("McpEndpoint(<redacted>)")
    }
}

/// The bearer a delegated provider presents at its MCP endpoint.
///
/// The run authority's endpoint token (`koru_run_…`, ADR-348), issued once
/// per attempt and rotated when the generation advances. Bounded and opaque
/// the way [`McpEndpoint`] is, and redacted from `Debug` — but held to a
/// stricter rule than the endpoint: **it is never rendered anywhere a
/// second process can read it.** An adapter puts it in the child's own
/// environment, never on argv, which every process on the machine can see,
/// and never in a file it leaves behind (ADR-359).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct McpBearer(String);

impl McpBearer {
    /// The one place the spelling leaves the type: an adapter handing it to
    /// the process it is about to start.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for McpBearer {
    type Err = TokenParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        valid_opaque(value)
            .then(|| Self(value.to_owned()))
            .ok_or(TokenParseError("invalid MCP bearer"))
    }
}

impl TryFrom<String> for McpBearer {
    type Error = TokenParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<McpBearer> for String {
    fn from(value: McpBearer) -> Self {
        value.0
    }
}

impl fmt::Debug for McpBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("McpBearer(<redacted>)")
    }
}

/// One MCP tool name a delegated provider may call: lowercase, digits and
/// underscores, as MCP tools are spelled.
#[derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct McpToolName(String);

impl McpToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for McpToolName {
    type Err = TokenParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        valid
            .then(|| Self(value.to_owned()))
            .ok_or(TokenParseError("invalid MCP tool name"))
    }
}

impl TryFrom<String> for McpToolName {
    type Error = TokenParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<McpToolName> for String {
    fn from(value: McpToolName) -> Self {
        value.0
    }
}

/// The bounded, duplicate-free set of tools one delegated launch allows.
/// Empty allows none, which is a legal launch: a run may be read-only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "Vec<McpToolName>", into = "Vec<McpToolName>")]
pub struct McpToolAllowlist(BTreeSet<McpToolName>);

impl McpToolAllowlist {
    pub fn new(names: impl IntoIterator<Item = McpToolName>) -> Result<Self, TokenParseError> {
        let set: BTreeSet<_> = names.into_iter().collect();
        (set.len() <= MAX_MCP_TOOLS)
            .then_some(Self(set))
            .ok_or(TokenParseError("too many MCP tools"))
    }

    pub fn iter(&self) -> impl Iterator<Item = &McpToolName> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<Vec<McpToolName>> for McpToolAllowlist {
    type Error = TokenParseError;

    fn try_from(value: Vec<McpToolName>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<McpToolAllowlist> for Vec<McpToolName> {
    fn from(value: McpToolAllowlist) -> Self {
        value.0.into_iter().collect()
    }
}

/// An adapter's opaque handle for resuming a provider session it once ran.
///
/// Bounded and never printed. Koru stores it beside a run's attempt and
/// hands it back unchanged; only the adapter that minted it can read it,
/// and an adapter that declares no native resume never mints one.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderResumeToken(String);

impl ProviderResumeToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ProviderResumeToken {
    type Err = TokenParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        valid_opaque(value)
            .then(|| Self(value.to_owned()))
            .ok_or(TokenParseError("invalid resume token"))
    }
}

impl TryFrom<String> for ProviderResumeToken {
    type Error = TokenParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ProviderResumeToken> for String {
    fn from(value: ProviderResumeToken) -> Self {
        value.0
    }
}

impl fmt::Debug for ProviderResumeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderResumeToken(<redacted>)")
    }
}

/// What a delegated launch hands the adapter beyond the profile: the MCP
/// endpoint it must render, the bearer that authenticates this attempt
/// there, the tools it may allow, and the resume token of the attempt it
/// continues, if any.
///
/// Its presence on a creation is what makes the creation a delegation. A
/// host that did not negotiate `delegation` refuses it rather than spawning
/// a provider that would never reach its tools.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegationLaunch {
    /// Explicit requested model; absent preserves the provider's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelName>,
    pub mcp_endpoint: McpEndpoint,
    /// The endpoint's bearer. Defaulted, so a peer that predates it reads
    /// the message: `None` is a launch whose endpoint refuses every call,
    /// which is what a host without a run authority could ever have offered
    /// anyway.
    #[serde(default)]
    pub credential: Option<McpBearer>,
    #[serde(default)]
    pub tools: McpToolAllowlist,
    #[serde(default)]
    pub resume: Option<ProviderResumeToken>,
}

impl fmt::Debug for DelegationLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelegationLaunch")
            .field("model_selected", &self.model.is_some())
            .field("tool_count", &self.tools.len())
            .field("authenticated", &self.credential.is_some())
            .field("resumes", &self.resume.is_some())
            .finish_non_exhaustive()
    }
}

/// The answer to a non-creating lookup of a creation key.
///
/// Retrying `create_profile_session` is not a safe way to ask "did it
/// start?": on a missing key the host falls through and spawns. This is the
/// question asked as a question. It never creates, never retires, and never
/// takes a lease — and it is paired with the host's `instance_id`, because
/// "no evidence" from the same host incarnation and "no evidence" from a
/// restarted one are different facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreationLookup {
    /// A live session carries the key: it started and is running.
    Known { session: SessionSummary },
    /// The key is reserved and insertion has not completed: a process may
    /// exist right now. Never the nothing-running branch.
    Creating,
    /// No live session, but the host retained the creation's outcome: it
    /// started and has finished.
    KnownEnded { session: SessionSummary },
    /// Nothing under this key. With a new `instance_id` the host restarted
    /// and nothing survived; with the same one, nothing is running.
    NoEvidence,
}

impl CreationLookup {
    /// Whether a caller must treat the creation as running: a live session,
    /// or a reservation that may already own a process.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Known { .. } | Self::Creating)
    }
}

/// Why a managed turn is being cancelled.
///
/// A takeover cancellation ends the step, not the job (D36). The reason
/// travels with the cancellation so the resulting `TurnCancelled` can say
/// which one it was, and a consumer that reads only the event still knows
/// whether the run ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// The run itself is being cancelled: terminal for the run.
    #[default]
    Run,
    /// The turn ends so control can be handed over: never terminal.
    TakeoverBoundary,
}

impl CancelReason {
    /// A run cancellation outranks a takeover boundary requested for the
    /// same turn; the reverse never happens, because a person who cancelled
    /// the run did not change their mind by also asking for control.
    pub fn strongest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Run, _) | (_, Self::Run) => Self::Run,
            _ => Self::TakeoverBoundary,
        }
    }
}

/// How soon a control request wants the lease.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverUrgency {
    /// Wait for the holder to release at the turn boundary.
    #[default]
    AtTurnBoundary,
    /// Cancel the active turn with [`CancelReason::TakeoverBoundary`] so the
    /// boundary arrives now. Still one rule: the request waits for the
    /// boundary; this only brings the boundary forward.
    Now,
}

/// The answer to a control request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlRequestOutcome {
    /// No controller is held: attach now.
    Available,
    /// A controller is held and has been told. The requester waits for the
    /// session's handover to read `Released`, then attaches.
    Waiting,
}

/// Where a controller handover stands.
///
/// Release, not preemption: nothing takes a lease from its holder. The
/// holder is asked, releases at a boundary of its choosing, the requester
/// attaches, and gives it back the same way. Each step is a state a reader
/// can see and an event a subscriber receives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandoverState {
    #[default]
    Idle,
    /// Someone asked while a controller was held; the holder has been told.
    Requested,
    /// The holder released its lease for the handover; the next attach takes
    /// it.
    Released,
    /// The lease was granted after a release: a person holds the session.
    Taken,
    /// The person gave the lease back, or lost it; the next attach reclaims.
    Returned,
}

impl HandoverState {
    /// The handover cycle, plus the two shortcuts a crash produces: a holder
    /// that vanishes while a request is pending has released, and a person
    /// whose window dies has returned.
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::Idle, Self::Requested | Self::Released)
                | (Self::Requested, Self::Released)
                | (Self::Released, Self::Taken | Self::Idle)
                | (Self::Taken, Self::Returned)
                | (Self::Returned, Self::Idle)
        )
    }
}

fn valid_model_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"')
}

/// A provider's model name, as the provider spells it.
///
/// Bounded and printable, and otherwise opaque: Koru compares it, stores it
/// beside a run's economics and prices it at display, but never interprets
/// it. It is not an event attribute — `AttrValue` has no free string, and
/// the bus carries an `Id` reference to retained metadata instead (§17.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct ModelName(String);

impl fmt::Debug for ModelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ModelName(<redacted>)")
    }
}

impl ModelName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModelName {
    type Err = TokenParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        valid_model_name(value)
            .then(|| Self(value.to_owned()))
            .ok_or(TokenParseError("invalid model name"))
    }
}

impl TryFrom<String> for ModelName {
    type Error = TokenParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ModelName> for String {
    fn from(value: ModelName) -> Self {
        value.0
    }
}

impl fmt::Display for ModelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The model a session asked for and the model the provider says it used.
///
/// They differ — a provider may reroute — and a provider may say nothing,
/// which is an explicit `None` rather than a copy of the request. Pricing
/// needs the reported one; a comparison across runs needs both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelIdentity {
    #[serde(default)]
    pub requested: Option<ModelName>,
    #[serde(default)]
    pub reported: Option<ModelName>,
}

/// Whether a provider's `input_tokens` already counts its cached input.
///
/// Declared by the adapter, never assumed: a wrong guess here misprices
/// every run silently. `Undeclared` is recorded as incomplete downstream,
/// which is the honest answer when the provider's note has not said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CachedInputRelationship {
    /// `cached_input_tokens` is a subset of `input_tokens`.
    Included,
    /// `cached_input_tokens` is counted beside `input_tokens`, not inside it.
    Separate,
    /// The adapter has not said. Recorded incomplete, never guessed.
    #[default]
    Undeclared,
}
