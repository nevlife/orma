//! The disk floor: refused starts and recordings ended by it, on a file system
//! whose free space the test sets.

mod collectd;

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use collectd::*;
use orma_collectd::record::Caps;
use orma_collectd::sys::Disk;
use orma_proto::msg::*;

const GIB: u64 = 1 << 30;
static AVAIL: AtomicU64 = AtomicU64::new(100 * GIB);

fn fake_disk(_: &Path) -> io::Result<Disk> {
    Ok(Disk {
        avail: AVAIL.load(Ordering::Relaxed),
        total: 1000 * GIB,
    })
}

#[test]
fn the_default_floor_is_the_larger_of_2_gib_and_5_percent() {
    let floor = |total| Caps::default().floor(Disk { avail: 0, total });
    assert_eq!(floor(1000 * GIB), 50 * GIB);
    assert_eq!(floor(10 * GIB), 2 * GIB);
    let set = Caps {
        disk_floor_bytes: Some(GIB),
        ..Caps::default()
    };
    assert_eq!(
        set.floor(Disk {
            avail: 0,
            total: 1000 * GIB
        }),
        GIB
    );
}

#[test]
fn the_disk_floor_refuses_and_ends_recordings() {
    let dir = scratch("disk");
    let mut cfg = config(&dir);
    cfg.disk = fake_disk;
    cfg.caps.disk_floor_bytes = Some(10 * GIB);
    let server = Server::start(&dir, cfg);
    let mut c = server.client("orma");

    AVAIL.store(10 * GIB + GIB / 2, Ordering::Relaxed);
    let refused = c.request(ClientOp::RecordStart(options(&dir))).unwrap_err();
    assert!(
        refused.message.contains("disk floor"),
        "{}",
        refused.message
    );

    AVAIL.store(20 * GIB, Ordering::Relaxed);
    let too_big = RecordOptions {
        max_bytes: Some(11 * GIB),
        ..options(&dir)
    };
    assert!(c.request(ClientOp::RecordStart(too_big)).is_err());
    let (id, rec) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    });
    let limits = c.recording(&id).unwrap().limits;
    assert_eq!(limits.max_bytes, GIB);
    assert_eq!(limits.disk_floor_bytes, 10 * GIB);
    let (events, _) = c.start(options(&dir));
    assert_eq!(c.recording(&events).unwrap().limits.max_bytes, 8 * GIB);

    AVAIL.store(9 * GIB, Ordering::Relaxed);
    let r = c.ended(&id);
    assert_eq!(r.end_reason, Some(EndReason::DiskFloor));
    let m = meta(&rec);
    assert_eq!(m.end_reason, Some(EndReason::DiskFloor));
    assert!(m.end_detail.unwrap().contains("under the floor"));
    assert_eq!(c.ended(&events).end_reason, Some(EndReason::DiskFloor));
}
