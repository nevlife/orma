//! `orma-collectd`: the per-user daemon. `orma-collectd --guard` is its guard
//!, started by orma-collectd itself.

use std::io;
use std::process::ExitCode;

use orma_collectd::guard;
use orma_collectd::model::Model;
use orma_collectd::server::{self, Config, Exit};
use orma_collectd::sys;

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
                eprintln!("orma-collectd: another orma-collectd serves this user");
            }
        }),
        [flag] if flag == "--guard" => guard::run(io::stdin().lock()),
        _ => {
            eprintln!("usage: orma-collectd [--guard]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("orma-collectd: {e}");
            ExitCode::FAILURE
        }
    }
}
