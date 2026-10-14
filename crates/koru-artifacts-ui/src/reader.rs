// SPDX-License-Identifier: MPL-2.0

//! Everything this surface does that is not instant, off the window
//! thread.
//!
//! **Two threads, for the reason `koru-search-ui` has two.** The read
//! thread coalesces to the newest request, which is right for a refresh and
//! wrong for anything that only needs doing once: a Register queued behind
//! two refreshes would simply be dropped. So Register has its own channel
//! and its own thread, and a slow catalog can never sit in front of a
//! click.
//!
//! **Dead under test, and that is the point.** [`Reader::start`] is never
//! called under `cfg(test)` or with the `no-reader` feature, so no unit
//! test dials the developer's own gateway — see the testing section of
//! `AGENTS.md`. The consequence is that the worker and the messages it
//! sends have no caller in a test build; allowed module-wide rather than
//! item by item, because warnings that are always there are warnings
//! nobody reads.
#![cfg_attr(test, allow(dead_code))]

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};

use koru_artifact_wire::run_folder::{RegisterFileRequest, RegisteredFile, RelativePath};
use koru_artifact_wire::{
    ArtifactFamilyId, ArtifactVersionId, Failure, Provenance, RegisterVersionRequest,
    RegistrationId, RunId, WorkspaceId,
};
use koru_markdown_view::{Content, MarkdownDocument};

use crate::gateway::Gateway;
use crate::model::Catalog;
use crate::result::{MARKDOWN, ResultBody, body_of};
use crate::tab::VersionPlace;

/// What the read thread is asked for. One variant, because there is one
/// answer a column needs and asking for its halves separately would put two
/// handshakes behind one refresh.
pub(crate) struct CatalogRequest {
    pub(crate) seq: u64,
    pub(crate) workspace: WorkspaceId,
}

/// What the act thread is asked for. **Never coalesced**: a person pressed
/// each of these.
pub(crate) enum Act {
    Provenance {
        version: ArtifactVersionId,
    },
    Register {
        run: RunId,
        path: RelativePath,
    },
    /// One version's bytes, and the document they are.
    Content {
        version: ArtifactVersionId,
    },
    /// **The next version of a family.** The bytes go into the workspace's
    /// blob store first and the registration names the blob — P1.11's own
    /// path, not a second one — and `derived_from` is the version that was
    /// open, so the provenance sentence says what it was built on without
    /// anybody writing that down.
    SaveVersion {
        workspace: WorkspaceId,
        family: ArtifactFamilyId,
        derived_from: ArtifactVersionId,
        text: String,
    },
}

pub(crate) enum Answer {
    Catalog {
        seq: u64,
        result: Result<Catalog, Failure>,
    },
    Provenance {
        version: ArtifactVersionId,
        result: Result<Provenance, Failure>,
    },
    Registered {
        run: RunId,
        path: RelativePath,
        result: Result<RegisteredFile, Failure>,
    },
    Content {
        version: ArtifactVersionId,
        /// Where it sits, when the store could say. `None` is a version the
        /// store would not describe -- the tab then reads it and offers no
        /// edit, because it does not know what saving would make.
        place: Option<VersionPlace>,
        content: Box<Content>,
    },
    Saved {
        result: Result<(ArtifactVersionId, u32, Box<Content>), Failure>,
    },
}

pub(crate) struct Reader {
    /// **Absent for a surface that never lists.** The tab holds one version;
    /// a catalog thread behind it would be a thread nothing ever speaks to.
    catalog: Option<Sender<CatalogRequest>>,
    acts: Sender<Act>,
    answers: Receiver<Answer>,
}

