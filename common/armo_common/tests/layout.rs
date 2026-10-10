//! The C headers against the Rust types (design 13.5): every field's offset and size, struct
//! sizes, the slot layout functions, and `armo_emit.h` against the catalog.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use armo_common::catalog::CATALOG;
use armo_common::events::*;
use armo_common::shm::*;

struct Layout {
    name: &'static str,
    size: usize,
    fields: Vec<(&'static str, usize, usize)>,
}

fn field_size<T, F>(_: fn(&T) -> &F) -> usize {
    size_of::<F>()
}

macro_rules! layout {
    ($t:ident { $($f:ident),* $(,)? }) => {
        Layout {
            name: stringify!($t),
            size: size_of::<$t>(),
            fields: vec![$((
                stringify!($f),
                std::mem::offset_of!($t, $f),
                field_size(|v: &$t| &v.$f),
            )),*],
        }
    };
}

fn layouts() -> Vec<Layout> {
    vec![
        layout!(armo_shm_header {
            magic,
            version,
            header_size,
            thread_size,
            n_threads,
            ring_bytes,
            table_entries,
            backend,
            pid,
            starttime,
            armed,
            options,
            reader_gen,
            pad,
            threads_overflow,
            missing,
            reserved,
        }),
        layout!(armo_thread_slot {
            state,
            tid,
            generation,
            depth,
            cur_cb,
            cur_src,
            cur_entry,
            table_overflow,
            depth_overflow,
            reserved0,
            ring_head,
            ring_drops,
            reserved1,
            ring_tail,
            reserved2,
        }),
        layout!(armo_table_entry {
            kind,
            src,
            entry,
            key,
            count,
            sum,
            max,
            hist
        }),
        layout!(armo_record {
            event,
            len,
            flags,
            t_mono_ns
        }),
        layout!(armo_position { pid, tid }),
        layout!(armo_evfile_header {
            magic,
            version,
            pad
        }),
        layout!(armo_ev_rcl_init { context }),
        layout!(armo_ev_rcl_node_init { node, rmw_node }),
        layout!(armo_ev_rmw_endpoint_init {
            rmw_handle,
            gid,
            gid_len,
            pad
        }),
        layout!(armo_ev_rcl_topic_init {
            handle,
            node,
            rmw_handle,
            queue_depth
        }),
        layout!(armo_ev_publish {
            publisher,
            message,
            stamp
        }),
        layout!(armo_ev_rmw_publish {
            rmw_publisher,
            message,
            timestamp,
            stamp
        }),
        layout!(armo_ev_rmw_publish_rcl { message, stamp }),
        layout!(armo_ev_rclcpp_subscription_init {
            subscription_handle,
            subscription
        }),
        layout!(armo_ev_callback_added { handle, callback }),
        layout!(armo_ev_rmw_take {
            rmw_subscription,
            message,
            source_timestamp,
            stamp,
            taken,
            pad
        }),
        layout!(armo_ev_take { message }),
        layout!(armo_ev_rcl_service_init {
            handle,
            node,
            rmw_handle
        }),
        layout!(armo_ev_rcl_timer_init { timer, period }),
        layout!(armo_ev_rclcpp_timer_link_node { timer, node }),
        layout!(armo_ev_rclcpp_callback_register { callback }),
        layout!(armo_ev_callback_start {
            callback,
            is_intra_process,
            pad
        }),
        layout!(armo_ev_handle { handle }),
        layout!(armo_ev_rclcpp_executor_wait_for_work { timeout }),
        layout!(armo_ev_rmw_send_request {
            rmw_client,
            request,
            sequence_number
        }),
        layout!(armo_ev_rmw_take_request {
            rmw_service,
            request,
            sequence_number,
            client_gid,
            gid_len,
            taken,
        }),
        layout!(armo_ev_rmw_send_response {
            rmw_service,
            response,
            sequence_number,
            timestamp,
            client_gid,
            gid_len,
            pad,
        }),
        layout!(armo_ev_rmw_take_response {
            rmw_client,
            response,
            sequence_number,
            source_timestamp,
            taken,
            pad,
        }),
        layout!(armo_ev_rcl_wait_enter { wait_set, timeout }),
        layout!(armo_ev_rcl_wait_exit { wait_set, ret, pad }),
        layout!(armo_ev_rclcpp_object { object, arg }),
        layout!(armo_ev_rclcpp_callback_owner {
            callback,
            object,
            handle,
            node,
            kind,
            pad,
        }),
        layout!(armo_ev_rclcpp_ipm_insert_sub_id_for_pub {
            ipm,
            sub_id,
            pub_id,
            use_take_shared_method,
            pad,
        }),
        layout!(armo_ev_callback_code { callback, words }),
        layout!(armo_ev_rmw_topic { rmw_handle }),
        layout!(armo_ev_dds_serialized_size { size }),
        layout!(armo_ev_flux2_channel_init {
            channel,
            dev,
            ino,
            publisher,
            slot_count,
            slot_size,
            stamp_offset,
        }),
        layout!(armo_ev_flux2_claim {
            channel,
            slot,
            result,
            loan,
            pad
        }),
        layout!(armo_ev_flux2_loan { channel, slot, pad }),
        layout!(armo_ev_flux2_loan_end {
            channel,
            slot,
            result
        }),
        layout!(armo_ev_flux2_commit {
            channel,
            ticket,
            nbytes,
            stamp,
            slot,
            woke
        }),
        layout!(armo_ev_flux2_take {
            channel,
            ticket,
            lost,
            stamp,
            slot,
            pad
        }),
        layout!(armo_ev_flux2_release {
            channel,
            ticket,
            held,
            pad
        }),
        layout!(armo_ev_flux2_refused {
            channel,
            reason,
            pad
        }),
        layout!(armo_ev_flux2_wait {
            executor,
            timeout_ns,
            sources,
            pad
        }),
        layout!(armo_ev_flux2_wake {
            executor,
            result,
            entry
        }),
        layout!(armo_ev_flux2_callback_start {
            executor,
            endpoint,
            entry,
            pad
        }),
        layout!(armo_ev_flux2_callback_end {
            executor,
            channel,
            entry,
            delivered
        }),
        layout!(armo_ev_flux2_endpoint_init {
            endpoint,
            fingerprint,
            role,
            lang
        }),
        layout!(armo_ev_flux2_py_gil { phase, pad }),
        layout!(armo_ev_flux2_py_ready { channel, ticket }),
        layout!(armo_ev_flux2_record_drop {
            channel,
            ticket,
            count,
            reason
        }),
        layout!(armo_ev_sched_switch {
            prev_state,
            next_tid,
            cpu
        }),
        layout!(armo_ev_sched_wakeup { tid, target_cpu }),
    ]
}

fn include_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../collect/armo_preload/include")
}

