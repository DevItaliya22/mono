// SPDX-License-Identifier: MPL-2.0

//! **`koru-voice-bench` — run the protocol, print the tables, write the JSON.**
//!
//! Three exit codes and no others:
//!
//! | | |
//! | --- | --- |
//! | 0 | the run finished — **including when no model was present**, which is the ordinary state of a machine that has not downloaded three gigabytes |
//! | 1 | the run could not be set up: an unreadable corpus, a protocol-revision mismatch, a decision stage with no `t2s` table |
//! | 2 | the command line was not understood |
//!
//! Exit 2 for a bad argument is `koru_workspace_host::cli::parse`'s convention and is
//! deliberate: *an argument that is not understood is not run*. Exit 0 with
//! *model missing* is Plan 0.3's acceptance criterion, and it is the one that
//! looks surprising until you notice that the alternative is a harness nobody
//! can run to find out whether it works.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", koru_voice_bench::cli::usage());
        return;
    }

    let options = match koru_voice_bench::cli::parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("koru-voice-bench: {error}\n");
            eprint!("{}", koru_voice_bench::cli::usage());
            std::process::exit(2);
        }
    };

    let run = match koru_voice_bench::run::execute(&options) {
        Ok(run) => run,
        Err(message) => {
            eprintln!("koru-voice-bench: {message}");
            std::process::exit(1);
        }
    };

    print!("{}", run.render());

    // The JSON is written beside the tables rather than instead of them, and
    // both come from one structure so they cannot disagree. A failure to write
    // it is reported and does **not** change the exit code: the numbers are
    // already on stdout, and turning a full run into a failure over a file
    // permission would throw away hours of paced measurement.
    let path = options.out.join(run.json_file_name());
    match std::fs::write(&path, run.to_json()) {
        Ok(()) => println!("\nwrote {}", path.display()),
        Err(e) => eprintln!("\ncould not write {}: {e}", path.display()),
    }
}
