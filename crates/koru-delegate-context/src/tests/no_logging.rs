// SPDX-License-Identifier: MPL-2.0

//! "No filename, title or body text reaches a log" is held the strong way:
//! this crate writes no log line at all. A tree scan over `src/` (tests
//! excluded) refuses any logging macro or standard-stream print, so the
//! privacy test over `Debug` and `Display` covers every byte the crate can
//! emit.

use std::path::Path;

#[test]
fn the_crate_emits_no_log_line() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    scan(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "logging in a crate that must not: {offenders:?}"
    );
}

fn scan(dir: &Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            scan(&path, offenders);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (line, content) in text.lines().enumerate() {
            let code = content.split("//").next().unwrap_or("");
            for needle in [
                "tracing::",
                "log::",
                "println!",
                "eprintln!",
                "print!(",
                "eprint!(",
                "dbg!(",
            ] {
                if code.contains(needle) {
                    offenders.push(format!(
                        "{}:{}: {needle}",
                        path.file_name().unwrap().to_string_lossy(),
                        line + 1
                    ));
                }
            }
        }
    }
}