/// Fields that tile the struct leave no padding, which `Plain` needs, and none unlisted.
#[test]
fn fields_tile_every_struct() {
    for l in layouts() {
        let mut at = 0;
        for (name, offset, size) in &l.fields {
            assert_eq!(*offset, at, "{}.{name} after a gap", l.name);
            at += size;
        }
        assert_eq!(at, l.size, "{} ends past its last field", l.name);
    }
}

#[test]
fn every_payload_type_is_laid_out() {
    let laid: BTreeSet<_> = layouts().iter().map(|l| l.name).collect();
    for e in CATALOG.iter().filter(|e| e.payload != "()") {
        assert!(
            laid.contains(e.payload),
            "{} ({}) has no layout check",
            e.payload,
            e.name
        );
    }
}

#[test]
fn c_agrees_with_rust() {
    let mut c = String::from("#include <stdio.h>\n#include <stddef.h>\n");
    for _ in 0..2 {
        for h in ["armo_shm.h", "armo_events.h", "armo_emit.h"] {
            c += &format!("#include \"armo_preload/{h}\"\n");
        }
    }
    for l in layouts() {
        let t = l.name;
        c += &format!(
            "_Static_assert(sizeof({t}) == {}, \"sizeof {t}\");\n",
            l.size
        );
        for (f, offset, size) in &l.fields {
            c += &format!(
                "_Static_assert(offsetof({t}, {f}) == {offset} && sizeof((({t} *)0)->{f}) == \
                 {size}, \"{t}.{f}\");\n"
            );
        }
    }
    let cases = [
        (1u32, 64u32),
        (3, 4096),
        (ARMO_TABLE_ENTRIES_DEFAULT, ARMO_RING_BYTES_DEFAULT),
    ];
    c += "int main(void)\n{\n";
    for (entries, ring) in cases {
        c += &format!(
            "  printf(\"%zu %zu\\n\", armo_ring_offset({entries}), armo_thread_size({entries}, \
             {ring}));\n"
        );
    }
    c += "  return 0;\n}\n";
    let dir = std::env::temp_dir().join(format!("armo_layout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, exe) = (dir.join("layout.c"), dir.join("layout"));
    std::fs::write(&src, c).unwrap();
    let built = Command::new("cc")
        .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic", "-I"])
        .arg(include_dir())
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(built.success(), "{} failed to build", src.display());
    let out = Command::new(&exe).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let want: String = cases
        .iter()
        .map(|&(e, r)| format!("{} {}\n", armo_ring_offset(e), armo_thread_size(e, r)))
        .collect();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), want);
}

/// `armo_emit_<name>(const <payload> * ev, const char * ...)` for exactly the emitted events.
#[test]
fn emit_header_matches_the_catalog() {
    let text = std::fs::read_to_string(include_dir().join("armo_preload/armo_emit.h")).unwrap();
    let mut declared = BTreeSet::new();
    for decl in text
        .split(';')
        .filter_map(|d| d.split_once("void armo_emit_"))
    {
        let (name, args) = decl.1.split_once('(').unwrap();
        let args = args.trim_end_matches(')');
        let e = CATALOG.iter().find(|e| e.name == name);
        let e = e.unwrap_or_else(|| panic!("armo_emit_{name} has no event"));
        let first = args.split(',').next().unwrap().trim();
        let want = if e.payload == "()" {
            "void".to_string()
        } else {
            format!("const {} * ev", e.payload)
        };
        assert_eq!(first, want, "armo_emit_{name}'s payload");
        assert_eq!(
            args.matches("const char *").count(),
            e.strings as usize,
            "armo_emit_{name}"
        );
        declared.insert(name.trim().to_string());
    }
    let emitted: BTreeSet<_> = CATALOG
        .iter()
        .filter(|e| e.emitted)
        .map(|e| e.name.to_string())
        .collect();
    assert_eq!(declared, emitted);
}

#[test]
fn catalog_holds_every_event_once() {
    let header = include_dir().join("armo_preload/armo_events.h");
    let text = std::fs::read_to_string(header).unwrap();
    let mut defined = BTreeSet::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#define ARMO_EV_") {
            let (name, value) = rest.split_once(' ').unwrap();
            if name != "LIMIT" {
                defined.insert(value.parse::<u16>().unwrap());
            }
        }
    }
    let ids: Vec<_> = CATALOG.iter().map(|e| e.id).collect();
    let unique: BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "an event number twice");
    assert!(ids.iter().all(|id| *id < ARMO_EV_LIMIT));
    defined.remove(&ARMO_EV_PAD);
    assert_eq!(defined, unique);
}
