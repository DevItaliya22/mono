// SPDX-License-Identifier: MPL-2.0

use std::str::FromStr;

use uuid::Version;

use crate::*;

macro_rules! id_contract {
    ($name:ident, $type:ty) => {
        #[test]
        fn $name() {
            let minted = <$type>::mint();
            assert_eq!(minted.as_uuid().get_version(), Some(Version::Random));
            let text = minted.to_string();
            assert_eq!(text.parse::<$type>().unwrap(), minted);
            assert_eq!(
                serde_json::to_string(&minted).unwrap(),
                format!("\"{text}\"")
            );
            assert!(<$type>::from_str("00000000-0000-0000-0000-000000000000").is_err());
            assert!(<$type>::from_str("11111111111141118111111111111111").is_err());
            assert!(<$type>::from_str("11111111-1111-4111-8111-11111111111A").is_err());
            assert!(<$type>::from_str("11111111-1111-7111-8111-111111111111").is_err());
        }
    };
}

id_contract!(session_ids_are_strict_v4, SessionId);
id_contract!(profile_ids_are_strict_v4, SessionProfileId);
id_contract!(attachment_ids_are_strict_v4, AttachmentId);
id_contract!(turn_ids_are_strict_v4, AgentTurnId);
id_contract!(idempotency_keys_are_strict_v4, IdempotencyKey);
id_contract!(approval_ids_are_strict_v4, ApprovalId);
id_contract!(tool_call_ids_are_strict_v4, ToolCallId);

#[test]
fn lease_tokens_are_strict_and_redacted() {
    let lease = ControllerLeaseToken::mint();
    assert!(!format!("{lease:?}").contains(&lease.to_string_for_test()));
    assert!(ControllerLeaseToken::from_str("00000000-0000-0000-0000-000000000000").is_err());
    let json = serde_json::to_string(&lease).unwrap();
    assert_eq!(
        serde_json::from_str::<ControllerLeaseToken>(&json).unwrap(),
        lease
    );
}

trait LeaseTestString {
    fn to_string_for_test(&self) -> String;
}

impl LeaseTestString for ControllerLeaseToken {
    fn to_string_for_test(&self) -> String {
        serde_json::from_str(&serde_json::to_string(self).unwrap()).unwrap()
    }
}
