//! `armo-collectd`: the per-user daemon. `armo-collectd --guard` is its guard
//!, started by armo-collectd itself.

use std::io;
use std::process::ExitCode;

use armo_collectd::guard;
use armo_collectd::model::Model;
use armo_collectd::server::{self, Config, Exit};
use armo_collectd::sys;

fn serve() -> io::Result<Exit> {
    let stop = sys::stop_on_signals();
    sys::raise_nofile();
    server::run(Config::from_env()?, Box::new(Model::live()), stop)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => serve().map(|exit| {
            if exit == Exit::Running {
                eprintln!("armo-collectd: another armo-collectd serves this user");
            }
        }),
        [flag] if flag == "--guard" => guard::run(io::stdin().lock()),
        _ => {
            eprintln!("usage: armo-collectd [--guard]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("armo-collectd: {e}");
            ExitCode::FAILURE
        }
    }
}
