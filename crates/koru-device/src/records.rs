// SPDX-License-Identifier: MPL-2.0

use ed25519_dalek::{Signature, Verifier};
use koru_store_core::ProfileId;
use serde::{Deserialize, Serialize};
use uuid::{Variant, Version};

use crate::signing::Canonical;
use crate::{
    DeviceId, DevicePublicKey, DeviceSigningKey, RecordId, RelationshipId, ReplicationEndpointId,
    SignatureBytes,
};

pub const RECORD_VERSION: u32 = 1;

/// A device name is shown in lists on every mesh peer, so it stays short.
/// Both rename entry points (desktop Agent, mobile host) validate against
/// this before signing.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptorV1 {
    pub version: u32,
    pub device_id: DeviceId,
    pub display_name: String,
    pub signing_public_key: DevicePublicKey,
    pub endpoint_id: ReplicationEndpointId,
    pub created_at_millis: i64,
    pub endpoint_binding_signature: SignatureBytes,
}

impl DeviceDescriptorV1 {
    pub fn new(
        device_id: DeviceId,
        display_name: String,
        signing_key: &DeviceSigningKey,
        endpoint_id: ReplicationEndpointId,
        created_at_millis: i64,
    ) -> Self {
        let signing_public_key = signing_key.public_key();
        let bytes =
            endpoint_binding_bytes(device_id, signing_public_key, endpoint_id, RECORD_VERSION);
        Self {
            version: RECORD_VERSION,
            device_id,
            display_name,
            signing_public_key,
            endpoint_id,
            created_at_millis,
            endpoint_binding_signature: signing_key.sign(&bytes),
        }
    }

