//! Ring buffer entries into positioned records (design 13.4). The programs write each string into
//! its own fixed slot; here they are packed, cut and flagged as the record format says, the rmw
//! gid length is set from the process's distro, and an `rcl_executor` claim says whether the process
//! maps librclcpp.so.

use orma_common::{bpf::STRING_SLOT, catalog, events::*, read};

const POSITION: usize = size_of::<orma_position>();
const HEADER: usize = size_of::<orma_record>();

/// One converted record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub pid: u32,
    pub tid: u32,
    pub event: u16,
}

/// What a record takes from its process.
pub struct ProcFacts {
    /// `RMW_GID_STORAGE_SIZE` of its distro.
    pub gid_len: u32,
    pub rclcpp: bool,
}

/// Appends the record in `raw` to `out`.
pub fn convert(raw: &[u8], facts: impl Fn(u32) -> ProcFacts, out: &mut Vec<u8>) -> Option<Head> {
    let pos: orma_position = read(raw)?;
    let rec: orma_record = read(&raw[POSITION..])?;
    let info = catalog::info(rec.event)?;
    let body = POSITION + HEADER;
    let strings_at = body + info.size;
    let start = out.len();
    out.extend_from_slice(&raw[..body]);
    let payload_at = out.len();
    out.extend_from_slice(&raw[body..strings_at]);
    let facts = facts(pos.pid);
    gid(rec.event, &mut out[payload_at..], facts.gid_len);
    if rec.event == ORMA_EV_RCL_EXECUTOR_CLAIM {
        let at = payload_at + std::mem::offset_of!(orma_ev_rcl_executor_claim, rclcpp);
        out[at..at + 4].copy_from_slice(&u32::from(facts.rclcpp).to_le_bytes());
    }
    let mut flags = rec.flags & ORMA_RECORD_RCL_EXECUTOR;
    for i in 0..info.strings as usize {
        let slot = &raw[strings_at + i * STRING_SLOT..strings_at + (i + 1) * STRING_SLOT];
        let len = slot.iter().position(|b| *b == 0).unwrap_or(STRING_SLOT);
        // The slot holds 255 bytes and a NUL: 255 means the string went on.
        let keep = len.min(ORMA_STRING_MAX - 1);
        if keep < len {
            flags |= ORMA_RECORD_TRUNCATED;
        }
        out.extend_from_slice(&slot[..keep]);
        out.push(0);
    }
    let len = out.len() - start - POSITION;
    out.resize(
        start + POSITION + len.next_multiple_of(ORMA_RECORD_ALIGN),
        0,
    );
    let len = (out.len() - start - POSITION) as u16;
    let header = orma_record {
        event: rec.event,
        len,
        flags,
        t_mono_ns: rec.t_mono_ns,
    };
    out[start + POSITION..start + POSITION + HEADER]
        .copy_from_slice(orma_common::bytes_of(&header));
    Some(Head {
        pid: pos.pid,
        tid: pos.tid,
        event: rec.event,
    })
}

