//! `armo-fake-server SOCKET`: serves the fake armo-collectd (`armo_proto::fake::server`) until
//! killed, printing each request it receives as one JSON line.

use std::path::PathBuf;

fn main() {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: armo-fake-server SOCKET");
        std::process::exit(2);
    };
    let server = armo_proto::fake::server::FakeServer::start(&path, Box::new(std::io::stdout()));
    if let Err(e) = server {
        eprintln!("armo-fake-server: {}: {e}", path.display());
        std::process::exit(1);
    }
    loop {
        std::thread::park();
    }
}
