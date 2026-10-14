// SPDX-License-Identifier: MPL-2.0

//! The unshipped standalone host: the same router the gateway embeds, on a
//! loopback listener, for integration suites and hand diagnostics. It has no
//! launcher target, no discovery, no credential — a shipping client reaches
//! the events worker through the API gateway.

use koru_events_server::{EventsHandle, WorkerConfig};

fn main() {
    if let Err(error) = run() {
        eprintln!("koru-events-server: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // A diagnostic host that prints nothing is not one. Everything this crate
    // says -- the events debug lines, the telemetry drain's delivery lines --
    // is a `tracing` event, and without a subscriber they all went nowhere.
    let _logging = koru_log::init_stdout("info");
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => {
                dir = Some(std::path::PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--root needs a path".to_owned())?,
                ));
            }
            "--version" => {
                println!("koru-events-server {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("koru-events-server [--root <dir>] [--version]");
                return Ok(());
            }
            unknown => return Err(format!("unknown argument {unknown}")),
        }
    }
    let dir = dir
        .or_else(|| koru_channel::store_root().map(|root| koru_channel::events_root_beside(&root)))
        .ok_or_else(|| "could not determine the events directory".to_owned())?;
    let handle = EventsHandle::open(WorkerConfig::new(dir)).map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        println!(
            "koru-events-server listening on http://127.0.0.1:{}",
            address.port()
        );
        axum::serve(listener, handle.router())
            .await
            .map_err(|error| error.to_string())
    })
}
