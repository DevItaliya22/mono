// SPDX-License-Identifier: MPL-2.0

//! Who is holding an endpoint — the one question a failed bind cannot answer.
//!
//! A Unix socket outlives its process's *installation*: an older Koru whose
//! bundle was replaced on disk keeps serving the channel endpoint from
//! nowhere, the new build's bind fails as "taken", and the person sees only
//! "nothing is listening" from the other side. This module names the squatter
//! so the failure message can say *quit that process* instead of asking for an
//! hour of diagnosis. It shells out to `lsof` — a diagnostic on a path that is
//! already fatal, never on a hot path — and answers `None` on any platform or
//! parse it does not understand: naming nobody is better than naming wrongly.

/// The process holding the Unix socket at `path`, as `pid N (command)`.
///
/// Unix only, best effort: `None` on Windows (named pipes vanish with their
/// process, so a taken name always has a live, findable owner in Task
/// Manager), when `lsof` is unavailable, or when nothing holds the path.
pub fn holder_of(path: &str) -> Option<String> {
    if cfg!(windows) || path.is_empty() {
        return None;
    }
    let output = std::process::Command::new("lsof")
        .args(["-U", "-Fpcn"])
        .output()
        .ok()?;
    parse_lsof(&String::from_utf8_lossy(&output.stdout), path)
}

/// The parse half, split out so a test needs no live socket. `lsof -Fpcn`
/// emits one field per line: `p<pid>` then `c<command>` once per process,
/// then `n<name>` per descriptor.
fn parse_lsof(output: &str, path: &str) -> Option<String> {
    let mut pid = "";
    let mut command = "";
    for line in output.lines() {
        match line.split_at_checked(1) {
            Some(("p", rest)) => pid = rest,
            Some(("c", rest)) => command = rest,
            Some(("n", rest)) if rest == path && !pid.is_empty() => {
                return Some(if command.is_empty() {
                    format!("pid {pid}")
                } else {
                    format!("pid {pid} ({command})")
                });
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_holder_is_read_out_of_lsof_field_output() {
        let output = "p123\ncSomething Else\nn/tmp/other.sock\np456\nckoru-store\nn/tmp/koru-501/koru-store/nightly.sock\n";
        assert_eq!(
            parse_lsof(output, "/tmp/koru-501/koru-store/nightly.sock"),
            Some("pid 456 (koru-store)".into())
        );
        assert_eq!(parse_lsof(output, "/tmp/nobody.sock"), None);
        assert_eq!(parse_lsof("", "/tmp/koru.sock"), None);
    }
}