    pub fn verify(&self) -> Result<(), VerifyError> {
        require_version(self.version)?;
        verify_signature(
            self.signing_public_key,
            &endpoint_binding_bytes(
                self.device_id,
                self.signing_public_key,
                self.endpoint_id,
                self.version,
            ),
            self.endpoint_binding_signature,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointBindingV1 {
    pub version: u32,
    pub record_id: RecordId,
    pub device_id: DeviceId,
    pub device_public_key: DevicePublicKey,
    pub endpoint_id: ReplicationEndpointId,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl EndpointBindingV1 {
    pub fn sign(
        record_id: RecordId,
        device_id: DeviceId,
        key: &DeviceSigningKey,
        endpoint_id: ReplicationEndpointId,
        issued_at_millis: i64,
    ) -> Self {
        let device_public_key = key.public_key();
        let bytes = endpoint_binding_record_bytes(
            record_id,
            device_id,
            device_public_key,
            endpoint_id,
            issued_at_millis,
            RECORD_VERSION,
        );
        Self {
            version: RECORD_VERSION,
            record_id,
            device_id,
            device_public_key,
            endpoint_id,
            issued_at_millis,
            signature: key.sign(&bytes),
        }
    }

    pub fn verify(self) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        verify_signature(
            self.device_public_key,
            &self.signing_bytes(),
            self.signature,
        )?;
        Ok(VerifiedTrustRecord(TrustRecord::EndpointBinding(self)))
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        endpoint_binding_record_bytes(
            self.record_id,
            self.device_id,
            self.device_public_key,
            self.endpoint_id,
            self.issued_at_millis,
            self.version,
        )
    }
}

/// One directional authorization for exactly one Profile on one relationship.
/// Both relationship parties must sign opposite grants before the edge is
/// active; `relationship_id` is also the revocation/re-pair fence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileGrantV1 {
    pub version: u32,
    pub relationship_id: RelationshipId,
    pub record_id: RecordId,
    pub profile_id: ProfileId,
    pub grantee_device_id: DeviceId,
    pub issued_by: DeviceId,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl ProfileGrantV1 {
    pub fn sign(
        relationship_id: RelationshipId,
        record_id: RecordId,
        profile_id: ProfileId,
        grantee_device_id: DeviceId,
        issued_by: DeviceId,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self {
            version: RECORD_VERSION,
            relationship_id,
            record_id,
            profile_id,
            grantee_device_id,
            issued_by,
            issued_at_millis,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:profile-grant:v1\0")
            .u32(self.version)
            .relationship(self.relationship_id)
            .record(self.record_id)
            .variable(self.profile_id.to_storage().as_bytes())
            .device(self.grantee_device_id)
            .device(self.issued_by)
            .i64(self.issued_at_millis)
            .finish()
    }

    pub fn verify(self, issuer_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        verify_signature(issuer_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::ProfileGrant(self)))
    }
}

/// Hosted lifecycle authority for exactly one Profile, carried through the
/// existing signed trust-record channel. The hosted revision is the only
/// ordering input; `issued_at_millis` is display metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileLifecycleStateV1 {
    Active,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLifecycleV1 {
    pub version: u32,
    pub record_id: RecordId,
    pub issued_by: DeviceId,
    pub profile_id: ProfileId,
    pub state: ProfileLifecycleStateV1,
    pub lifecycle_revision: u64,
    pub operation_id: uuid::Uuid,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl ProfileLifecycleV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        record_id: RecordId,
        issued_by: DeviceId,
        profile_id: ProfileId,
        state: ProfileLifecycleStateV1,
        lifecycle_revision: u64,
        operation_id: uuid::Uuid,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self {
            version: RECORD_VERSION,
            record_id,
            issued_by,
            profile_id,
            state,
            lifecycle_revision,
            operation_id,
            issued_at_millis,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:profile-lifecycle:v1\0")
            .u32(self.version)
            .record(self.record_id)
            .device(self.issued_by)
            .variable(self.profile_id.to_storage().as_bytes())
            .u32(match self.state {
                ProfileLifecycleStateV1::Active => 0,
                ProfileLifecycleStateV1::Deleted => 1,
            })
            .u64(self.lifecycle_revision)
            .variable(self.operation_id.hyphenated().to_string().as_bytes())
            .i64(self.issued_at_millis)
            .finish()
    }

    pub fn verify(self, issuer_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        if self.lifecycle_revision == 0
            || self.operation_id.get_version() != Some(Version::Random)
            || self.operation_id.get_variant() != Variant::RFC4122
        {
            return Err(VerifyError::InvalidLifecycle);
        }
        verify_signature(issuer_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::ProfileLifecycle(self)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRelationshipV1 {
    pub version: u32,
    pub relationship_id: RelationshipId,
    pub inviter: DeviceDescriptorV1,
    pub joiner: DeviceDescriptorV1,
    pub pairing_transcript_hash: [u8; 32],
    pub created_at_millis: i64,
    pub inviter_signature: SignatureBytes,
    pub joiner_signature: SignatureBytes,
}

impl TrustRelationshipV1 {
    /// Build the canonical fact before either remote device signs it.
    pub fn unsigned(
        relationship_id: RelationshipId,
        inviter: DeviceDescriptorV1,
        joiner: DeviceDescriptorV1,
        pairing_transcript_hash: [u8; 32],
        created_at_millis: i64,
    ) -> Self {
        Self {
            version: RECORD_VERSION,
            relationship_id,
            inviter,
            joiner,
            pairing_transcript_hash,
            created_at_millis,
            inviter_signature: SignatureBytes::from_bytes([0; 64]),
            joiner_signature: SignatureBytes::from_bytes([0; 64]),
        }
    }

    /// Attach the signatures gathered independently from the two devices.
    pub fn with_signatures(
        mut self,
        inviter_signature: SignatureBytes,
        joiner_signature: SignatureBytes,
    ) -> Self {
        self.inviter_signature = inviter_signature;
        self.joiner_signature = joiner_signature;
        self
    }

    pub fn sign(
        relationship_id: RelationshipId,
        inviter: DeviceDescriptorV1,
        joiner: DeviceDescriptorV1,
        pairing_transcript_hash: [u8; 32],
        created_at_millis: i64,
        inviter_key: &DeviceSigningKey,
        joiner_key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self::unsigned(
            relationship_id,
            inviter,
            joiner,
            pairing_transcript_hash,
            created_at_millis,
        );
        let bytes = value.signing_bytes();
        value.inviter_signature = inviter_key.sign(&bytes);
        value.joiner_signature = joiner_key.sign(&bytes);
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:trust-relationship:v1\0")
            .u32(self.version)
            .relationship(self.relationship_id)
            .device(self.inviter.device_id)
            .public_key(self.inviter.signing_public_key)
            .endpoint(self.inviter.endpoint_id)
            .device(self.joiner.device_id)
            .public_key(self.joiner.signing_public_key)
            .endpoint(self.joiner.endpoint_id)
            .fixed32(self.pairing_transcript_hash)
            .i64(self.created_at_millis)
            .finish()
    }

    pub fn verify(self) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        self.inviter.verify()?;
        self.joiner.verify()?;
        if self.inviter.device_id == self.joiner.device_id {
            return Err(VerifyError::InvalidRelationship);
        }
        let bytes = self.signing_bytes();
        verify_signature(
            self.inviter.signing_public_key,
            &bytes,
            self.inviter_signature,
        )?;
        verify_signature(
            self.joiner.signing_public_key,
            &bytes,
            self.joiner_signature,
        )?;
        Ok(VerifiedTrustRecord(TrustRecord::Relationship(self)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllDataGrantV1 {
    pub version: u32,
    pub relationship_id: RelationshipId,
    pub device_id: DeviceId,
    pub issued_by: DeviceId,
    pub record_id: RecordId,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl AllDataGrantV1 {
    pub fn sign(
        relationship_id: RelationshipId,
        device_id: DeviceId,
        issued_by: DeviceId,
        record_id: RecordId,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self {
            version: RECORD_VERSION,
            relationship_id,
            device_id,
            issued_by,
            record_id,
            issued_at_millis,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:all-data-grant:v1\0")
            .u32(self.version)
            .relationship(self.relationship_id)
            .device(self.device_id)
            .device(self.issued_by)
            .record(self.record_id)
            .i64(self.issued_at_millis)
            .finish()
    }

    pub fn verify(self, issuer_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        verify_signature(issuer_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::AllDataGrant(self)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceNameV1 {
    pub version: u32,
    pub record_id: RecordId,
    pub device_id: DeviceId,
    pub display_name: String,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl DeviceNameV1 {
    /// A rename is self-signed by the device being named and gossips like any
    /// other record. Only one identity can produce them, so ordering is that
    /// identity's own clock (with the record id as the deterministic
    /// tie-break); this resolves *display metadata*, never authority between
    /// parties, which is the case the no-wall-clock rule exists for.
    pub fn sign(
        record_id: RecordId,
        device_id: DeviceId,
        display_name: String,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self {
            version: RECORD_VERSION,
            record_id,
            device_id,
            display_name,
            issued_at_millis,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:device-name:v1\0")
            .u32(self.version)
            .record(self.record_id)
            .device(self.device_id)
            .variable(self.display_name.as_bytes())
            .i64(self.issued_at_millis)
            .finish()
    }

    pub fn verify(self, device_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        verify_signature(device_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::DeviceName(self)))
    }
}

/// A build record's payload is signed as the bytes it was written with, and
/// a reader parses only what it knows, so it can grow without an older peer
/// losing the signature or the fields it understands. This bounds the bytes
/// the mesh will carry for one device's self-description.
pub const MAX_BUILD_PAYLOAD_BYTES: usize = 2048;

/// Each string inside a [`BuildPayload`] is shown in a list row or a log
/// line, so it stays short — the device-name rule, per field.
pub const MAX_BUILD_FIELD_CHARS: usize = 64;

/// What a device says about the software it runs. Every field is optional on
/// read and unknown fields are ignored, which is what lets a newer device add
/// one without an older reader rejecting the record.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildPayload {
    /// "0.0.3-nightly.20260816+…" on desktop; Gradle's "0.1.0 (1)" on Android.
    pub app_version: String,
    /// A release channel on desktop; Android's build type.
    pub channel: String,
    /// Dev builds only — a released desktop build deliberately carries none.
    pub branch: Option<String>,
    pub commit: Option<String>,
    /// Short, for a list row: "Windows 11", "Android 14".
    pub os: Option<String>,
    /// Full, for a log line: "10.0.26200", "14 (API 34)".
    pub os_version: Option<String>,
    /// The peer protocol number this build speaks
    /// (`koru_replication::PEER_PROTOCOL_VERSION`). Two devices on different
    /// numbers do not sync; one of them has to update. Absent from a device
    /// built before the number existed.
    pub peer_protocol: Option<u32>,
}

/// How two devices' builds relate, most serious first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildDrift {
    /// Different peer protocol numbers: sync is refused and one side must
    /// update — the lower number, by convention.
    Incompatible {
        ours: u32,
        theirs: u32,
    },
    Major,
    Minor,
    Patch,
    Same,
    /// Either version does not start with `MAJOR.MINOR.PATCH`.
    Unknown,
}

/// How loudly a drift is shown: nothing, the notice tone, or red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftSeverity {
    None,
    Warning,
    Danger,
}

impl BuildDrift {
    /// Patch drift is compatible by semver's promise and is not shown; minor
    /// drift is a warning; a major bump or an incompatible protocol is red.
    pub fn severity(self) -> DriftSeverity {
        match self {
            Self::Incompatible { .. } | Self::Major => DriftSeverity::Danger,
            Self::Minor => DriftSeverity::Warning,
            Self::Patch | Self::Same | Self::Unknown => DriftSeverity::None,
        }
    }

    pub fn is_incompatible(self) -> bool {
        matches!(self, Self::Incompatible { .. })
    }

    /// Which side has to update when the two do not sync: the one on the
    /// lower number. `"peer"` or `"this_device"`, from the point of view of
    /// the device whose payload `drift_from` was called on.
    pub fn update_needed_on(self) -> Option<&'static str> {
        match self {
            Self::Incompatible { ours, theirs } => {
                Some(if theirs < ours { "peer" } else { "this_device" })
            }
            _ => None,
        }
    }

    /// The wire and log word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incompatible { .. } => "incompatible",
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Patch => "patch",
            Self::Same => "same",
            Self::Unknown => "unknown",
        }
    }
}

impl DriftSeverity {
    /// The wire word; `None` when there is nothing to mark.
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Warning => Some("warning"),
            Self::Danger => Some("danger"),
        }
    }
}

impl BuildPayload {
    /// The one line every surface prints: version, channel, then branch and
    /// OS when known. Tokens only, so there is nothing to localize.
    pub fn label(&self) -> String {
        let mut label = format!("{} · {}", self.app_version, self.channel);
        for extra in [&self.branch, &self.os].into_iter().flatten() {
            label.push_str(" · ");
            label.push_str(extra);
        }
        label
    }

    /// The leading `MAJOR.MINOR.PATCH` of `app_version`, which is all the
    /// comparison reads: Gradle's "0.1.0 (1)", cargo's "0.1.0" and a release's
    /// "0.0.3-nightly.20260816+…" all start the same way, and what follows
    /// the three numbers is a build's own business.
    pub fn core_version(&self) -> Option<(u64, u64, u64)> {
        let head: String = self
            .app_version
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = head.split('.').map(|part| part.parse::<u64>().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    }

    /// How this build relates to another's, in the meaning semver gives the
    /// three numbers — after the one check that overrides them: two devices
    /// on different peer protocol numbers do not sync at all, whatever their
    /// versions say. A device that predates the number is not locked out by
    /// the build that introduced it — its versions are compared as usual.
    /// Channel, branch, commit and OS never take part.
    pub fn drift_from(&self, other: &Self) -> BuildDrift {
        if let (Some(ours), Some(theirs)) = (self.peer_protocol, other.peer_protocol)
            && ours != theirs
        {
            return BuildDrift::Incompatible { ours, theirs };
        }
        let (Some(mine), Some(theirs)) = (self.core_version(), other.core_version()) else {
            return BuildDrift::Unknown;
        };
        if mine.0 != theirs.0 {
            BuildDrift::Major
        } else if mine.1 != theirs.1 {
            BuildDrift::Minor
        } else if mine.2 != theirs.2 {
            BuildDrift::Patch
        } else {
            BuildDrift::Same
        }
    }

    fn within_limits(&self) -> bool {
        [
            Some(&self.app_version),
            Some(&self.channel),
            self.branch.as_ref(),
            self.commit.as_ref(),
            self.os.as_ref(),
            self.os_version.as_ref(),
        ]
        .into_iter()
        .flatten()
        .all(|field| field.chars().count() <= MAX_BUILD_FIELD_CHARS)
    }
}

/// A device's self-description of its build: self-signed, gossiped, and
/// superseded by the next one the same device signs. See [`BuildPayload`]
/// for why the payload travels as the JSON text it was signed as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBuildV1 {
    pub version: u32,
    pub record_id: RecordId,
    pub device_id: DeviceId,
    pub issued_at_millis: i64,
    pub payload: String,
    pub signature: SignatureBytes,
}

impl DeviceBuildV1 {
    /// Same ordering rule as a rename: one identity produces these, so its
    /// own clock orders them and the record id breaks ties.
    pub fn sign(
        record_id: RecordId,
        device_id: DeviceId,
        payload: &BuildPayload,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let payload = serde_json::to_string(payload).unwrap_or_default();
        let mut value = Self {
            version: RECORD_VERSION,
            record_id,
            device_id,
            issued_at_millis,
            payload,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    /// The payload as far as this build understands it; `None` only when the
    /// text is not a JSON object at all.
    pub fn payload(&self) -> Option<BuildPayload> {
        serde_json::from_str(&self.payload).ok()
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:device-build:v1\0")
            .u32(self.version)
            .record(self.record_id)
            .device(self.device_id)
            .i64(self.issued_at_millis)
            .variable(self.payload.as_bytes())
            .finish()
    }

    pub fn verify(self, device_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        if self.payload.len() > MAX_BUILD_PAYLOAD_BYTES {
            return Err(VerifyError::Oversized);
        }
        let payload = self.payload().ok_or(VerifyError::InvalidPayload)?;
        if !payload.within_limits() {
            return Err(VerifyError::Oversized);
        }
        verify_signature(device_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::DeviceBuild(self)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationV1 {
    pub version: u32,
    pub relationship_id: RelationshipId,
    pub revoked_device_id: DeviceId,
    pub issued_by: DeviceId,
    pub record_id: RecordId,
    pub issued_at_millis: i64,
    pub signature: SignatureBytes,
}

impl RevocationV1 {
    pub fn sign(
        relationship_id: RelationshipId,
        revoked_device_id: DeviceId,
        issued_by: DeviceId,
        record_id: RecordId,
        issued_at_millis: i64,
        key: &DeviceSigningKey,
    ) -> Self {
        let mut value = Self {
            version: RECORD_VERSION,
            relationship_id,
            revoked_device_id,
            issued_by,
            record_id,
            issued_at_millis,
            signature: SignatureBytes::from_bytes([0; 64]),
        };
        value.signature = key.sign(&value.signing_bytes());
        value
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        Canonical::new(b"koru:revocation:v1\0")
            .u32(self.version)
            .relationship(self.relationship_id)
            .device(self.revoked_device_id)
            .device(self.issued_by)
            .record(self.record_id)
            .i64(self.issued_at_millis)
            .finish()
    }

    pub fn verify(self, issuer_key: DevicePublicKey) -> Result<VerifiedTrustRecord, VerifyError> {
        require_version(self.version)?;
        verify_signature(issuer_key, &self.signing_bytes(), self.signature)?;
        Ok(VerifiedTrustRecord(TrustRecord::Revocation(self)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrustRecord {
    EndpointBinding(EndpointBindingV1),
    ProfileGrant(ProfileGrantV1),
    ProfileLifecycle(ProfileLifecycleV1),
    Relationship(TrustRelationshipV1),
    AllDataGrant(AllDataGrantV1),
    Revocation(RevocationV1),
    DeviceName(DeviceNameV1),
    DeviceBuild(DeviceBuildV1),
    /// A kind this build does not know. It exists so that a gossip batch from
    /// a newer device decodes — the unknown record is refused on its own by
    /// [`crate::verify_gossip_record`] and the rest of the batch survives —
    /// which is what makes adding a record kind free from here on. Nothing
    /// persists or re-sends one.
    #[serde(other)]
    Unknown,
}

impl TrustRecord {
    pub fn record_id(&self) -> RecordId {
        match self {
            Self::EndpointBinding(v) => v.record_id,
            Self::ProfileGrant(v) => v.record_id,
            Self::ProfileLifecycle(v) => v.record_id,
            Self::Relationship(v) => RecordId::from_uuid(v.relationship_id.as_uuid()),
            Self::AllDataGrant(v) => v.record_id,
            Self::Revocation(v) => v.record_id,
            Self::DeviceName(v) => v.record_id,
            Self::DeviceBuild(v) => v.record_id,
            Self::Unknown => RecordId::from_uuid(uuid::Uuid::nil()),
        }
    }

    /// Canonical send order for replicating the device list itself:
    /// relationships carry the descriptors every later record verifies
    /// against, so they always travel first.
    pub fn gossip_rank(&self) -> u8 {
        match self {
            Self::Relationship(_) => 0,
            Self::AllDataGrant(_) => 1,
            Self::Revocation(_) => 2,
            Self::ProfileGrant(_) => 3,
            Self::ProfileLifecycle(_) => 4,
            Self::EndpointBinding(_) => 5,
            Self::DeviceName(_) => 6,
            Self::DeviceBuild(_) => 7,
            Self::Unknown => u8::MAX,
        }
    }
}

/// Sort a trust-record set so a receiving device can verify it in one pass.
pub fn sort_for_gossip(records: &mut [TrustRecord]) {
    records.sort_by_key(|record| (record.gossip_rank(), record.record_id()));
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedTrustRecord(pub(crate) TrustRecord);

impl VerifiedTrustRecord {
    pub fn record(&self) -> &TrustRecord {
        &self.0
    }
    pub fn into_record(self) -> TrustRecord {
        self.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    UnsupportedVersion,
    InvalidSignature,
    InvalidKey,
    InvalidRelationship,
    /// A build payload over its byte or per-field limits.
    Oversized,
    /// A build payload that is not a JSON object.
    InvalidPayload,
    /// A lifecycle fact has no hosted mutation revision or a non-v4 operation id.
    InvalidLifecycle,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::UnsupportedVersion => "unsupported_record_version",
                Self::InvalidSignature => "invalid_signature",
                Self::InvalidKey => "invalid_public_key",
                Self::InvalidRelationship => "invalid_relationship",
                Self::Oversized => "oversized_record",
                Self::InvalidPayload => "invalid_payload",
                Self::InvalidLifecycle => "invalid_profile_lifecycle",
            }
        )
    }
}

impl std::error::Error for VerifyError {}

fn require_version(version: u32) -> Result<(), VerifyError> {
    (version == RECORD_VERSION)
        .then_some(())
        .ok_or(VerifyError::UnsupportedVersion)
}

fn verify_signature(
    key: DevicePublicKey,
    bytes: &[u8],
    signature: SignatureBytes,
) -> Result<(), VerifyError> {
    let key = key.verifying_key().map_err(|_| VerifyError::InvalidKey)?;
    key.verify(bytes, &Signature::from_bytes(signature.as_bytes()))
        .map_err(|_| VerifyError::InvalidSignature)
}

fn endpoint_binding_bytes(
    device_id: DeviceId,
    key: DevicePublicKey,
    endpoint: ReplicationEndpointId,
    version: u32,
) -> Vec<u8> {
    Canonical::new(b"koru:endpoint-binding:v1\0")
        .u32(version)
        .device(device_id)
        .public_key(key)
        .endpoint(endpoint)
        .finish()
}

fn endpoint_binding_record_bytes(
    record_id: RecordId,
    device_id: DeviceId,
    key: DevicePublicKey,
    endpoint: ReplicationEndpointId,
    issued_at_millis: i64,
    version: u32,
) -> Vec<u8> {
    Canonical::new(b"koru:endpoint-binding-record:v1\0")
        .u32(version)
        .record(record_id)
        .device(device_id)
        .public_key(key)
        .endpoint(endpoint)
        .i64(issued_at_millis)
        .finish()
}
