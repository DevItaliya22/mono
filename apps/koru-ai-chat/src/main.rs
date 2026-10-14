#![cfg_attr(not(test), windows_subsystem = "windows")]
// SPDX-License-Identifier: MPL-2.0

mod activation;
mod cli;
mod geometry;
mod window;

use std::time::Duration;

use cli::{Action, Invocation};

fn main() {
    koru_core::timing::start();
    let invocation = match cli::parse(std::env::args()) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("error: {}\n\n{}", error.0, cli::help(&program_name()));
            std::process::exit(2);
        }
    };
    match invocation.action {
        Action::Help => println!("{}", cli::help(&program_name())),
        Action::Version => println!("Koru AI {}", cli::version()),
        Action::Stop => stop(invocation),
        Action::Run => run(invocation),
    }
}

fn stop(invocation: Invocation) {
    let _ = activation::request(
        invocation.root.as_deref(),
        &koru_agent_wire::AiChatRequest::Exit,
        Duration::from_secs(1),
    );
}

fn run(invocation: Invocation) {
    let channel_root = invocation.root;
    // Every installed process that can be replaced owns the shared side of
    // the installation gate for its whole lifetime. Agent's token-bearing
    // restart is the sole exception; Chat must fail before binding or
    // activating while updater still owns the post-swap health window.
    let _update_lease = match acquire_update_lease(channel_root.as_deref()) {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!(
                "Koru AI Chat cannot start while this installation is being updated: {error}"
            );
            return;
        }
    };
    let store_root = channel_root
        .as_ref()
        .map(|root| koru_channel::store_root_under(root));
    let _logging = store_root
        .clone()
        .or_else(koru_channel::store_root)
        .map(|root| {
            let logging = koru_log::init_file_and_stdout(
                &root,
                "koru-ai-chat.log",
                "warn,koru_ai=info,vello=off",
            );
            tracing::info!(
                target: "koru_ai::startup",
                version = cli::version(),
                pid = std::process::id(),
                "Koru AI chat started"
            );
            logging
        });

    let activation = activation::bind(channel_root.as_deref());
    if activation.is_none() {
        let request = invocation
            .link
            .as_ref()
            .map(|(chat, block)| koru_agent_wire::AiChatRequest::OpenChat {
                chat: chat.as_uuid().to_string(),
                block: block.map(|block| block.as_uuid().to_string()),
            })
            .unwrap_or(koru_agent_wire::AiChatRequest::Activate);
        let _ = activation::request(channel_root.as_deref(), &request, Duration::from_secs(1));
        return;
    }

    // **Defaults now, correct later.** This window used to parse
    // `preferences.conf` here, on the main thread, before any window existed.
    // It now opens on the registry's defaults — which is exactly what that
    // parser produced when the file could not be read — and the link corrects
    // it when the authority answers. See ADR-266.
    let appearance = koru_window::Appearance::default();
    appearance.apply();
    let settings = koru_api_client::GlobalLink::global(
        koru_channel::segment().to_owned(),
        channel_root.clone(),
    );
    let geometry_path = window::geometry_path(channel_root.as_deref());
    if let Err(error) = koru_window::run(move |_engine| {
        window::ChatApp::new(
            invocation.link,
            activation,
            store_root,
            channel_root,
            geometry_path,
            appearance,
            settings,
        )
    }) {
        tracing::error!(target: "koru_ai::startup", %error, "AI Chat window loop failed");
    }
}

fn acquire_update_lease(
    channel_root: Option<&std::path::Path>,
) -> Result<Option<koru_update::InstanceLease>, String> {
    if !koru_channel::build_info().updates_enabled() {
        return Ok(None);
    }
    let paths = match channel_root {
        Some(root) => koru_update::UpdatePaths::under(root),
        None => koru_update::UpdatePaths::discover()
            .ok_or("Koru AI Chat could not determine where its update state lives")?,
    };
    let install = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .ok_or("Koru AI Chat could not determine its installation directory")?;
    koru_update::InstanceLease::acquire(&paths, &install).map(Some)
}

fn program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "koru-ai-chat".into())
}
