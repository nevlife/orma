use armo_common::events::*;
use armo_proto::msg::Method;
use armo_proto::records::Record;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    Sub,
    Service,
    Timer,
}

/// The events the model uses, with what it reads of each.
#[derive(Clone, Debug)]
pub enum E {
    NodeInit {
        node: u64,
        name: String,
        ns: String,
    },
    PublisherInit {
        handle: u64,
        node: u64,
        rmw: u64,
        topic: String,
    },
    SubscriptionInit {
        handle: u64,
        node: u64,
        rmw: u64,
        topic: String,
    },
    RmwTopic {
        rmw: u64,
        topic: String,
    },
    RclcppSubscriptionInit {
        rcl: u64,
        rclcpp: u64,
    },
    ServiceInit {
        handle: u64,
        node: u64,
        name: String,
    },
    TimerInit {
        timer: u64,
        period: i64,
    },
    TimerLinkNode {
        timer: u64,
        node: u64,
    },
    TimerFini {
        timer: u64,
    },
    CallbackAdded {
        what: Added,
        handle: u64,
        callback: u64,
    },
    CallbackRegister {
        callback: u64,
        symbol: String,
    },
    CallbackCode {
        words: [u64; 6],
    },
    GroupAddSubscription {
        group: u64,
        rclcpp: u64,
    },
    GroupAddTimer {
        group: u64,
    },
    CallbackStart {
        callback: u64,
        intra: bool,
    },
    CallbackEnd {
        callback: u64,
    },
    RclcppExecutor,
    RclPublish {
        publisher: u64,
        stamp: u64,
    },
    IntraPublish {
        publisher: u64,
        stamp: u64,
    },
    RmwPublish {
        rmw: u64,
        timestamp: i64,
        stamp: u64,
    },
    RmwPublishRcl,
    RmwTake {
        rmw: u64,
        source_timestamp: i64,
        stamp: u64,
    },
    DdsSize {
        size: u64,
    },
    WaitEnter {
        wait_set: u64,
    },
    WaitExit {
        wait_set: u64,
    },
    ChannelInit {
        channel: u64,
        dev: u64,
        ino: u64,
        publisher: bool,
        segment: String,
    },
    Claim {
        channel: u64,
        result: i32,
    },
    LoanEnd {
        channel: u64,
        result: i32,
    },
    Commit {
        channel: u64,
        ticket: u64,
        nbytes: u64,
        stamp: u64,
    },
    Take {
        channel: u64,
        ticket: u64,
        lost: u64,
        stamp: u64,
    },
    Flux2Wait {
        executor: u64,
    },
    Flux2Wake {
        executor: u64,
    },
    Flux2Start {
        executor: u64,
        entry: u32,
        endpoint: u64,
    },
    Flux2End {
        executor: u64,
        entry: u32,
        delivered: bool,
        channel: u64,
        segment: String,
    },
    EndpointInit {
        endpoint: u64,
        fingerprint: u64,
        publisher: bool,
        node: String,
        key: String,
        signpost: String,
    },
    RecordDrop {
        channel: u64,
        count: u32,
    },
    SchedSwitch {
        next_tid: u32,
        cpu: u32,
    },
    SchedWakeup {
        tid: u32,
    },
}

#[derive(Clone, Debug)]
pub struct Ev {
    pub pid: u32,
    pub tid: u32,
    pub t: u64,
    pub method: Method,
    pub e: E,
}

fn text(s: Option<&[u8]>) -> String {
    String::from_utf8_lossy(s.unwrap_or_default()).into_owned()
}

