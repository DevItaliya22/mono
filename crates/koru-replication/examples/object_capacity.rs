// SPDX-License-Identifier: MPL-2.0

//! Opt-in synthetic candidate characterization, never a runtime entry point.
//! The Windows launcher must arm the Job before this process starts executing.
#[path = "object_capacity/fixture.rs"]
mod fixture;
#[path = "object_capacity/guard.rs"]
mod guard;

use std::io::{Read, Write};

fn main() {
    if let Err(error) = run() {
        eprintln!("capacity helper refused: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    // Before argument/input/corpus allocation. This is an OS query, not a flag.
    guard::require_job()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let output = match args.as_slice() {
        [mode] if mode == "baseline" => serde_json::json!({"baseline": true}),
        [mode, family, tier] if mode == "generate" => {
            fixture::generate(family, tier.parse().map_err(|_| "tier")?)?
        }
        [mode, index] if mode == "smoke" => {
            fixture::smoke(index.parse().map_err(|_| "smoke index")?)?
        }
        [mode] if mode == "measure" => {
            let mut input = Vec::new();
            std::io::stdin().take(fixture::JSON_CAP + 1).read_to_end(&mut input)
                .map_err(|e| e.to_string())?;
            if input.len() as u64 > fixture::JSON_CAP { return Err("input cap".into()); }
            fixture::measure(serde_json::from_slice(&input).map_err(|e| e.to_string())?)?
        }
        _ => return Err("strict arguments: baseline | generate ordinary/history 65536/262144/1048576 | smoke 0..4 | measure".into()),
    };
    let output = serde_json::to_vec(&output).map_err(|e| e.to_string())?;
    if output.len() as u64 > fixture::JSON_CAP {
        return Err("output cap".into());
    }
    std::io::stdout()
        .write_all(&output)
        .map_err(|e| e.to_string())
}
