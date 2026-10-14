// SPDX-License-Identifier: MPL-2.0

//! Test-only: what a live agent asked the desktop bridge for, read from the
//! bridge's own log.
//!
//! Every agent logs differently, and some not at all; the bridge logs each
//! `tools/list` it answers with the cursor it was given and the one it handed
//! back (ADR-433-compacttools). Started with `--root <folder>`, a bridge
//! writes that log under `<folder>/store/logs/`, so one folder per start is
//! one agent's evidence and nobody else's.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One `tools/list` the bridge answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListRequest {
    pub cursor: String,
    pub tools: usize,
    pub next_cursor: String,
}

/// Everything one bridge answered, in order.
#[derive(Debug, Default)]
pub(crate) struct BridgeLog {
    pub started: bool,
    pub paged: Option<bool>,
    pub requests: Vec<ListRequest>,
}

impl BridgeLog {
    /// The tools a client was handed, counting each page once however often
    /// it was asked for.
    pub fn distinct_tools(&self) -> usize {
        let mut seen: Vec<&str> = Vec::new();
        self.requests
            .iter()
            .filter(|request| {
                let new = !seen.contains(&request.cursor.as_str());
                seen.push(&request.cursor);
                new
            })
            .map(|request| request.tools)
            .sum()
    }

    /// Whether the client asked for the last page: a request answered with
    /// no next cursor.
    pub fn reached_the_end(&self) -> bool {
        self.requests
            .iter()
            .any(|request| request.next_cursor.is_empty())
    }

    /// Whether every cursor handed out was asked for.
    pub fn followed_every_cursor(&self) -> bool {
        self.requests
            .iter()
            .filter(|request| !request.next_cursor.is_empty())
            .all(|handed| {
                self.requests
                    .iter()
                    .any(|asked| asked.cursor == handed.next_cursor)
            })
    }

    pub fn summary(&self) -> String {
        format!(
            "bridge_started={}, paged={:?}, list_requests={}, cursors_asked={:?}, distinct_tools={}, followed_every_cursor={}, reached_end={}",
            self.started,
            self.paged,
            self.requests.len(),
            self.requests
                .iter()
                .map(|request| request.cursor.as_str())
                .collect::<Vec<_>>(),
            self.distinct_tools(),
            self.followed_every_cursor(),
            self.reached_the_end()
        )
    }
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(&format!(" {name}="))? + name.len() + 2;
    let rest = &line[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next()
    } else {
        rest.split_whitespace().next()
    }
}

/// Read what the bridge started under `root` logged.
pub(crate) fn read(root: &Path) -> BridgeLog {
    let logs: PathBuf = root.join("store").join("logs");
    let mut log = BridgeLog::default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&logs)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("koru-mcp.log"))
        })
        .collect();
    files.sort();
    for text in files
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
    {
        for line in text.lines() {
            if line.contains(" serving stdio") {
                log.started = true;
            } else if line.contains(" tools/list paging ") {
                log.paged = field(line, "paged_tools").map(|value| value == "true");
            } else if line.contains(" tools/list ") && line.contains(" next_cursor=") {
                log.requests.push(ListRequest {
                    cursor: field(line, "cursor").unwrap_or_default().to_owned(),
                    tools: field(line, "tools")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0),
                    next_cursor: field(line, "next_cursor").unwrap_or_default().to_owned(),
                });
            }
        }
    }
    log
}

/// Read the log until the client has reached the last page, or `limit`
/// passes. An agent may list tools after its session call returns.
pub(crate) fn wait_for_the_end(root: &Path, limit: Duration) -> BridgeLog {
    let deadline = Instant::now() + limit;
    loop {
        let log = read(root);
        if log.reached_the_end() || Instant::now() >= deadline {
            return log;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The bridge this tree built, and the argv that starts it paging its tool
/// list and logging under `root`.
pub(crate) fn bridge_launch(root: &Path) -> Option<crate::providers::McpServerLaunch> {
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let bridge = tree.join(if cfg!(windows) {
        "target/debug/koru-mcp.exe"
    } else {
        "target/debug/koru-mcp"
    });
    bridge.is_file().then(|| crate::providers::McpServerLaunch {
        name: "koru".into(),
        command: bridge.into_os_string(),
        args: vec![
            "serve".into(),
            "--stdio".into(),
            "--root".into(),
            root.as_os_str().to_owned(),
            "--tool-pages".into(),
            "on".into(),
        ],
        env: Vec::new(),
    })
}

#[test]
fn a_bridge_log_line_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("store").join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("koru-mcp.log.2026-09-26"),
        "2026-09-26T06:13:32Z  INFO serving stdio version=\"0.0.3\"\n\
         2026-09-26T06:13:32Z  INFO tools/list paging paged_tools=true\n\
         2026-09-26T06:13:32Z  INFO tools/list cursor=\"\" tools=4 next_cursor=\"koru-tools:b\"\n\
         2026-09-26T06:13:32Z  INFO tools/list cursor=\"koru-tools:b\" tools=11 next_cursor=\"koru-tools:c\"\n\
         2026-09-26T06:13:32Z  INFO tools/list cursor=\"koru-tools:c\" tools=6 next_cursor=\"\"\n\
         2026-09-26T06:13:33Z  INFO tools/list cursor=\"\" tools=4 next_cursor=\"koru-tools:b\"\n",
    )
    .unwrap();
    let log = read(dir.path());
    assert!(log.started);
    assert_eq!(log.paged, Some(true));
    assert_eq!(log.requests.len(), 4);
    assert_eq!(log.distinct_tools(), 21);
    assert!(log.followed_every_cursor());
    assert!(log.reached_the_end());
}
