//! `airlock-eval labels.jsonl scripts.json`
//!
//! Prints a JSON report, then a markdown table. Clips with no script are skipped.

use airlock_pipeline::{ClipScript, parse_labels, score_scripts};
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(labels_path) = args.next() else {
        eprintln!("usage: airlock-eval <labels.jsonl> <scripts.json>");
        return ExitCode::from(2);
    };
    let Some(scripts_path) = args.next() else {
        eprintln!("usage: airlock-eval <labels.jsonl> <scripts.json>");
        return ExitCode::from(2);
    };
    let labels = match fs::read_to_string(&labels_path) {
        Ok(text) => match parse_labels(&text) {
            Ok(labels) => labels,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        },
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let scripts: Vec<ClipScript> = match fs::read_to_string(&scripts_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(scripts) => scripts,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let report = score_scripts(&labels, &scripts);
    println!("{}", report.to_json());
    println!();
    println!("{}", report.to_markdown());
    ExitCode::SUCCESS
}
