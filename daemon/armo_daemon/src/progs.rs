//! Every uprobe program, where it attaches and in which probe set (design 2, 6.1, 12.8).

use crate::libs::Lib;

/// Probe sets: those of the protocol plus flux v1, which only the text mode attaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Set {
    Discovery,
    Hot,
    Bandwidth,
    RclcppSymbols,
    Sched,
    Flux1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sym {
    /// `ros_trace_<name>`. The gated `ros_trace_do_<name>` is not attached: a uprobe cannot
    /// open the gate (design 2.4).
    Trace(&'static str),
    Name(&'static str),
    /// The front of a mangled name (design 2.1.2). `true`: overloads all attach.
    Prefix(&'static str, bool),
    /// A symbol only the dbgsym file has, by the front of its name.
    Debug(&'static str),
}

/// Which files of the library get the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    Always,
    /// `ros_trace_rmw_publish(handle, message, timestamp)`: tracetools 8 and later.
    RmwAbi,
    /// `ros_trace_rmw_publish(message)`: Humble.
    RclAbi,
    Rclcpp16,
    Rclcpp28,
}

#[derive(Clone, Copy, Debug)]
pub struct Prog {
    /// The eBPF function's name.
    pub name: &'static str,
    /// The collection point, as `probes_missing` names it: the catalog's event name.
    pub point: &'static str,
    pub lib: Lib,
    pub set: Set,
    pub ret: bool,
    pub sym: Sym,
    pub cond: Cond,
}

const fn p(name: &'static str, lib: Lib, set: Set, sym: Sym) -> Prog {
    Prog {
        name,
        point: name,
        lib,
        set,
        ret: false,
        sym,
        cond: Cond::Always,
    }
}

const fn tp(name: &'static str, set: Set) -> Prog {
    p(name, Lib::Tracetools, set, Sym::Trace(name))
}

const fn cxx(name: &'static str, point: &'static str, sym: Sym, cond: Cond) -> Prog {
    Prog {
        name,
        point,
        lib: Lib::Rclcpp,
        set: Set::RclcppSymbols,
        ret: false,
        sym,
        cond,
    }
}

use Set::*;

pub const PROGS: &[Prog] = &[
    // Discovery: init, registration and fini, every process (design 6.1).
    tp("rcl_init", Discovery),
    tp("rcl_node_init", Discovery),
    tp("rmw_publisher_init", Discovery),
    tp("rcl_publisher_init", Discovery),
    tp("rmw_subscription_init", Discovery),
    tp("rcl_subscription_init", Discovery),
    tp("rclcpp_subscription_init", Discovery),
    tp("rclcpp_subscription_callback_added", Discovery),
    tp("rcl_service_init", Discovery),
    tp("rclcpp_service_callback_added", Discovery),
    tp("rcl_client_init", Discovery),
    tp("rmw_client_init", Discovery),
    tp("rcl_timer_init", Discovery),
    tp("rclcpp_timer_callback_added", Discovery),
    tp("rclcpp_timer_link_node", Discovery),
    tp("rclcpp_callback_register", Discovery),
    p(
        "rcl_timer_fini",
        Lib::Rcl,
        Discovery,
        Sym::Name("rcl_timer_fini"),
    ),
    p(
        "flux2_channel_init",
        Lib::Flux2,
        Discovery,
        Sym::Name("flux2_trace_channel_init"),
    ),
    p(
        "flux2_endpoint_init",
        Lib::Flux2,
        Discovery,
        Sym::Name("flux2_trace_endpoint_init"),
    ),
    // Hot: per message and per callback, the selected processes only.
    tp("rclcpp_publish", Hot),
    tp("rclcpp_intra_publish", Hot),
    tp("rcl_publish", Hot),
    Prog {
        cond: Cond::RmwAbi,
        ..tp("rmw_publish", Hot)
    },
    Prog {
        name: "rmw_publish_rcl",
        point: "rmw_publish_rcl",
        cond: Cond::RclAbi,
        ..tp("rmw_publish", Hot)
    },
    tp("rmw_take", Hot),
    tp("rcl_take", Hot),
    tp("rclcpp_take", Hot),
    tp("callback_start", Hot),
    tp("callback_end", Hot),
    tp("rclcpp_executor_get_next_ready", Hot),
    tp("rclcpp_executor_wait_for_work", Hot),
    tp("rclcpp_executor_execute", Hot),
    tp("rmw_send_request", Hot),
    tp("rmw_take_request", Hot),
    tp("rmw_send_response", Hot),
    tp("rmw_take_response", Hot),
    Prog {
        point: "rcl_wait_enter",
        ..p("rcl_wait", Lib::Rcl, Hot, Sym::Name("rcl_wait"))
    },
    Prog {
        point: "rcl_wait_exit",
        ret: true,
        ..p("rcl_wait_ret", Lib::Rcl, Hot, Sym::Name("rcl_wait"))
    },
    p(
        "flux2_claim",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_claim"),
    ),
    p("flux2_loan", Lib::Flux2, Hot, Sym::Name("flux2_trace_loan")),
    p(
        "flux2_loan_end",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_loan_end"),
    ),
    p(
        "flux2_commit",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_commit"),
    ),
    p("flux2_take", Lib::Flux2, Hot, Sym::Name("flux2_trace_take")),
    p(
        "flux2_release",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_release"),
    ),
    p(
        "flux2_refused",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_refused"),
    ),
    p("flux2_wait", Lib::Flux2, Hot, Sym::Name("flux2_trace_wait")),
    p("flux2_wake", Lib::Flux2, Hot, Sym::Name("flux2_trace_wake")),
    p(
        "flux2_callback_start",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_callback_start"),
    ),
    p(
        "flux2_callback_end",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_callback_end"),
    ),
    p(
        "flux2_py_gil",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_py_gil"),
    ),
    p(
        "flux2_py_ready",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_py_ready"),
    ),
    p(
        "flux2_record_drop",
        Lib::Flux2,
        Hot,
        Sym::Name("flux2_trace_record_drop"),
    ),
    // rclcpp C++ symbols, by rclcpp major (design 2.1.2, 4.5).
    cxx(
        "rclcpp_publisher_setup_intra",
        "rclcpp_publisher_setup_intra",
        Sym::Prefix("_ZN6rclcpp13PublisherBase19setup_intra_processE", false),
        Cond::Always,
    ),
    cxx(
        "rclcpp_subscription_setup_intra",
        "rclcpp_subscription_setup_intra",
        Sym::Prefix("_ZN6rclcpp16SubscriptionBase19setup_intra_processE", false),
        Cond::Always,
    ),
    cxx(
        "rclcpp_ipm_insert_sub_id_for_pub",
        "rclcpp_ipm_insert_sub_id_for_pub",
        Sym::Prefix(
            "_ZN6rclcpp12experimental19IntraProcessManager21insert_sub_id_for_pubE",
            false,
        ),
        Cond::Always,
    ),
    cxx(
        "rclcpp_group_add_subscription",
        "rclcpp_group_add_subscription",
        Sym::Prefix("_ZN6rclcpp13CallbackGroup16add_subscriptionE", false),
        Cond::Always,
    ),
    cxx(
        "rclcpp_group_add_timer",
        "rclcpp_group_add_timer",
        Sym::Prefix("_ZN6rclcpp13CallbackGroup9add_timerE", false),
        Cond::Always,
    ),
    cxx(
        "rclcpp_publisher_base",
        "rclcpp_publisher_base",
        Sym::Prefix("_ZN6rclcpp13PublisherBaseC", false),
        Cond::Always,
    ),
    cxx(
        "rclcpp_executor",
        "rclcpp_executor",
        Sym::Prefix("_ZN6rclcpp8ExecutorC", true),
        Cond::Always,
    ),
    cxx(
        "rclcpp_executor_add_group",
        "rclcpp_executor_add_group",
        Sym::Prefix("_ZN6rclcpp8Executor25add_callback_group_to_mapE", false),
        Cond::Rclcpp16,
    ),
    cxx(
        "rclcpp_collector",
        "rclcpp_collector",
        Sym::Prefix("_ZN6rclcpp9executors25ExecutorEntitiesCollectorC", false),
        Cond::Rclcpp28,
    ),
    cxx(
        "rclcpp_collector_add_group_to_collection",
        "rclcpp_collector_add_group_to_collection",
        Sym::Prefix(
            "_ZN6rclcpp9executors25ExecutorEntitiesCollector32add_callback_group_to_collectionE",
            false,
        ),
        Cond::Rclcpp28,
    ),
    cxx(
        "rclcpp_collector_add_group",
        "rclcpp_collector_add_group",
        Sym::Prefix(
            "_ZN6rclcpp9executors25ExecutorEntitiesCollector18add_callback_groupE",
            false,
        ),
        Cond::Rclcpp28,
    ),
    // DDS serialized size, opt-in (design 2.1.2).
    Prog {
        point: "dds_serialized_size",
        ..p(
            "fastdds_serialize",
            Lib::Fastdds,
            Bandwidth,
            Sym::Prefix(
                "_ZN23rmw_fastrtps_shared_cpp11TypeSupport9serializeE",
                false,
            ),
        )
    },
    Prog {
        point: "dds_serialized_size",
        ret: true,
        ..p(
            "fastdds_serialize_ret",
            Lib::Fastdds,
            Bandwidth,
            Sym::Prefix(
                "_ZN23rmw_fastrtps_shared_cpp11TypeSupport9serializeE",
                false,
            ),
        )
    },
    Prog {
        point: "dds_serialized_size",
        ..p(
            "cyclone_from_sample",
            Lib::Cyclone,
            Bandwidth,
            Sym::Debug("_ZL23serdata_rmw_from_sample"),
        )
    },
    Prog {
        point: "dds_serialized_size",
        ret: true,
        ..p(
            "cyclone_from_sample_ret",
            Lib::Cyclone,
            Bandwidth,
            Sym::Debug("_ZL23serdata_rmw_from_sample"),
        )
    },
    // flux v1, text mode only (design D12).
    p(
        "flux_callback_start",
        Lib::Flux,
        Flux1,
        Sym::Name("flux_trace_callback_start"),
    ),
    p(
        "flux_callback_end",
        Lib::Flux,
        Flux1,
        Sym::Name("flux_trace_callback_end"),
    ),
    p(
        "flux_channel_init",
        Lib::Flux,
        Flux1,
        Sym::Name("flux_trace_channel_init"),
    ),
];

pub const SCHED_PROGS: [&str; 2] = ["sched_switch", "sched_wakeup"];

/// Events a process shows once per entity: they name what the hot events carry, so they go to
/// every client that watches the process, streamed or not.
pub fn naming(event: u16) -> bool {
    use armo_common::events::*;
    matches!(
        event,
        ARMO_EV_RCL_INIT
            | ARMO_EV_RCL_NODE_INIT
            | ARMO_EV_RMW_PUBLISHER_INIT
            | ARMO_EV_RCL_PUBLISHER_INIT
            | ARMO_EV_RMW_SUBSCRIPTION_INIT
            | ARMO_EV_RCL_SUBSCRIPTION_INIT
            | ARMO_EV_RCLCPP_SUBSCRIPTION_INIT
            | ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED
            | ARMO_EV_RCL_SERVICE_INIT
            | ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED
            | ARMO_EV_RCL_CLIENT_INIT
            | ARMO_EV_RCL_TIMER_INIT
            | ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED
            | ARMO_EV_RCLCPP_TIMER_LINK_NODE
            | ARMO_EV_RCLCPP_CALLBACK_REGISTER
            | ARMO_EV_RMW_CLIENT_INIT
            | ARMO_EV_RCL_TIMER_FINI
            ..=ARMO_EV_RMW_PUBLISHER_TOPIC
                | ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP
                | ARMO_EV_FLUX2_CHANNEL_INIT
                | ARMO_EV_FLUX2_ENDPOINT_INIT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use armo_common::{catalog, events::*};

    #[test]
    fn every_point_is_a_catalog_event() {
        for p in PROGS.iter().filter(|p| p.set != Flux1) {
            assert!(
                catalog::CATALOG.iter().any(|e| e.name == p.point),
                "{} is no event",
                p.point
            );
        }
    }

    #[test]
    fn program_names_are_unique() {
        let mut names: Vec<_> = PROGS.iter().map(|p| p.name).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn naming_events_are_the_rare_ones() {
        assert!(naming(ARMO_EV_RCL_NODE_INIT));
        assert!(naming(ARMO_EV_CALLBACK_CODE));
        assert!(naming(ARMO_EV_RMW_SUBSCRIPTION_TOPIC));
        assert!(naming(ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION));
        assert!(!naming(ARMO_EV_RMW_PUBLISH));
        assert!(!naming(ARMO_EV_RCL_PUBLISH));
        assert!(!naming(ARMO_EV_RMW_TAKE));
        assert!(!naming(ARMO_EV_CALLBACK_START));
        assert!(!naming(ARMO_EV_DDS_SERIALIZED_SIZE));
        assert!(!naming(ARMO_EV_FLUX2_COMMIT));
    }
}
