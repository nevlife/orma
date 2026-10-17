//! `armo-analyze DIR [--chain NAME=t1,t2,...[:first]]...`: the analysis behind `armo analyze`.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: armo-analyze RECORDING_DIR [--chain NAME=t1,t2,...[:first]]...";

fn main() -> ExitCode {
    let mut dir = None;
    let mut chains = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let spec = match a.strip_prefix("--chain") {
            Some("") => args.next(),
            Some(v) if v.starts_with('=') => Some(v[1..].to_string()),
            Some(_) => None,
            None if dir.is_none() => {
                dir = Some(PathBuf::from(a));
                continue;
            }
            None => None,
        };
        match spec.map(|s| armo_collectd::analyze::parse_chain(&s)) {
            Some(Ok(c)) => chains.push(c),
            Some(Err(e)) => {
                eprintln!("armo-analyze: {e}");
                return ExitCode::from(2);
            }
            None => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match armo_collectd::analyze::run(&dir, chains) {
        Ok(report) => {
            print!("{}", report.text);
            println!(
                "\n{} records, {} windows; per-message and per-callback CSV in {}",
                report.records,
                report.windows,
                dir.join("analysis").display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("armo-analyze: {e}");
            ExitCode::FAILURE
        }
    }
}
