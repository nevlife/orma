//! Ring buffer entries into positioned records (design 13.4). The programs write each string into
//! its own fixed slot; here they are packed, cut and flagged as the record format says, and the
//! rmw gid length is set from the process's distro.

use armo_common::{bpf::STRING_SLOT, catalog, events::*, read};

const POSITION: usize = size_of::<armo_position>();
const HEADER: usize = size_of::<armo_record>();

/// One converted record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub pid: u32,
    pub tid: u32,
    pub event: u16,
}

/// Appends the record in `raw` to `out`. `gid_len` is the process's `RMW_GID_STORAGE_SIZE`.
pub fn convert(raw: &[u8], gid_len: impl Fn(u32) -> u32, out: &mut Vec<u8>) -> Option<Head> {
    let pos: armo_position = read(raw)?;
    let rec: armo_record = read(&raw[POSITION..])?;
    let info = catalog::info(rec.event)?;
    let body = POSITION + HEADER;
    let strings_at = body + info.size;
    let start = out.len();
    out.extend_from_slice(&raw[..body]);
    let payload_at = out.len();
    out.extend_from_slice(&raw[body..strings_at]);
    gid(rec.event, &mut out[payload_at..], gid_len(pos.pid));
    let mut flags = 0;
    for i in 0..info.strings as usize {
        let slot = &raw[strings_at + i * STRING_SLOT..strings_at + (i + 1) * STRING_SLOT];
        let len = slot.iter().position(|b| *b == 0).unwrap_or(STRING_SLOT);
        // The slot holds 255 bytes and a NUL: 255 means the string went on.
        let keep = len.min(ARMO_STRING_MAX - 1);
        if keep < len {
            flags |= ARMO_RECORD_TRUNCATED;
        }
        out.extend_from_slice(&slot[..keep]);
        out.push(0);
    }
    let len = out.len() - start - POSITION;
    out.resize(
        start + POSITION + len.next_multiple_of(ARMO_RECORD_ALIGN),
        0,
    );
    let len = (out.len() - start - POSITION) as u16;
    let header = armo_record {
        event: rec.event,
        len,
        flags,
        t_mono_ns: rec.t_mono_ns,
    };
    out[start + POSITION..start + POSITION + HEADER]
        .copy_from_slice(armo_common::bytes_of(&header));
    Some(Head {
        pid: pos.pid,
        tid: pos.tid,
        event: rec.event,
    })
}

/// The programs read `ARMO_GID_MAX` bytes; the gid is the distro's first `gid_len` of them.
fn gid(event: u16, payload: &mut [u8], gid_len: u32) {
    let (gid_at, len_at) = match event {
        ARMO_EV_RMW_PUBLISHER_INIT | ARMO_EV_RMW_SUBSCRIPTION_INIT | ARMO_EV_RMW_CLIENT_INIT => {
            (8, 32)
        }
        ARMO_EV_RMW_TAKE_REQUEST => (24, 48),
        ARMO_EV_RMW_SEND_RESPONSE => (32, 56),
        _ => return,
    };
    let n = gid_len as usize;
    payload[gid_at + n..gid_at + ARMO_GID_MAX].fill(0);
    payload[len_at..len_at + 4].copy_from_slice(&(n as u32).to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use armo_common::bytes_of;
    use armo_proto::records;

    fn raw<T: armo_common::Plain>(event: u16, payload: &T, strings: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(bytes_of(&armo_position { pid: 7, tid: 8 }));
        v.extend_from_slice(bytes_of(&armo_record {
            event,
            len: 0,
            flags: 0,
            t_mono_ns: 99,
        }));
        v.extend_from_slice(bytes_of(payload));
        for s in strings {
            let mut slot = [0xAAu8; STRING_SLOT];
            slot[..s.len()].copy_from_slice(s);
            if s.len() < STRING_SLOT {
                slot[s.len()] = 0;
            }
            v.extend_from_slice(&slot);
        }
        v
    }

    #[test]
    fn strings_are_packed_and_the_record_checks() {
        let ev = armo_ev_rcl_node_init {
            node: 1,
            rmw_node: 2,
        };
        let mut out = Vec::new();
        let h = convert(
            &raw(ARMO_EV_RCL_NODE_INIT, &ev, &[b"talker", b"/"]),
            |_| 16,
            &mut out,
        )
        .unwrap();
        assert_eq!((h.pid, h.tid, h.event), (7, 8, ARMO_EV_RCL_NODE_INIT));
        let r = records::positioned(&out).next().unwrap().unwrap();
        let s: Vec<&[u8]> = r.strings().collect();
        assert_eq!(s, [&b"talker"[..], &b"/"[..]]);
        assert!(!r.truncated());
        assert_eq!(r.payload::<armo_ev_rcl_node_init>(), ev);
        assert_eq!(r.header.t_mono_ns, 99);
    }

    #[test]
    fn a_full_slot_is_cut_and_flagged() {
        let ev = armo_ev_rclcpp_callback_register { callback: 3 };
        let long = [b'x'; 255];
        let mut out = Vec::new();
        convert(
            &raw(ARMO_EV_RCLCPP_CALLBACK_REGISTER, &ev, &[&long]),
            |_| 16,
            &mut out,
        )
        .unwrap();
        let r = records::positioned(&out).next().unwrap().unwrap();
        assert!(r.truncated());
        assert_eq!(r.strings().next().unwrap().len(), ARMO_STRING_MAX - 1);
        let mut out = Vec::new();
        convert(
            &raw(ARMO_EV_RCLCPP_CALLBACK_REGISTER, &ev, &[&long[..254]]),
            |_| 16,
            &mut out,
        )
        .unwrap();
        assert!(
            !records::positioned(&out)
                .next()
                .unwrap()
                .unwrap()
                .truncated()
        );
    }

    #[test]
    fn the_gid_takes_the_distro_length() {
        let ev = armo_ev_rmw_endpoint_init {
            rmw_handle: 5,
            gid: [0xEE; ARMO_GID_MAX],
            gid_len: 0,
            pad: 0,
        };
        for len in [16, 24] {
            let mut out = Vec::new();
            convert(
                &raw(ARMO_EV_RMW_PUBLISHER_INIT, &ev, &[]),
                |_| len,
                &mut out,
            )
            .unwrap();
            let r = records::positioned(&out).next().unwrap().unwrap();
            let p: armo_ev_rmw_endpoint_init = r.payload();
            assert_eq!(p.gid_len, len);
            assert!(p.gid[..len as usize].iter().all(|b| *b == 0xEE));
            assert!(p.gid[len as usize..].iter().all(|b| *b == 0));
        }
    }
}
