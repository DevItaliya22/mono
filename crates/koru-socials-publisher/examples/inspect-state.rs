// SPDX-License-Identifier: MPL-2.0

//! Read-only local diagnostics. Never constructs an X provider or reads tokens.
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("explicit root required"));
    let client = koru_api_client::Client::connect_at(koru_channel::segment(), &root)
        .expect("existing rooted Agent");
    for ws in client.store().workspaces().expect("workspace catalog") {
        for entry in client.store().documents(ws.id).expect("manifest") {
            if entry.trashed_at.is_some() || entry.kind != koru_socials::POST_KIND {
                continue;
            }
            match koru_socials_store::load_post(&client, ws.id, entry.id.as_uuid()) {
                Ok(p) => println!(
                    "post={} scheduled={} owner_assigned={} due_ms={:?} updated_ms={} cancelled={} deleted={} deliveries={:?}",
                    p.id,
                    p.scheduled,
                    p.execution_device.is_some(),
                    p.scheduled_at_ms,
                    p.updated_at_ms,
                    p.cancel_requested,
                    p.deleted,
                    p.destinations
                        .iter()
                        .map(|d| (
                            d.delivery.state,
                            d.delivery.error_code.as_deref(),
                            d.delivery.published.len()
                        ))
                        .collect::<Vec<_>>()
                ),
                Err(e) => println!("post={} unreadable={e:?}", entry.id),
            }
        }
    }
}
