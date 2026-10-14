// SPDX-License-Identifier: MPL-2.0

//! Explicit disk-only migration check under the worktree's ignored target tree.
//! No Agent, Store, credential or provider is constructed.
use koru_socials::PublicationTarget;
use koru_socials_publisher::{Journal, Key, Phase, SqliteLedger};
use rusqlite::{Connection, params};
use uuid::Uuid;

fn main() {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/verification/socials");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("migration-{}.db", Uuid::new_v4()));
    let key = Key {
        workspace: Uuid::new_v4(),
        post: Uuid::new_v4(),
        channel: Uuid::new_v4(),
    };
    let attempt_id = Uuid::new_v4();
    let device = Uuid::new_v4();
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(include_str!("../migrations/socials_delivery_v1.sql"))
            .unwrap();
        db.execute("INSERT INTO social_attempts(workspace,post,channel,attempt,device,actor,phase,failures) VALUES(?1,?2,?3,?4,?5,'synthetic','published',0)",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice(),attempt_id.as_bytes().as_slice(),device.as_bytes().as_slice()]).unwrap();
        db.execute("INSERT INTO social_parts(workspace,post,channel,part,text,remote_id,published_at_ms) VALUES(?1,?2,?3,0,'Synthetic migration fixture','123',42)",params![key.workspace.as_bytes().as_slice(),key.post.as_bytes().as_slice(),key.channel.as_bytes().as_slice()]).unwrap();
    }
    {
        let mut ledger = SqliteLedger::open(&path).unwrap();
        let saved = ledger.load(key).unwrap().unwrap();
        assert_eq!(saved.target, PublicationTarget::X);
        assert_eq!(saved.id, attempt_id);
        assert_eq!(saved.phase, Phase::Published);
        assert_eq!(saved.published[0].id, "123");
        ledger.store(key, &saved).unwrap();
    }
    let mut reopened = SqliteLedger::open(&path).unwrap();
    assert_eq!(
        reopened.load(key).unwrap().unwrap().published[0].published_at_ms,
        42
    );
    println!(
        "PASS disk migration 1 -> 2 and reopen: X identity, frozen body and confirmed publication retained"
    );
    println!("Synthetic evidence: {}", path.display());
}
