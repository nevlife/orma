use orma_common::events::*;
use orma_proto::msg::Method;
use orma_proto::records::Record;

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
    CallbackOwner {
        what: Added,
        callback: u64,
        object: u64,
        handle: u64,
        node: u64,
        fqn: String,
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
    RclcppPublish,
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
    FluxWait {
        executor: u64,
    },
    FluxWake {
        executor: u64,
    },
    FluxStart {
        executor: u64,
        entry: u32,
        endpoint: u64,
    },
    FluxEnd {
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
    GpuWedged {
        result: i32,
    },
    SchedSwitch {
        next_tid: u32,
        cpu: u32,
    },
    SchedWakeup {
        tid: u32,
    },
    DdsWrite {
        duration: u64,
    },
    CudaCall {
        duration: u64,
        bytes: u64,
        api: u32,
        error: bool,
    },
    SpanBegin {
        key: u64,
        name: String,
    },
    SpanEnd {
        key: u64,
    },
    FutexWait {
        duration: u64,
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
        ORMA_EV_RCL_NODE_INIT => {
            let p: orma_ev_rcl_node_init = r.payload();
            E::NodeInit {
                node: p.node,
                name: next(),
                ns: next(),
            }
        }
        ORMA_EV_RCL_PUBLISHER_INIT | ORMA_EV_RCL_SUBSCRIPTION_INIT => {
            let p: orma_ev_rcl_topic_init = r.payload();
            let (handle, node, rmw, topic) = (p.handle, p.node, p.rmw_handle, next());
            if r.header.event == ORMA_EV_RCL_PUBLISHER_INIT {
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
        ORMA_EV_RMW_SUBSCRIPTION_TOPIC | ORMA_EV_RMW_PUBLISHER_TOPIC => {
            let p: orma_ev_rmw_topic = r.payload();
            E::RmwTopic {
                rmw: p.rmw_handle,
                topic: next(),
            }
        }
        ORMA_EV_RCLCPP_SUBSCRIPTION_INIT => {
            let p: orma_ev_rclcpp_subscription_init = r.payload();
            E::RclcppSubscriptionInit {
                rcl: p.subscription_handle,
                rclcpp: p.subscription,
            }
        }
        ORMA_EV_RCL_SERVICE_INIT => {
            let p: orma_ev_rcl_service_init = r.payload();
            E::ServiceInit {
                handle: p.handle,
                node: p.node,
                name: next(),
            }
        }
        ORMA_EV_RCL_TIMER_INIT => {
            let p: orma_ev_rcl_timer_init = r.payload();
            E::TimerInit {
                timer: p.timer,
                period: p.period,
            }
        }
        ORMA_EV_RCLCPP_TIMER_LINK_NODE => {
            let p: orma_ev_rclcpp_timer_link_node = r.payload();
            E::TimerLinkNode {
                timer: p.timer,
                node: p.node,
            }
        }
        ORMA_EV_RCL_TIMER_FINI => E::TimerFini {
            timer: r.payload::<orma_ev_handle>().handle,
        },
        ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED
        | ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED
        | ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED => {
            let p: orma_ev_callback_added = r.payload();
            E::CallbackAdded {
                what: match r.header.event {
                    ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED => Added::Sub,
                    ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED => Added::Service,
                    _ => Added::Timer,
                },
                handle: p.handle,
                callback: p.callback,
            }
        }
        ORMA_EV_RCLCPP_CALLBACK_REGISTER => E::CallbackRegister {
            callback: r.payload::<orma_ev_rclcpp_callback_register>().callback,
            symbol: next(),
        },
        ORMA_EV_RCLCPP_CALLBACK_OWNER => {
            let p: orma_ev_rclcpp_callback_owner = r.payload();
            E::CallbackOwner {
                what: match p.kind {
                    ORMA_OWNER_SUBSCRIPTION => Added::Sub,
                    ORMA_OWNER_SERVICE => Added::Service,
                    ORMA_OWNER_TIMER => Added::Timer,
                    _ => return None,
                },
                callback: p.callback,
                object: p.object,
                handle: p.handle,
                node: p.node,
                fqn: next(),
            }
        }
        ORMA_EV_CALLBACK_CODE => {
            let p: orma_ev_callback_code = r.payload();
            let mut words = [p.callback; 6];
            words[1..].copy_from_slice(&p.words);
            E::CallbackCode { words }
        }
        ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION => {
            let p: orma_ev_rclcpp_object = r.payload();
            E::GroupAddSubscription {
                group: p.object,
                rclcpp: p.arg,
            }
        }
        ORMA_EV_RCLCPP_GROUP_ADD_TIMER => E::GroupAddTimer {
            group: r.payload::<orma_ev_rclcpp_object>().object,
        },
        ORMA_EV_CALLBACK_START => {
            let p: orma_ev_callback_start = r.payload();
            E::CallbackStart {
                callback: p.callback,
                intra: p.is_intra_process != 0,
            }
        }
        ORMA_EV_CALLBACK_END => E::CallbackEnd {
            callback: r.payload::<orma_ev_handle>().handle,
        },
        ORMA_EV_RCLCPP_EXECUTOR_GET_NEXT_READY
        | ORMA_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK
        | ORMA_EV_RCLCPP_EXECUTOR_EXECUTE => E::RclcppExecutor,
        ORMA_EV_RCLCPP_PUBLISH => E::RclcppPublish,
        ORMA_EV_RCL_PUBLISH | ORMA_EV_RCLCPP_INTRA_PUBLISH => {
            let p: orma_ev_publish = r.payload();
            if r.header.event == ORMA_EV_RCL_PUBLISH {
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
        ORMA_EV_RMW_PUBLISH => {
            let p: orma_ev_rmw_publish = r.payload();
            E::RmwPublish {
                rmw: p.rmw_publisher,
                timestamp: p.timestamp,
                stamp: p.stamp,
            }
        }
        ORMA_EV_RMW_PUBLISH_RCL => E::RmwPublishRcl,
        ORMA_EV_RMW_TAKE => {
            let p: orma_ev_rmw_take = r.payload();
            if p.taken == 0 {
                return None;
            }
            E::RmwTake {
                rmw: p.rmw_subscription,
                source_timestamp: p.source_timestamp,
                stamp: p.stamp,
            }
        }
        ORMA_EV_DDS_SERIALIZED_SIZE => E::DdsSize {
            size: r.payload::<orma_ev_dds_serialized_size>().size,
        },
        ORMA_EV_RCL_WAIT_ENTER => E::WaitEnter {
            wait_set: r.payload::<orma_ev_rcl_wait_enter>().wait_set,
        },
        ORMA_EV_RCL_WAIT_EXIT => E::WaitExit {
            wait_set: r.payload::<orma_ev_rcl_wait_exit>().wait_set,
        },
        ORMA_EV_FLUX_CHANNEL_INIT => {
            let p: orma_ev_flux_channel_init = r.payload();
            E::ChannelInit {
                channel: p.channel,
                dev: p.dev,
                ino: p.ino,
                publisher: p.publisher != 0,
                segment: next(),
            }
        }
        ORMA_EV_FLUX_CLAIM => {
            let p: orma_ev_flux_claim = r.payload();
            E::Claim {
                channel: p.channel,
                result: p.result,
            }
        }
        ORMA_EV_FLUX_LOAN_END => {
            let p: orma_ev_flux_loan_end = r.payload();
            E::LoanEnd {
                channel: p.channel,
                result: p.result,
            }
        }
        ORMA_EV_FLUX_COMMIT => {
            let p: orma_ev_flux_commit = r.payload();
            E::Commit {
                channel: p.channel,
                ticket: p.ticket,
                nbytes: p.nbytes,
                stamp: p.stamp,
            }
        }
        ORMA_EV_FLUX_TAKE => {
            let p: orma_ev_flux_take = r.payload();
            E::Take {
                channel: p.channel,
                ticket: p.ticket,
                lost: p.lost,
                stamp: p.stamp,
            }
        }
        ORMA_EV_FLUX_WAIT => E::FluxWait {
            executor: r.payload::<orma_ev_flux_wait>().executor,
        },
        ORMA_EV_FLUX_WAKE => E::FluxWake {
            executor: r.payload::<orma_ev_flux_wake>().executor,
        },
        ORMA_EV_FLUX_CALLBACK_START => {
            let p: orma_ev_flux_callback_start = r.payload();
            E::FluxStart {
                executor: p.executor,
                entry: p.entry,
                endpoint: p.endpoint,
            }
        }
        ORMA_EV_FLUX_CALLBACK_END => {
            let p: orma_ev_flux_callback_end = r.payload();
            E::FluxEnd {
                executor: p.executor,
                entry: p.entry,
                delivered: p.delivered != 0,
                channel: p.channel,
                segment: next(),
            }
        }
        ORMA_EV_FLUX_ENDPOINT_INIT => {
            let p: orma_ev_flux_endpoint_init = r.payload();
            E::EndpointInit {
                endpoint: p.endpoint,
                fingerprint: p.fingerprint,
                publisher: p.role == ORMA_FLUX_ROLE_PUBLISHER,
                node: next(),
                key: next(),
                signpost: next(),
            }
        }
        ORMA_EV_FLUX_RECORD_DROP => {
            let p: orma_ev_flux_record_drop = r.payload();
            E::RecordDrop {
                channel: p.channel,
                count: p.count,
            }
        }
        ORMA_EV_FLUX_GPU_WEDGED => E::GpuWedged {
            result: r.payload::<orma_ev_flux_gpu_wedged>().result,
        },
        ORMA_EV_SCHED_SWITCH => {
            let p: orma_ev_sched_switch = r.payload();
            E::SchedSwitch {
                next_tid: p.next_tid,
                cpu: p.cpu,
            }
        }
        ORMA_EV_SCHED_WAKEUP => E::SchedWakeup {
            tid: r.payload::<orma_ev_sched_wakeup>().tid,
        },
        ORMA_EV_DDS_WRITE => E::DdsWrite {
            duration: r.payload::<orma_ev_dds_write>().duration_ns,
        },
        ORMA_EV_CUDA_CALL => {
            let p: orma_ev_cuda_call = r.payload();
            E::CudaCall {
                duration: p.duration_ns,
                bytes: p.bytes,
                api: p.api,
                error: p.result != 0,
            }
        }
        ORMA_EV_SPAN_BEGIN => E::SpanBegin {
            key: r.payload::<orma_ev_span>().key,
            name: next(),
        },
        ORMA_EV_SPAN_END => E::SpanEnd {
            key: r.payload::<orma_ev_span>().key,
        },
        ORMA_EV_FUTEX_WAIT => E::FutexWait {
            duration: r.payload::<orma_ev_futex_wait>().duration_ns,
        },
        _ => return None,
    })
}