/// The programs read `ORMA_GID_MAX` bytes; the gid is the distro's first `gid_len` of them.
fn gid(event: u16, payload: &mut [u8], gid_len: u32) {
    let (gid_at, len_at) = match event {
        ORMA_EV_RMW_PUBLISHER_INIT | ORMA_EV_RMW_SUBSCRIPTION_INIT => (8, 32),
        _ => return,
    };
    let n = gid_len as usize;
    payload[gid_at + n..gid_at + ORMA_GID_MAX].fill(0);
    payload[len_at..len_at + 4].copy_from_slice(&(n as u32).to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use orma_common::bytes_of;
    use orma_proto::records;

    fn jazzy(_: u32) -> ProcFacts {
        ProcFacts {
            gid_len: 16,
            rclcpp: false,
        }
    }

    fn raw<T: orma_common::Plain>(event: u16, payload: &T, strings: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(bytes_of(&orma_position { pid: 7, tid: 8 }));
        v.extend_from_slice(bytes_of(&orma_record {
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
        let ev = orma_ev_rcl_node_init {
            node: 1,
            rmw_node: 2,
        };
        let mut out = Vec::new();
        let h = convert(
            &raw(ORMA_EV_RCL_NODE_INIT, &ev, &[b"talker", b"/"]),
            jazzy,
            &mut out,
        )
        .unwrap();
        assert_eq!((h.pid, h.tid, h.event), (7, 8, ORMA_EV_RCL_NODE_INIT));
        let r = records::positioned(&out).next().unwrap().unwrap();
        let s: Vec<&[u8]> = r.strings().collect();
        assert_eq!(s, [&b"talker"[..], &b"/"[..]]);
        assert!(!r.truncated());
        assert_eq!(r.payload::<orma_ev_rcl_node_init>(), ev);
        assert_eq!(r.header.t_mono_ns, 99);
    }

    #[test]
    fn a_full_slot_is_cut_and_flagged() {
        let ev = orma_ev_rclcpp_callback_register { callback: 3 };
        let long = [b'x'; 255];
        let mut out = Vec::new();
        convert(
            &raw(ORMA_EV_RCLCPP_CALLBACK_REGISTER, &ev, &[&long]),
            jazzy,
            &mut out,
        )
        .unwrap();
        let r = records::positioned(&out).next().unwrap().unwrap();
        assert!(r.truncated());
        assert_eq!(r.strings().next().unwrap().len(), ORMA_STRING_MAX - 1);
        let mut out = Vec::new();
        convert(
            &raw(ORMA_EV_RCLCPP_CALLBACK_REGISTER, &ev, &[&long[..254]]),
            jazzy,
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
    fn a_core_wait_keeps_its_flag_and_a_claim_names_its_client() {
        let wait = orma_ev_rcl_wait_enter {
            wait_set: 0x10,
            timeout: -1,
        };
        let mut v = raw(ORMA_EV_RCL_WAIT_ENTER, &wait, &[]);
        v[size_of::<orma_position>() + 4] = (ORMA_RECORD_RCL_EXECUTOR | 0x80) as u8;
        let mut out = Vec::new();
        convert(&v, jazzy, &mut out).unwrap();
        let r = records::positioned(&out).next().unwrap().unwrap();
        assert_eq!(r.header.flags, ORMA_RECORD_RCL_EXECUTOR);
        assert_eq!(r.payload::<orma_ev_rcl_wait_enter>(), wait);

        let claim = orma_ev_rcl_executor_claim {
            executor: 0x10,
            registration: 3,
            entity: 0x20,
            context: 0x30,
            group: 0x40,
            entity_type: 1,
            rclcpp: 7,
        };
        for rclcpp in [false, true] {
            let mut out = Vec::new();
            let facts = |_| ProcFacts {
                gid_len: 16,
                rclcpp,
            };
            convert(
                &raw(ORMA_EV_RCL_EXECUTOR_CLAIM, &claim, &[]),
                facts,
                &mut out,
            )
            .unwrap();
            let r = records::positioned(&out).next().unwrap().unwrap();
            assert_eq!(r.header.flags, 0);
            let got: orma_ev_rcl_executor_claim = r.payload();
            assert_eq!(
                got,
                orma_ev_rcl_executor_claim {
                    rclcpp: u32::from(rclcpp),
                    ..claim
                }
            );
        }
    }

    #[test]
    fn the_gid_takes_the_distro_length() {
        let ev = orma_ev_rmw_endpoint_init {
            rmw_handle: 5,
            gid: [0xEE; ORMA_GID_MAX],
            gid_len: 0,
            pad: 0,
        };
        for len in [16, 24] {
            let mut out = Vec::new();
            convert(
                &raw(ORMA_EV_RMW_PUBLISHER_INIT, &ev, &[]),
                |_| ProcFacts {
                    gid_len: len,
                    rclcpp: false,
                },
                &mut out,
            )
            .unwrap();
            let r = records::positioned(&out).next().unwrap().unwrap();
            let p: orma_ev_rmw_endpoint_init = r.payload();
            assert_eq!(p.gid_len, len);
            assert!(p.gid[..len as usize].iter().all(|b| *b == 0xEE));
            assert!(p.gid[len as usize..].iter().all(|b| *b == 0));
        }
    }
}
