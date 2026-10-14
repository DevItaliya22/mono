// SPDX-License-Identifier: MPL-2.0

//! **Where the source of a package is (MPL-2.0 §3.2).**
//!
//! Distributing Koru in executable form obliges us to tell recipients how to
//! obtain its Source Code Form. Each package therefore names the exact commit
//! it was built from, as a link into the public repository: in the packaged
//! `NOTICE` (what a person reads) and in the channel manifest's `source`
//! field (what a tool reads).
//!
//! **The commit is the build's own git state** — `git rev-parse HEAD` of the
//! tree being packaged, the same value `VERSION` records — and there is
//! deliberately no environment override: a variable could make a package name
//! a commit it was not built from, and the package's whole claim is that it
//! was. An official release refuses a tree whose commit it cannot name; a
//! local bundle built from a tarball says it does not know and points at the
//! repository instead.

/// The public repository every Koru package is built from.
pub const REPOSITORY: &str = "https://github.com/koruworks/mono";

/// Official artifacts must identify the committed source that was built.
/// Called before a build and again before its archive can be signed.
pub fn check_tree(status: &str, head: &str, expected: Option<&str>) -> Result<(), String> {
    if !status.trim().is_empty() {
        return Err("official publication requires a clean Git tree; commit or remove pending changes (including generated release notes), then retry; use dist-local for an uncommitted build".into());
    }
    if expected.is_some_and(|commit| commit != head) {
        return Err(
            "Git HEAD changed during publication; retry from the commit to be released".into(),
        );
    }
    Ok(())
}

/// The source link for `commit`, or `None` when `commit` is not a full git
/// object id (40 hex digits for SHA-1, 64 for SHA-256). An abbreviated id is
/// refused on purpose: it can become ambiguous as the repository grows, and a
/// license notice must keep naming the same tree forever.
pub fn url(commit: &str) -> Option<String> {
    let full = matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    full.then(|| format!("{REPOSITORY}/tree/{commit}"))
}

/// The `NOTICE` a package carries: the repository's own notice, then where
/// this build's source code is.
pub fn notice(repository_notice: &str, commit: &str) -> String {
    let mut text = repository_notice.trim_end().to_string();
    text.push_str("\n\nSource code\n\n");
    match url(commit) {
        Some(url) => text.push_str(&format!(
            "This build of Koru was made from the source code at\n\n    {url}\n\n\
             which is the Source Code Form the Mozilla Public License 2.0\n\
             (section 3.2) refers to, including the third-party code and the\n\
             patches in third-party/.\n"
        )),
        None => text.push_str(&format!(
            "This build does not record the commit it was made from. Koru's\n\
             source code is at\n\n    {REPOSITORY}\n"
        )),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn official_source_refuses_uncommitted_changes_and_a_changed_head() {
        assert!(check_tree("", COMMIT, None).is_ok());
        assert!(check_tree("", COMMIT, Some(COMMIT)).is_ok());
        for status in [" M src/main.rs", "M  Cargo.lock", "?? new.rs", " M docs"] {
            assert!(check_tree(status, COMMIT, Some(COMMIT)).is_err());
        }
        assert!(check_tree("", COMMIT, Some(&"a".repeat(40))).is_err());
    }

    #[test]
    fn a_full_commit_becomes_a_tree_link() {
        assert_eq!(
            url(COMMIT).as_deref(),
            Some("https://github.com/koruworks/mono/tree/0123456789abcdef0123456789abcdef01234567")
        );
        let sha256 = "a".repeat(64);
        assert_eq!(
            url(&sha256),
            Some(format!("https://github.com/koruworks/mono/tree/{sha256}"))
        );
    }

    #[test]
    fn anything_but_a_full_lowercase_object_id_is_refused() {
        for commit in [
            "",
            "0123456",
            "nogit",
            &COMMIT.to_uppercase(),
            &format!("{COMMIT}\n"),
            &format!("{}g", &COMMIT[..39]),
            &"a".repeat(41),
        ] {
            assert_eq!(url(commit), None, "{commit:?}");
        }
    }

    #[test]
    fn the_packaged_notice_keeps_the_repository_notice_and_names_the_commit() {
        let notice = notice("Koru\nCopyright (c) Koru Works, Inc.\n\n", COMMIT);
        assert!(notice.starts_with("Koru\nCopyright (c) Koru Works, Inc.\n\nSource code\n"));
        assert!(notice.contains(&format!("{REPOSITORY}/tree/{COMMIT}")));
        assert!(notice.contains("Mozilla Public License 2.0"));
        assert!(notice.ends_with('\n'));
    }

    #[test]
    fn a_build_without_git_says_so_and_points_at_the_repository() {
        let notice = notice("Koru\n", "");
        assert!(notice.contains("does not record the commit"));
        assert!(notice.contains(REPOSITORY));
        assert!(!notice.contains("/tree/"));
    }
}
