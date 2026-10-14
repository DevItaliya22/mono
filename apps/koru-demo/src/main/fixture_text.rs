// SPDX-License-Identifier: MPL-2.0

//! Offline UTF-8 fixture artifacts. This command never attaches to an app.
use std::{io::Write, path::PathBuf, process::ExitCode};

pub fn run(arguments: &[String]) -> ExitCode {
    let result = (|| -> Result<(), &'static str> {
        let name = arguments
            .get(1)
            .ok_or("fixture-text requires a compiled case name")?;
        let mut output = None;
        let mut count = koru_demo::fixtures::FILLERS;
        let mut flags = arguments.iter().skip(2);
        while let Some(flag) = flags.next() {
            let value = flags.next().ok_or("fixture-text flag requires a value")?;
            match flag.as_str() {
                "--output" if output.is_none() => output = Some(PathBuf::from(value)),
                "--filler-count" => count = value.parse().map_err(|_| "invalid filler count")?,
                _ => return Err("unknown or duplicate fixture-text flag"),
            }
        }
        let text = koru_demo::fixtures::document_text_with_count(name, count)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.ok_or("fixture-text requires --output <new.txt>")?)
            .map_err(|_| "fixture output exists or cannot be created")?;
        file.write_all(text.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| "fixture output write failed")?;
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("koru-demo: {code}");
            ExitCode::from(2)
        }
    }
}
