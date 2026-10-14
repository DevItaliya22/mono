// SPDX-License-Identifier: MPL-2.0

//! Optional standalone Koru AI integration host.

fn main() {
    let requested_version = std::env::args().skip(1).any(|arg| arg == "--version");
    if requested_version {
        println!("koru-ai-server {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    eprintln!(
        "koru-ai-server: no public standalone transport is enabled; embed AiWorker through the Koru API gateway"
    );
    std::process::exit(2);
}
