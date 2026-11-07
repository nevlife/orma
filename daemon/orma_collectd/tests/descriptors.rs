//! Alone in its binary: the descriptor limit is the whole process's.
#![allow(unsafe_code)]

mod collectd;

use std::os::unix::net::UnixStream;
use std::time::Duration;

use collectd::*;

fn limit_descriptors(rlim_cur: u64, rlim_max: u64) {
    let limit = libc::rlimit { rlim_cur, rlim_max };
    // SAFETY: `limit` is live for the call.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) };
    assert_eq!(rc, 0);
}

fn open_descriptors() -> u64 {
    std::fs::read_dir("/proc/self/fd").unwrap().count() as u64 - 1
}

#[test]
fn a_client_the_server_cannot_serve_does_not_end_it() {
    let dir = scratch("descriptors");
    let server = Server::start(&dir, config(&dir));
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is live for the call.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
    assert_eq!(rc, 0);
    limit_descriptors(open_descriptors() + 2, limit.rlim_max);
    let refused = UnixStream::connect(&server.socket).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    limit_descriptors(limit.rlim_cur, limit.rlim_max);
    drop(refused);
    let mut c = server.client("orma");
    c.request(orma_proto::msg::ClientOp::Subscribe {
        streams: vec![orma_proto::msg::Stream::Window],
    })
    .unwrap();
}
