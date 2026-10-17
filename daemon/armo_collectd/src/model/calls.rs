//! CUDA driver calls, DDS writes, futex waits and spans (design 20.1, 20.2, 20.4, 20.5).

use armo_common::events::{
    ARMO_CUDA_ALLOC, ARMO_CUDA_GRAPH_LAUNCH, ARMO_CUDA_LAUNCH, ARMO_CUDA_MEMCPY,
    ARMO_CUDA_MEMCPY_ASYNC, ARMO_CUDA_SYNC_CTX, ARMO_CUDA_SYNC_EVENT, ARMO_CUDA_SYNC_STREAM,
};

use super::*;

/// Open spans per thread: a begin whose end never comes (a C caller returning early) would
/// otherwise grow the stack for as long as the thread lives.
const SPAN_DEPTH: usize = 64;

impl Model {
    /// A call that returned inside a callback is that callback's.
    pub(super) fn cuda_call(
        &mut self,
        pid: u32,
        tid: u32,
        duration: u64,
        bytes: u64,
        api: u32,
        error: bool,
    ) {
        self.proc_mut(pid).cuda = true;
        let launch = matches!(api, ARMO_CUDA_LAUNCH | ARMO_CUDA_GRAPH_LAUNCH);
        let copied = match api {
            ARMO_CUDA_MEMCPY | ARMO_CUDA_MEMCPY_ASYNC => bytes,
            _ => 0,
        };
        let sync = matches!(
            api,
            ARMO_CUDA_SYNC_STREAM | ARMO_CUDA_SYNC_CTX | ARMO_CUDA_SYNC_EVENT
        );
        let w = self.win.cuda.entry(pid).or_default();
        w.calls += 1;
        w.launches += u64::from(launch);
        w.memcpy_bytes += copied;
        w.allocs += u64::from(api == ARMO_CUDA_ALLOC);
        w.errors += u64::from(error);
        if sync {
            w.sync.add(duration);
        }
        let Some(f) = self
            .threads
            .get_mut(&(pid, tid))
            .and_then(|th| th.stack.last_mut())
        else {
            return;
        };
        f.cuda_ns += duration;
        if sync {
            f.cuda_sync_ns += duration;
        }
        let cb = f.cb;
        let c = &mut self.win.cbs.entry(cb).or_default().cuda;
        c.calls += 1;
        c.launches += u64::from(launch);
        c.memcpy_bytes += copied;
        c.ns += duration;
        if sync {
            c.sync_ns += duration;
        }
    }

    /// A wait that began before the callback counts from the callback's start.
    pub(super) fn futex_wait(&mut self, pid: u32, tid: u32, t: u64, duration: u64) {
        self.win
            .threads
            .entry((pid, tid))
            .or_default()
            .futex
            .add(duration);
        if let Some(f) = self.thread(pid, tid).stack.last_mut() {
            let inside = duration.min(t.saturating_sub(f.t));
            f.futex_ns += inside;
            let cb = f.cb;
            self.win.cbs.entry(cb).or_default().futex += inside;
        }
    }

    pub(super) fn dds_write(&mut self, pid: u32, tid: u32, t: u64, duration: u64) {
        let th = self.thread(pid, tid);
        match th.dds_topic.take() {
            Some(topic) => self.dds_pair(pid, tid, topic, t, duration),
            None => th.dds_write = Some((t, duration)),
        }
    }

    /// The topic of the publish inside the current `rcl_publish`, for its DDS write.
    pub(super) fn dds_publish(&mut self, pid: u32, tid: u32, topic: TopicIx) {
        let th = self.thread(pid, tid);
        match th.dds_write.take() {
            Some((t, duration)) => self.dds_pair(pid, tid, topic, t, duration),
            None => th.dds_topic = Some(topic),
        }
    }

    fn dds_pair(&mut self, pid: u32, tid: u32, topic: TopicIx, t: u64, duration: u64) {
        let from = self.threads.get(&(pid, tid)).and_then(|th| th.pub_from);
        let w = self.win.topics.entry(topic).or_default();
        w.dds_write.add(duration);
        let start = t.saturating_sub(duration);
        let pre = from.filter(|f| *f <= start).map(|f| start - f);
        if let Some(pre) = pre {
            w.pub_to_dds.add(pre);
        }
        if self.samples.is_some() {
            let topic = self.topic_id(topic);
            self.sample(Sample::Publish {
                topic,
                pid,
                tid,
                t_ns: t,
                pub_to_dds_ns: pre,
                dds_write_ns: duration,
            });
        }
    }

    pub(super) fn span_begin(&mut self, pid: u32, tid: u32, t: u64, key: u64, name: String) {
        let th = self.thread(pid, tid);
        let cb = th.stack.last().map(|f| f.cb);
        let parent = th.spans.last().map(|s| s.id.clone()).unwrap_or_default();
        let cb_id = cb.map(|k| self.cb_id(&k)).unwrap_or_default();
        // The key tells apart two begins of one name in one callback: another string at
        // another call site.
        let id = format!(
            "span:{pid}:{}:{name}@{key:x}#{cb_id}#{parent}",
            self.starttime(pid)
        );
        let spans = &mut self.thread(pid, tid).spans;
        if spans.len() == SPAN_DEPTH {
            spans.remove(0);
        }
        spans.push(OpenSpan {
            key,
            id,
            name,
            cb,
            t,
        });
    }

    /// An end with no begin open on its thread is dropped.
    pub(super) fn span_end(&mut self, pid: u32, tid: u32, t: u64, key: u64) {
        let spans = &mut self.thread(pid, tid).spans;
        let Some(pos) = spans.iter().rposition(|s| s.key == key) else {
            return;
        };
        spans.truncate(pos + 1);
        let s = spans.pop().expect("found above");
        let parent = spans.last().map(|p| p.id.clone());
        let d = t.saturating_sub(s.t);
        self.win
            .spans
            .entry(s.id.clone())
            .or_insert_with(|| SpanWin {
                pid,
                name: s.name,
                cb: s.cb,
                parent,
                dur: LatAcc::default(),
            })
            .dur
            .add(d);
        self.sample(Sample::Span {
            id: s.id,
            duration_ns: d,
        });
    }
}