impl Reader {
    /// **The acting half alone** -- what a tab that reads and saves one
    /// version needs. One thread, because everything a tab asks for is
    /// something a person pressed, and none of it may be coalesced away.
    pub(crate) fn start_acting(root: Option<std::path::PathBuf>) -> std::io::Result<Self> {
        let (acts, acts_rx) = mpsc::channel::<Act>();
        let (act_tx, answers) = mpsc::channel::<Answer>();
        let acting = Gateway::new(root);
        std::thread::Builder::new()
            .name("koru-artifact-tab".into())
            .spawn(move || act_loop(&acting, &acts_rx, &act_tx))?;
        Ok(Self {
            catalog: None,
            acts,
            answers,
        })
    }

    pub(crate) fn start(root: Option<std::path::PathBuf>) -> std::io::Result<Self> {
        let (catalog, catalog_rx) = mpsc::channel::<CatalogRequest>();
        let (acts, acts_rx) = mpsc::channel::<Act>();
        let (answer_tx, answers) = mpsc::channel::<Answer>();
        let act_tx = answer_tx.clone();

        // **One handle, two threads.** Two clients would mint two gateway
        // sessions and put two "session started" lines in the log per
        // window.
        let gateway = Gateway::new(root);
        let acting = gateway.clone();

        std::thread::Builder::new()
            .name("koru-artifacts-read".into())
            .spawn(move || {
                while let Ok(request) = catalog_rx.recv() {
                    let request = newest(request, &catalog_rx);
                    let result = load(&gateway, request.workspace);
                    if answer_tx
                        .send(Answer::Catalog {
                            seq: request.seq,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;

        std::thread::Builder::new()
            .name("koru-artifacts-act".into())
            .spawn(move || act_loop(&acting, &acts_rx, &act_tx))?;

        Ok(Self {
            catalog: Some(catalog),
            acts,
            answers,
        })
    }

    pub(crate) fn refresh(&self, seq: u64, workspace: WorkspaceId) -> bool {
        self.catalog
            .as_ref()
            .is_some_and(|catalog| catalog.send(CatalogRequest { seq, workspace }).is_ok())
    }

    pub(crate) fn act(&self, act: Act) -> bool {
        self.acts.send(act).is_ok()
    }

    pub(crate) fn drain(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }
}

/// **The one act loop**, whichever surface started it. The column and the
/// tab ask the same questions of the same gateway; two copies of this would
/// be two answers to drift.
fn act_loop(gateway: &Gateway, acts: &Receiver<Act>, out: &Sender<Answer>) {
    while let Ok(act) = acts.recv() {
        let answer = match act {
            Act::Provenance { version } => Answer::Provenance {
                version,
                result: gateway.call(|client| client.artifacts().provenance(version)),
            },
            Act::Content { version } => {
                let (place, content) = read_content(gateway, version);
                Answer::Content {
                    version,
                    place,
                    content: Box::new(content),
                }
            }
            Act::SaveVersion {
                workspace,
                family,
                derived_from,
                text,
            } => Answer::Saved {
                result: save_version(gateway, workspace, family, derived_from, &text),
            },
            Act::Register { run, path } => {
                let result = gateway.call(|client| {
                    client.artifacts().register_file(
                        run,
                        &RegisterFileRequest {
                            path: path.clone(),
                            title: None,
                            mime: None,
                        },
                    )
                });
                Answer::Registered { run, path, result }
            }
        };
        if out.send(answer).is_err() {
            break;
        }
    }
}

/// One version, as something to draw. The metadata comes first: what it is
/// decides whether the bytes are worth asking for at all — and that decision
/// is [`body_of`]'s, the same one [`crate::read_result`] makes for a host.
fn read_content(gateway: &Gateway, version: ArtifactVersionId) -> (Option<VersionPlace>, Content) {
    let described = match gateway.call(|client| client.artifacts().version(version)) {
        Ok(described) => described,
        Err(_) => return (None, Content::Unreadable),
    };
    // **Where it sits, asked once and here.** The family says which
    // workspace's blob store the next version's bytes belong in and the
    // ordinal says what this one is called; a surface that had to ask either
    // from a frame would be reading a store on the window thread.
    let place = gateway
        .call(|client| client.artifacts().family(described.family))
        .ok()
        .map(|found| VersionPlace {
            family: described.family,
            workspace: found.family.workspace,
            ordinal: described.ordinal.get(),
        });
    let content = match body_of(&described, || {
        gateway.call(|client| client.artifacts().content(version))
    }) {
        Ok(ResultBody::Markdown(text)) => match MarkdownDocument::of(&text) {
            Ok(document) => Content::Document(Box::new(document)),
            Err(_) => Content::Unreadable,
        },
        // A picture and anything else are named here, not drawn: the tab
        // is a Markdown surface.
        Ok(ResultBody::Image { .. } | ResultBody::Other { .. }) => Content::NotMarkdown {
            mime: described.mime.as_str().to_owned(),
            size_bytes: described.size_bytes,
            filename: described
                .filename
                .as_ref()
                .map(|name| name.as_str().to_owned()),
        },
        Err(_) => Content::Unreadable,
    };
    (place, content)
}

/// **The next version, by the register path P1.11 already built.**
///
/// The bytes are the *document's* Markdown rather than the composer's text,
/// so what is stored is what the column was drawing. They go into the
/// workspace's blob store and the registration names the blob — there is
/// one rule about how content reaches a version, and this is not a second
/// one.
fn save_version(
    gateway: &Gateway,
    workspace: WorkspaceId,
    family: ArtifactFamilyId,
    derived_from: ArtifactVersionId,
    text: &str,
) -> Result<(ArtifactVersionId, u32, Box<Content>), Failure> {
    let document = MarkdownDocument::of(text)
        .map_err(|_| Failure::new("invalid_request", "The text is not a document.", false))?;
    let bytes = document.markdown().as_bytes().to_vec();
    let blob = gateway.call(|client| {
        client
            .store()
            .put_blob(
                koru_store_core::ids::WorkspaceId::from_uuid(workspace.as_uuid()),
                bytes.clone(),
                MARKDOWN,
                None,
            )
            .map_err(|error| Failure::new("store_unavailable", error.to_string(), true))
    })?;
    let request = RegisterVersionRequest {
        registration_id: RegistrationId::mint(),
        blob: blob.hash,
        mime: koru_artifact_wire::MediaType::new(MARKDOWN)
            .map_err(|_| Failure::new("invalid_request", "media type", false))?,
        filename: None,
        produced_by: None,
        derived_from: Some(derived_from),
    };
    let registered =
        gateway.call(|client| client.artifacts().register_version(family, &request))?;
    Ok((
        registered.version.id,
        registered.version.ordinal.get(),
        Box::new(Content::Document(Box::new(document))),
    ))
}

fn newest(mut request: CatalogRequest, pending: &Receiver<CatalogRequest>) -> CatalogRequest {
    for newer in pending.try_iter() {
        request = newer;
    }
    request
}

/// One refresh: the workspace's families with their versions, what its runs
/// left unregistered, and which blobs this device holds.
///
/// The blob listing is allowed to fail on its own: without it the column
/// simply does not claim anything is elsewhere (D27's `locality_known`),
/// which is a better answer than turning every result into a *not here*
/// because one request did not land.
fn load(gateway: &Gateway, workspace: WorkspaceId) -> Result<Catalog, Failure> {
    let families = gateway.call(|client| {
        let listed = client.artifacts().list_families(workspace, true)?;
        let mut out = Vec::with_capacity(listed.families.len());
        for family in listed.families {
            let full = client.artifacts().family(family.id)?;
            out.push((full.family, full.versions));
        }
        Ok(out)
    })?;
    let runs = gateway
        .call(|client| client.artifacts().unregistered(workspace))?
        .runs;
    let present = gateway
        .call(|client| {
            client
                .store()
                .blobs(workspace)
                .map_err(|error| Failure::new("store_unavailable", error.to_string(), true))
        })
        .ok()
        .map(|blobs| {
            blobs
                .into_iter()
                .map(|blob| blob.hash.to_string())
                .collect::<HashSet<String>>()
        });
    Ok(Catalog::build(families, runs, present))
}
