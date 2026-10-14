// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn dispatch_returns_when_no_worker_argument_is_present() {
    // The diverging paths (a role runs, a refusal exits 2) are process
    // behavior and live in tests/worker_process.rs; this pins the one path
    // that returns.
    let roles = [Role {
        name: "fake",
        run: |_| unreachable!("no --worker argument was given"),
    }];
    let args: Vec<String> = vec!["koru-agent.exe".into(), "run".into(), "--tray".into()];
    dispatch(args, &roles);
}

#[test]
fn a_bootstrap_line_is_read_up_to_its_newline_and_the_reader_stays_open() {
    let mut reader = std::io::Cursor::new(b"hello worker\nrest stays".to_vec());
    let line = read_bootstrap_line(&mut reader).unwrap();
    assert_eq!(line, b"hello worker");
    let mut rest = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut rest).unwrap();
    assert_eq!(rest, b"rest stays");
}

#[test]
fn the_parent_watcher_fires_once_the_pipe_ends() {
    let (tx, rx) = std::sync::mpsc::channel();
    ParentPipe::new(std::io::Cursor::new(b"leftover bytes".to_vec()))
        .watch("test parent", move || {
            let _ = tx.send(());
        })
        .unwrap();
    // Bytes still in the pipe are not a death; the end of it is. A watcher
    // that stopped at the first byte would report a parent loss that had not
    // happened, which is exactly what the previous one-byte read did.
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("the watcher reports the end of the pipe");
}

#[test]
fn an_unterminated_bootstrap_is_refused_at_the_bound() {
    let mut reader = std::io::Cursor::new(vec![b'x'; 128 * 1024]);
    let error = read_bootstrap_line(&mut reader).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}
