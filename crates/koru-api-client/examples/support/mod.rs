// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use koru_api_client::sessions::{
    AttachSessionRequest, BuiltInProfileKey, CreateSessionRequest, GridSize, IdempotencyKey,
    SessionId, SessionProfileKey, SessionsError, TerminateRequest, ViewportGeneration,
};

pub type ExampleResult<T> = Result<T, Box<dyn std::error::Error>>;

pub struct Args {
    pub root: PathBuf,
    pub image: Option<PathBuf>,
}

pub fn args(needs_image: bool) -> ExampleResult<Args> {
    parse_args(std::env::args_os().skip(1), needs_image)
}

fn parse_args(
    mut values: impl Iterator<Item = std::ffi::OsString>,
    needs_image: bool,
) -> ExampleResult<Args> {
    let mut root = None;
    let mut image = None;
    while let Some(value) = values.next() {
        if value == "--root" {
            root = Some(PathBuf::from(values.next().ok_or("--root needs a path")?));
        } else if value == "--image" {
            image = Some(PathBuf::from(values.next().ok_or("--image needs a path")?));
        } else if value == "--help" || value == "-h" {
            return Err(usage(needs_image).into());
        } else {
            return Err(format!(
                "unknown argument {}; {}",
                value.to_string_lossy(),
                usage(needs_image)
            )
            .into());
        }
    }
    let root = root.ok_or_else(|| usage(needs_image))?;
    if needs_image && image.is_none() {
        return Err(usage(needs_image).into());
    }
    Ok(Args { root, image })
}

fn usage(needs_image: bool) -> String {
    if needs_image {
        "usage: cargo run -p koru-api-client --example session_image_path -- --root <path> --image <path>".into()
    } else {
        "usage: cargo run -p koru-api-client --example session_raw_shell -- --root <path>".into()
    }
}

pub fn connect(root: &std::path::Path) -> ExampleResult<koru_api_client::Client> {
    Ok(koru_api_client::Client::open_at_with_scopes(
        koru_channel::segment(),
        root,
        koru_session_api::ALL_SCOPES,
    )?)
}

pub fn create_and_attach(
    client: &koru_api_client::Client,
) -> ExampleResult<(SessionId, koru_api_client::sessions::ControllerLeaseToken)> {
    let working_directory = std::env::current_dir()?.canonicalize()?;
    let profile = if cfg!(windows) { "powershell" } else { "bash" };
    let session = client.sessions().create(&CreateSessionRequest {
        idempotency_key: IdempotencyKey::mint(),
        profile: SessionProfileKey::BuiltIn(profile.parse::<BuiltInProfileKey>()?),
        parent_session_id: None,
        mode: koru_api_client::sessions::SessionMode::RawPty,
        working_directory: working_directory.to_string_lossy().into_owned(),
        size: GridSize::new(80, 24).map_err(protocol_error)?,
    })?;
    match client.sessions().attach(
        session.id,
        &AttachSessionRequest {
            viewport_generation: ViewportGeneration::new(1).map_err(protocol_error)?,
            columns: 80,
            overscan_rows: 2,
        },
    ) {
        Ok(lease) => Ok((session.id, lease.token)),
        Err(error) => {
            // A create succeeded, so make one best-effort lease acquisition to
            // preserve the example's "terminate what it creates" contract.
            cleanup_without_lease(client, session.id);
            Err(error.into())
        }
    }
}

pub fn terminate(
    client: &koru_api_client::Client,
    session_id: SessionId,
    lease: koru_api_client::sessions::ControllerLeaseToken,
) -> Result<(), SessionsError> {
    client.sessions().terminate(
        session_id,
        &TerminateRequest {
            idempotency_key: IdempotencyKey::mint(),
            controller_lease: lease,
        },
    )?;
    Ok(())
}

fn cleanup_without_lease(client: &koru_api_client::Client, session_id: SessionId) {
    let Ok(lease) = client.sessions().attach(
        session_id,
        &AttachSessionRequest {
            viewport_generation: ViewportGeneration::new(1).expect("one is positive"),
            columns: 80,
            overscan_rows: 0,
        },
    ) else {
        return;
    };
    let _ = terminate(client, session_id, lease.token);
}

pub fn finish<T>(
    operation: ExampleResult<T>,
    cleanup: Result<(), SessionsError>,
) -> ExampleResult<T> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(format!("session cleanup failed: {error}").into()),
    }
}

fn protocol_error(code: koru_api_client::sessions::ErrorCode) -> Box<dyn std::error::Error> {
    std::io::Error::other(code.to_string()).into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn root_is_required_and_image_is_explicit() {
        assert!(super::parse_args(std::iter::empty(), false).is_err());
        let values = ["--root", "r", "--image", "i"].map(Into::into).into_iter();
        let parsed = super::parse_args(values, true).unwrap();
        assert_eq!(parsed.root, std::path::Path::new("r"));
        assert_eq!(parsed.image.as_deref(), Some(std::path::Path::new("i")));
    }
}