/// `None` for an event the model does not use.
pub fn decode(r: &Record<'_>) -> Option<E> {
    let mut s = r.strings();
    let mut next = || text(s.next());
    Some(match r.header.event {
        ARMO_EV_RCL_NODE_INIT => {
            let p: armo_ev_rcl_node_init = r.payload();
            E::NodeInit {
                node: p.node,
                name: next(),
                ns: next(),
            }
        }
        ARMO_EV_RCL_PUBLISHER_INIT | ARMO_EV_RCL_SUBSCRIPTION_INIT => {
            let p: armo_ev_rcl_topic_init = r.payload();
            let (handle, node, rmw, topic) = (p.handle, p.node, p.rmw_handle, next());
            if r.header.event == ARMO_EV_RCL_PUBLISHER_INIT {
                E::PublisherInit {
                    handle,
                    node,
                    rmw,
                    topic,
                }
            } else {
                E::SubscriptionInit {
                    handle,
                    node,
                    rmw,
                    topic,
                }
            }
        }
        ARMO_EV_RMW_SUBSCRIPTION_TOPIC | ARMO_EV_RMW_PUBLISHER_TOPIC => {
            let p: armo_ev_rmw_topic = r.payload();
            E::RmwTopic {
                rmw: p.rmw_handle,
                topic: next(),
            }
        }
        ARMO_EV_RCLCPP_SUBSCRIPTION_INIT => {
            let p: armo_ev_rclcpp_subscription_init = r.payload();
            E::RclcppSubscriptionInit {
                rcl: p.subscription_handle,
                rclcpp: p.subscription,
            }
        }
        ARMO_EV_RCL_SERVICE_INIT => {
            let p: armo_ev_rcl_service_init = r.payload();
            E::ServiceInit {
                handle: p.handle,
                node: p.node,
                name: next(),
            }
        }
        ARMO_EV_RCL_TIMER_INIT => {
            let p: armo_ev_rcl_timer_init = r.payload();
            E::TimerInit {
                timer: p.timer,
                period: p.period,
            }
        }
        ARMO_EV_RCLCPP_TIMER_LINK_NODE => {
            let p: armo_ev_rclcpp_timer_link_node = r.payload();
            E::TimerLinkNode {
                timer: p.timer,
                node: p.node,
            }
        }
        ARMO_EV_RCL_TIMER_FINI => E::TimerFini {
            timer: r.payload::<armo_ev_handle>().handle,
        },
        ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED
        | ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED
        | ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED => {
            let p: armo_ev_callback_added = r.payload();
            E::CallbackAdded {
                what: match r.header.event {
                    ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED => Added::Sub,
                    ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED => Added::Service,
                    _ => Added::Timer,
                },
                handle: p.handle,
                callback: p.callback,
            }
        }
        ARMO_EV_RCLCPP_CALLBACK_REGISTER => E::CallbackRegister {
            callback: r.payload::<armo_ev_rclcpp_callback_register>().callback,
            symbol: next(),
        },
        ARMO_EV_CALLBACK_CODE => {
            let p: armo_ev_callback_code = r.payload();
            let mut words = [p.callback; 6];
            words[1..].copy_from_slice(&p.words);
            E::CallbackCode { words }
        }
        ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION => {
            let p: armo_ev_rclcpp_object = r.payload();
            E::GroupAddSubscription {
                group: p.object,
                rclcpp: p.arg,
            }
        }
        ARMO_EV_RCLCPP_GROUP_ADD_TIMER => E::GroupAddTimer {
            group: r.payload::<armo_ev_rclcpp_object>().object,
        },
        ARMO_EV_CALLBACK_START => {
            let p: armo_ev_callback_start = r.payload();
            E::CallbackStart {
                callback: p.callback,
                intra: p.is_intra_process != 0,
            }
        }
        ARMO_EV_CALLBACK_END => E::CallbackEnd {
            callback: r.payload::<armo_ev_handle>().handle,
        },
        ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY
        | ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK
        | ARMO_EV_RCLCPP_EXECUTOR_EXECUTE => E::RclcppExecutor,
        ARMO_EV_RCL_PUBLISH | ARMO_EV_RCLCPP_INTRA_PUBLISH => {
            let p: armo_ev_publish = r.payload();
            if r.header.event == ARMO_EV_RCL_PUBLISH {
                E::RclPublish {
                    publisher: p.publisher,
                    stamp: p.stamp,
                }
            } else {
                E::IntraPublish {
                    publisher: p.publisher,
                    stamp: p.stamp,
                }
            }
        }
        ARMO_EV_RMW_PUBLISH => {
            let p: armo_ev_rmw_publish = r.payload();
            E::RmwPublish {
                rmw: p.rmw_publisher,
                timestamp: p.timestamp,
                stamp: p.stamp,
            }
        }
        ARMO_EV_RMW_PUBLISH_RCL => E::RmwPublishRcl,
        ARMO_EV_RMW_TAKE => {
            let p: armo_ev_rmw_take = r.payload();
            if p.taken == 0 {
                return None;
            }
            E::RmwTake {
                rmw: p.rmw_subscription,
                source_timestamp: p.source_timestamp,
                stamp: p.stamp,
            }
        }
        ARMO_EV_DDS_SERIALIZED_SIZE => E::DdsSize {
            size: r.payload::<armo_ev_dds_serialized_size>().size,
        },
        ARMO_EV_RCL_WAIT_ENTER => E::WaitEnter {
            wait_set: r.payload::<armo_ev_rcl_wait_enter>().wait_set,
        },
        ARMO_EV_RCL_WAIT_EXIT => E::WaitExit {
            wait_set: r.payload::<armo_ev_rcl_wait_exit>().wait_set,
        },
        ARMO_EV_FLUX2_CHANNEL_INIT => {
            let p: armo_ev_flux2_channel_init = r.payload();
            E::ChannelInit {
                channel: p.channel,
                dev: p.dev,
                ino: p.ino,
                publisher: p.publisher != 0,
                segment: next(),
            }
        }
        ARMO_EV_FLUX2_CLAIM => {
            let p: armo_ev_flux2_claim = r.payload();
            E::Claim {
                channel: p.channel,
                result: p.result,
            }
        }
        ARMO_EV_FLUX2_LOAN_END => {
            let p: armo_ev_flux2_loan_end = r.payload();
            E::LoanEnd {
                channel: p.channel,
                result: p.result,
            }
        }
        ARMO_EV_FLUX2_COMMIT => {
            let p: armo_ev_flux2_commit = r.payload();
            E::Commit {
                channel: p.channel,
                ticket: p.ticket,
                nbytes: p.nbytes,
                stamp: p.stamp,
            }
        }
        ARMO_EV_FLUX2_TAKE => {
            let p: armo_ev_flux2_take = r.payload();
            E::Take {
                channel: p.channel,
                ticket: p.ticket,
                lost: p.lost,
                stamp: p.stamp,
            }
        }
        ARMO_EV_FLUX2_WAIT => E::Flux2Wait {
            executor: r.payload::<armo_ev_flux2_wait>().executor,
        },
        ARMO_EV_FLUX2_WAKE => E::Flux2Wake {
            executor: r.payload::<armo_ev_flux2_wake>().executor,
        },
        ARMO_EV_FLUX2_CALLBACK_START => {
            let p: armo_ev_flux2_callback_start = r.payload();
            E::Flux2Start {
                executor: p.executor,
                entry: p.entry,
                endpoint: p.endpoint,
            }
        }
        ARMO_EV_FLUX2_CALLBACK_END => {
            let p: armo_ev_flux2_callback_end = r.payload();
            E::Flux2End {
                executor: p.executor,
                entry: p.entry,
                delivered: p.delivered != 0,
                channel: p.channel,
                segment: next(),
            }
        }
        ARMO_EV_FLUX2_ENDPOINT_INIT => {
            let p: armo_ev_flux2_endpoint_init = r.payload();
            E::EndpointInit {
                endpoint: p.endpoint,
                fingerprint: p.fingerprint,
                publisher: p.role == ARMO_FLUX2_ROLE_PUBLISHER,
                node: next(),
                key: next(),
                signpost: next(),
            }
        }
        ARMO_EV_FLUX2_RECORD_DROP => {
            let p: armo_ev_flux2_record_drop = r.payload();
            E::RecordDrop {
                channel: p.channel,
                count: p.count,
            }
        }
        ARMO_EV_SCHED_SWITCH => {
            let p: armo_ev_sched_switch = r.payload();
            E::SchedSwitch {
                next_tid: p.next_tid,
                cpu: p.cpu,
            }
        }
        ARMO_EV_SCHED_WAKEUP => E::SchedWakeup {
            tid: r.payload::<armo_ev_sched_wakeup>().tid,
        },
        _ => return None,
    })
}
