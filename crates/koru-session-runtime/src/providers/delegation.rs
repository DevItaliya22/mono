// SPDX-License-Identifier: MPL-2.0

//! Whether a managed provider can be delegated to, and what its adapter
//! declares about the economics of a run.
//!
//! **A provider is delegatable if its adapter can reliably provide
//! structured lifecycle control** (D4). That is a seven-point contract, not
//! a provider allowlist: each requirement is a fact the adapter declares or
//! a capability it negotiated, the verdict is computed from them, and raw
//! PTY — which implements no [`super::AgentControl`] at all — is never
//! delegatable. Failing the verdict says nothing about the provider's
//! managed profile, which is a different gate with different evidence.

use koru_session_protocol::{CachedInputRelationship, CapabilitySet, ModelIdentity, capability};

/// Whether an adapter can continue a provider session it once ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeSupport {
    /// The adapter mints a resume token and honours it on a later start.
    Native,
    /// No native seam. The orchestrator re-submits the objective with the
    /// original pins and the artifacts produced so far, as a fresh attempt.
    /// Not a disqualification (§5, requirement 4).
    Unsupported,
}

/// Whether an adapter can hand its provider Koru's MCP endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpSupport {
    /// The adapter renders an endpoint and a tool allowlist into the
    /// provider's own configuration form.
    Supported,
    /// Nothing hands this provider an MCP server.
    Unsupported,
}

/// What the adapter can do about provider-native child sessions (§7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildSessionPolicy {
    /// Native children cannot yet be disabled or reported and terminated.
    /// Parent cancellation alone does not satisfy delegated cancellation.
    Uncontrolled,
    /// Explicitly admitted for supervised testing with native child control
    /// still unverified. Only parent cancellation/process ownership is known.
    Experimental,
    /// The provider can spawn its own agents and the adapter turns that off
    /// on a delegated launch.
    Disabled,
    /// The provider can spawn its own agents and the adapter has no switch;
    /// a `ChildSessionCreated` is flagged and the child terminated above.
    CannotDisable,
    /// The provider offers no native delegation to disable.
    NotOffered,
}

/// The shape of the provider's raw usage reports, before the adapter
/// normalizes them to one total per turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageReporting {
    /// The provider reports no token usage. Observable metrics — turns,
    /// tool calls, elapsed time, interventions, outcome — are still recorded
    /// above the adapter.
    None,
    /// Each report is an increment since the last one.
    Delta,
    /// Each report is a cumulative total the adapter differences per turn.
    RunningTotal,
    /// Each report is the turn's total so far and replaces the last.
    Replacement,
}

/// What the adapter declares about the numbers it reports (§17.2, §17.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicsDeclaration {
    pub usage: UsageReporting,
    pub cached_input: CachedInputRelationship,
    /// The model asked for and the model reported at start, with an explicit
    /// `None` on either side where the provider says nothing.
    pub model: ModelIdentity,
}

/// What an adapter declares about being delegated to. Every field is a
/// fact about the adapter, established by its compatibility note and
/// asserted by its offline replay; none is a promise about a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationFacts {
    /// The adapter starts the provider in the requested working directory.
    pub starts_in_cwd: bool,
    /// The adapter maps provider completion, failure and cancellation to
    /// distinct `TurnCompleted`, `TurnFailed` and `TurnCancelled` events.
    pub distinguishes_outcomes: bool,
    /// `submit` continues the live provider session with a follow-up turn.
    pub follows_up: bool,
    /// The adapter surfaces tool activity as `ToolStarted`/`ToolCompleted`.
    pub reports_tool_activity: bool,
    pub resume: ResumeSupport,
    pub mcp: McpSupport,
    pub child_sessions: ChildSessionPolicy,
    pub economics: EconomicsDeclaration,
}

impl DelegationFacts {
    /// What an adapter that has declared nothing amounts to: nothing. Used
    /// by fixtures that are not evaluating delegation.
    pub fn undeclared() -> Self {
        Self {
            starts_in_cwd: false,
            distinguishes_outcomes: false,
            follows_up: false,
            reports_tool_activity: false,
            resume: ResumeSupport::Unsupported,
            mcp: McpSupport::Unsupported,
            child_sessions: ChildSessionPolicy::NotOffered,
            economics: EconomicsDeclaration {
                usage: UsageReporting::None,
                cached_input: CachedInputRelationship::Undeclared,
                model: ModelIdentity::default(),
            },
        }
    }
}

/// The seven requirements of §5, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DelegationRequirement {
    /// 1. Start in a specified cwd.
    StartInCwd,
    /// 2. Structured messages, events and tool activity.
    StructuredEvents,
    /// 3. Explicitly detect success versus failure.
    DistinctOutcomes,
    /// 4. Follow-up, continue or resume where supported.
    FollowUp,
    /// 5. Cancel reliably.
    Cancel,
    /// 6. Enough structure for events and audit.
    AuditStructure,
    /// 7. Support Koru's MCP tools.
    McpTools,
}

impl DelegationRequirement {
    pub const ALL: [Self; 7] = [
        Self::StartInCwd,
        Self::StructuredEvents,
        Self::DistinctOutcomes,
        Self::FollowUp,
        Self::Cancel,
        Self::AuditStructure,
        Self::McpTools,
    ];
}

/// The computed verdict: which of the seven hold, and therefore whether the
/// provider is `Delegatable`. Experimental child policy admits supervised
/// testing without claiming verified cancellation of native children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationVerdict {
    met: [bool; 7],
}

impl DelegationVerdict {
    /// Evaluate an adapter's handshake. Two requirements are capability
    /// tokens the adapter negotiated; the other five are declared facts.
    pub fn evaluate(capabilities: &CapabilitySet, facts: &DelegationFacts) -> Self {
        Self {
            met: [
                facts.starts_in_cwd,
                capabilities.supports(capability::MANAGED_EVENTS),
                facts.distinguishes_outcomes,
                facts.follows_up,
                capabilities.supports(capability::MANAGED_CANCEL)
                    && facts.child_sessions != ChildSessionPolicy::Uncontrolled,
                capabilities.supports(capability::MANAGED_EVENTS) && facts.reports_tool_activity,
                facts.mcp == McpSupport::Supported,
            ],
        }
    }

    /// A raw PTY meets none of them: a run whose completion is a guess
    /// breaks the feature's only promise (D4).
    pub fn raw_pty() -> Self {
        Self { met: [false; 7] }
    }

    pub fn meets(&self, requirement: DelegationRequirement) -> bool {
        self.met[requirement as usize]
    }

    /// The requirements that do not hold, in §5's order.
    pub fn missing(&self) -> Vec<DelegationRequirement> {
        DelegationRequirement::ALL
            .into_iter()
            .filter(|requirement| !self.meets(*requirement))
            .collect()
    }

    /// `Delegatable`: granted only on all seven.
    pub fn delegatable(&self) -> bool {
        self.met.iter().all(|met| *met)
    }
}
