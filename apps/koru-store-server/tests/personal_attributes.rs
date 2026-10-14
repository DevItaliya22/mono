// SPDX-License-Identifier: MPL-2.0

//! **Personal attributes, through a real store and back.**
//!
//! A1b's gate: a `koru.plan` date set by one profile survives the store
//! process being killed and restarted, reads back **byte-exactly** as the
//! calendar date it was given, is invisible to every other profile (foundation
//! invariant 7), and clears to an *absent* key rather than a stored null.
//!
//! Driven through `koru-store-client` — the same client a third party holds —
//! because there is no privileged path in, including for our own tests.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_store_client::{Client, PersonalAttributeValue};
use koru_store_core::fixtures::SAMPLES;

/// A real store process on a root of its own.
struct Store {
    child: Option<Child>,
    root: PathBuf,
}

impl Store {
    fn start(root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_koru-store-server"));
        command
            .arg("serve")
            // A suite starting daemons on throwaway roots must never raise a
            // modal dialog: nothing dismisses it, so the process would never
            // exit and would go on holding its databases and this binary.
            .env("KORU_STORE_NO_DIALOG", "1")
            .arg("--console")
            .arg("--root")
            .arg(root);
        common::configure(&mut command, root);
        let child = command.spawn().expect("spawn koru-store-server");

        let deadline = Instant::now() + Duration::from_secs(30);
        while common::discovery(root).map(|d| Client::new(d).status().is_ok()) != Some(true) {
            assert!(
                Instant::now() < deadline,
                "the store never became reachable"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            child: Some(child),
            root: root.to_path_buf(),
        }
    }

    fn client(&self) -> Client {
        Client::new(common::discovery(&self.root).expect("discover"))
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Start a new one on the same root — a different process, a different
    /// port, and the same databases.
    fn restart(&mut self) {
        self.kill();
        *self = Self::start(&self.root.clone());
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.kill();
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("koru-personal-attrs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

/// **The whole plan in one pass**: set, restart, read back byte-exactly,
/// prove isolation between profiles, then clear to absence.
///
/// The write is an acknowledged HTTP response, and an acknowledgement means
/// the bytes are on the disk — which is exactly what killing the process
/// before reading is there to prove.
#[test]
fn a_personal_plan_survives_a_restart_and_belongs_to_one_profile() {
    let root = root_for("roundtrip");
    let mut store = Store::start(&root);
    let client = store.client();

    let alice = client
        .create_profile("Alice", None, None, None)
        .expect("profile a");
    let bob = client
        .create_profile("Bob", None, None, None)
        .expect("profile b");
    let workspace = client
        .create_workspace(alice.id, "Acme")
        .expect("workspace");
    let document = client
        .create_document(workspace.id, "Roadmap", "page")
        .expect("document");

    // A block reference: document and node carried together, because a node
    // id alone means nothing outside its document.
    let entity = format!(
        "block:{}:{}:{}",
        workspace.id.to_storage(),
        document.to_storage(),
        uuid::Uuid::new_v4()
    );

    let written = client
        .set_personal_attribute(
            alice.id,
            &entity,
            "koru.plan",
            Some(PersonalAttributeValue::Date {
                date: "2026-08-30".into(),
            }),
        )
        .expect("set koru.plan");
    assert_eq!(
        written.attributes.get("koru.plan"),
        Some(&PersonalAttributeValue::Date {
            date: "2026-08-30".into()
        }),
        "the write answers what is now true"
    );

    // A non-ASCII text value beside it, from the eight-language fixture — a
    // personal note is the person's own text and must survive untouched.
    let sample = SAMPLES
        .iter()
        .find(|sample| sample.tag == "ja")
        .expect("the Japanese fixture");
    client
        .set_personal_attribute(
            alice.id,
            &entity,
            "user.note",
            Some(PersonalAttributeValue::Text {
                text: sample.title.to_owned(),
            }),
        )
        .expect("set user.note");

    // ---- kill the process and start another on the same databases
    store.restart();
    let client = store.client();

    let read = client
        .personal_attributes(alice.id, &entity)
        .expect("read after restart");
    // Byte-exact: the calendar date comes back as the string it was given,
    // never rewritten through an instant (invariant 8).
    assert_eq!(
        read.attributes.get("koru.plan"),
        Some(&PersonalAttributeValue::Date {
            date: "2026-08-30".into()
        }),
        "the date did not survive the restart: {:?}",
        read.attributes
    );
    assert_eq!(
        read.attributes.get("user.note"),
        Some(&PersonalAttributeValue::Text {
            text: sample.title.to_owned()
        }),
        "the {} text did not survive the restart",
        sample.tag
    );

    // ---- invariant 7: personal state never leaks between profiles
    let bobs = client
        .personal_attributes(bob.id, &entity)
        .expect("read as bob");
    assert!(
        bobs.attributes.is_empty(),
        "another profile sees Alice's personal state: {:?}",
        bobs.attributes
    );

    // ---- clearing removes the key: absent, not null
    let cleared = client
        .set_personal_attribute(alice.id, &entity, "koru.plan", None)
        .expect("clear koru.plan");
    assert!(
        !cleared.attributes.contains_key("koru.plan"),
        "a cleared attribute must be absent: {:?}",
        cleared.attributes
    );
    assert!(
        cleared.attributes.contains_key("user.note"),
        "clearing one key must not touch its neighbours"
    );

    // And the clear is durable too.
    store.restart();
    let read = store
        .client()
        .personal_attributes(alice.id, &entity)
        .expect("read after the second restart");
    assert!(!read.attributes.contains_key("koru.plan"));
    assert!(read.attributes.contains_key("user.note"));
}

/// A reference the store does not understand is refused, not filed under —
/// an address that cannot be read back must never be written to.
#[test]
fn a_malformed_entity_reference_is_refused() {
    let root = root_for("refused");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");

    for bad in ["", "block:not-a-uuid", "just-a-node-id"] {
        assert!(
            client
                .set_personal_attribute(
                    profile.id,
                    bad,
                    "koru.plan",
                    Some(PersonalAttributeValue::Bool { value: true }),
                )
                .is_err(),
            "{bad:?} was accepted"
        );
        assert!(
            client.personal_attributes(profile.id, bad).is_err(),
            "{bad:?} was readable"
        );
    }

    // And a value that blurs a calendar date into an instant is refused too.
    let entity = format!(
        "block:{}:{}:{}",
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4()
    );
    assert!(
        client
            .set_personal_attribute(
                profile.id,
                &entity,
                "koru.plan",
                Some(PersonalAttributeValue::Date {
                    date: "2026-08-30T00:00:00Z".into()
                }),
            )
            .is_err(),
        "an instant is not a calendar date"
    );
}
