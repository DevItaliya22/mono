// SPDX-License-Identifier: MPL-2.0

use super::*;
use loro::{ExportMode, LoroDoc};

#[test]
fn directions_use_semantic_inclusion_for_equal_ahead_behind_and_concurrent_versions() {
    let local = LoroDoc::new();
    let remote = LoroDoc::new();
    assert_eq!(
        missing(&local.oplog_vv(), &remote.oplog_vv()),
        Missing {
            download: false,
            upload: false
        }
    );
    local.set_peer_id(1).unwrap();
    local.get_text("text").insert(0, "local").unwrap();
    local.commit();
    assert_eq!(
        missing(&local.oplog_vv(), &remote.oplog_vv()),
        Missing {
            download: false,
            upload: true
        }
    );
    assert_eq!(
        missing(&remote.oplog_vv(), &local.oplog_vv()),
        Missing {
            download: true,
            upload: false
        }
    );
    remote.set_peer_id(2).unwrap();
    remote.get_text("text").insert(0, "remote").unwrap();
    remote.commit();
    assert_eq!(
        missing(&local.oplog_vv(), &remote.oplog_vv()),
        Missing {
            download: true,
            upload: true
        }
    );
    remote
        .import(&local.export(ExportMode::snapshot()).unwrap())
        .unwrap();
    local
        .import(&remote.export(ExportMode::snapshot()).unwrap())
        .unwrap();
    assert_eq!(
        missing(&local.oplog_vv(), &remote.oplog_vv()),
        Missing {
            download: false,
            upload: false
        }
    );
}
