//! `armo-analyze DIR`: the analysis behind `armo analyze`.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        eprintln!("usage: armo-analyze RECORDING_DIR");
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);
    match armo_collectd::analyze::run(&dir) {
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
