/*
 * Copyright (c) 2022 Yunshan Networks
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thread::JoinHandle;

use arc_swap::access::Access;
use log::{debug, info, warn};
use rand::prelude::{Rng, SeedableRng, SmallRng};

use super::consts::*;
use super::round_to_minute;

use crate::common::{enums::TapType, flow::CloseType, tagged_flow::TaggedFlow};
use crate::config::handler::CollectorAccess;
use crate::sender::SendItem;
use crate::utils::{
    queue::{DebugSender, Error, Receiver},
    stats::{Counter, CounterType, CounterValue, RefCountable},
};

const MINUTE_SLOTS: usize = 2;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2 * SECONDS_IN_MINUTE);
const QUEUE_READ_TIMEOUT: Duration = Duration::from_secs(2);
const TAPTYPE_MAX: usize = 256; // TapType::Max

#[derive(Debug, Default)]
struct FlowAggrCounter {
    drop_before_window: AtomicU64,
    out: AtomicU64,
    drop_in_throttle: AtomicU64,
}

pub struct FlowAggrThread {
    id: usize,
    input: Arc<Receiver<Arc<TaggedFlow>>>,
    output: DebugSender<SendItem>,
    config: CollectorAccess,

    thread_handle: Option<JoinHandle<()>>,

    running: Arc<AtomicBool>,
}

impl FlowAggrThread {
    pub fn new(
        id: usize,
        input: Receiver<Arc<TaggedFlow>>,
        output: DebugSender<SendItem>,
        config: CollectorAccess,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(false));
        Self {
            id,
            input: Arc::new(input),
            output: output.clone(),
            thread_handle: None,
            config,
            running,
        }
    }

    pub fn start(&mut self) {
        if self.running.swap(true, Ordering::Relaxed) {
            warn!("l4 flow aggr id: {} already started, do nothing.", self.id);
            return;
        }

        let mut flow_aggr = FlowAggr::new(
            self.input.clone(),
            self.output.clone(),
            self.running.clone(),
            self.config.clone(),
        );
        self.thread_handle = Some(thread::spawn(move || flow_aggr.run()));
        info!("l4 flow aggr id: {} started", self.id);
    }

    pub fn stop(&mut self) {
        if !self.running.swap(false, Ordering::Relaxed) {
            warn!("l4 flow aggr id: {} already stopped, do nothing.", self.id);
            return;
        }
        info!("stoping l4 flow aggr: {}", self.id);
        let _ = self.thread_handle.take().unwrap().join();
        info!("stopped l4 flow aggr: {}", self.id);
    }
}

pub struct FlowAggr {
    input: Arc<Receiver<Arc<TaggedFlow>>>,
    output: ThrottlingQueue,
    slot_start_time: Duration,
    stashs: VecDeque<HashMap<u64, TaggedFlow>>,

    last_flush_time: Duration,
    config: CollectorAccess,

    running: Arc<AtomicBool>,

    counter: FlowAggrCounter,
}

impl FlowAggr {
    pub fn new(
        input: Arc<Receiver<Arc<TaggedFlow>>>,
        output: DebugSender<SendItem>,
        running: Arc<AtomicBool>,
        config: CollectorAccess,
    ) -> Self {
        let mut stashs = VecDeque::new();
        for _ in 0..MINUTE_SLOTS {
            stashs.push_front(HashMap::new())
        }
        Self {
            input,
            output: ThrottlingQueue::new(output, config.clone()),
            stashs,
            slot_start_time: round_to_minute(
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
                    - Duration::from_secs(SECONDS_IN_MINUTE),
            ),
            last_flush_time: Duration::ZERO,
            config,
            running,
            counter: FlowAggrCounter::default(),
        }
    }

    fn merge(&mut self, f: Arc<TaggedFlow>) {
        let flow_time = f.flow.flow_stat_time;
        if flow_time < self.slot_start_time {
            debug!("flow drop before slot start time. flow stat time: {:?}, slot start time is {:?}, delay is {:?}", flow_time, self.slot_start_time, self.slot_start_time - flow_time);
            self.counter
                .drop_before_window
                .fetch_add(1, Ordering::Relaxed);
            return;
        }

        let mut slot = ((flow_time - self.slot_start_time).as_secs() / SECONDS_IN_MINUTE) as usize;
        if slot >= MINUTE_SLOTS {
            let flush_count = slot - MINUTE_SLOTS + 1;
            self.flush_slots(flush_count);
            slot = MINUTE_SLOTS - 1;
        }
        let slot_map = &mut self.stashs[slot];
        if let Some(flow) = slot_map.get_mut(&f.flow.flow_id) {
            if flow.flow.reversed != f.flow.reversed {
                flow.reverse();
                if let Some(stats) = flow.flow.flow_perf_stats.as_mut() {
                    stats.reverse();
                }
            }
            flow.sequential_merge(&f);
            if flow.flow.close_type != CloseType::ForcedReport {
                if let Some(closed_flow) = slot_map.remove(&f.flow.flow_id) {
                    self.send_flow(closed_flow);
                }
            }
        } else {
            if f.flow.close_type != CloseType::ForcedReport {
                self.send_flow(f.as_ref().clone());
            } else {
                slot_map.insert(f.flow.flow_id, f.as_ref().clone());
            }
            // 收到flow下一分钟数据，则需要发送上一分钟的该flow
            if slot > 0 {
                if let Some(pre_flow) = self.stashs[slot - 1].remove(&f.flow.flow_id) {
                    self.send_flow(pre_flow);
                }
            }
        }
    }

    fn send_flow(&mut self, mut f: TaggedFlow) {
        if !f.flow.is_new_flow {
            f.flow.start_time = round_to_minute(f.flow.flow_stat_time);
        }

        if f.flow.close_type == CloseType::ForcedReport {
            f.flow.end_time =
                round_to_minute(f.flow.flow_stat_time + Duration::from_secs(SECONDS_IN_MINUTE));
        }
        self.counter.out.fetch_add(1, Ordering::Relaxed);
        if !self.output.send(f) {
            self.counter
                .drop_in_throttle
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush_front_slot_and_rotate(&mut self) {
        let mut slot_map = self.stashs.pop_front().unwrap();
        for (_, v) in slot_map.drain() {
            self.send_flow(v);
        }
        self.stashs.push_back(slot_map);
        self.last_flush_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        self.slot_start_time += Duration::from_secs(SECONDS_IN_MINUTE);
    }

    fn flush_slots(&mut self, slot_count: usize) {
        for _ in 0..slot_count.min(MINUTE_SLOTS) {
            self.flush_front_slot_and_rotate();
        }

        // 若移动数超过slot的数量后, 只需设置slot开始时间
        if slot_count > MINUTE_SLOTS {
            self.slot_start_time +=
                Duration::from_secs(SECONDS_IN_MINUTE * (slot_count - MINUTE_SLOTS) as u64);
            info!(
                "now slot start time is {:?} have flushed minute slot count is {:?}",
                self.slot_start_time, slot_count
            );
        }
    }

    fn run(&mut self) {
        while self.running.load(Ordering::Relaxed) {
            match self.input.recv(Some(QUEUE_READ_TIMEOUT)) {
                Ok(tagged_flow) => {
                    if self.config.load().l4_log_store_tap_types[u16::from(TapType::Any) as usize]
                        || self.config.load().l4_log_store_tap_types
                            [u16::from(tagged_flow.flow.flow_key.tap_type) as usize]
                    {
                        self.merge(tagged_flow);
                    }
                }
                Err(Error::Timeout) => {
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                    if now > self.last_flush_time + FLUSH_TIMEOUT {
                        self.flush_front_slot_and_rotate();
                    }
                }
                Err(Error::Terminated(..)) => {
                    break;
                }
            }
        }
    }
}

// FIXME: counter not registered
impl RefCountable for FlowAggr {
    fn get_counters(&self) -> Vec<Counter> {
        vec![
            (
                "drop-before-window",
                CounterType::Counted,
                CounterValue::Unsigned(self.counter.drop_before_window.swap(0, Ordering::Relaxed)),
            ),
            (
                "out",
                CounterType::Counted,
                CounterValue::Unsigned(self.counter.out.swap(0, Ordering::Relaxed)),
            ),
            (
                "drop-in-throttle",
                CounterType::Counted,
                CounterValue::Unsigned(self.counter.drop_in_throttle.swap(0, Ordering::Relaxed)),
            ),
        ]
    }

    // fn closed(&self) -> bool {
    //     !self.running.load(Ordering::Relaxed)
    // }
}

struct ThrottlingQueue {
    config: CollectorAccess,
    throttle: u64,

    small_rng: SmallRng,

    last_flush_time: Duration,
    period_count: usize,
    output: DebugSender<SendItem>,

    stashs: Vec<SendItem>,
}

impl ThrottlingQueue {
    const THROTTLE_BUCKET_BITS: u64 = 2;
    const THROTTLE_BUCKET: u64 = 1 << Self::THROTTLE_BUCKET_BITS; // 2^N。由于发送方是有突发的，需要累积一定时间做采样
    const MIN_L4_LOG_COLLECT_NPS_THRESHOLD: u64 = 100;
    const MAX_L4_LOG_COLLECT_NPS_THRESHOLD: u64 = 1000000;

    pub fn new(output: DebugSender<SendItem>, config: CollectorAccess) -> Self {
        let t: u64 = config.load().l4_log_collect_nps_threshold * Self::THROTTLE_BUCKET;
        Self {
            config,
            throttle: t,

            small_rng: SmallRng::from_entropy(),

            last_flush_time: Duration::ZERO,
            period_count: 0,

            output,
            stashs: Vec::with_capacity(t as usize),
        }
    }

    fn flush(&mut self) {
        if let Err(_) = self.output.send_all(self.stashs.drain(..).collect()) {
            debug! {"l4 flow throttle push aggred flow to sender queue failed, maybe queue have terminated"};
        }
    }

    pub fn send(&mut self, f: TaggedFlow) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        if now.as_secs() >> Self::THROTTLE_BUCKET_BITS
            != self.last_flush_time.as_secs() >> Self::THROTTLE_BUCKET_BITS
        {
            self.update_throttle();
            self.flush();
            self.last_flush_time = now;
            self.period_count = 0;
        }

        self.period_count += 1;
        if self.stashs.len() < self.throttle as usize {
            self.stashs.push(SendItem::L4FlowLog(Box::new(f)));
            true
        } else {
            let r = self.small_rng.gen_range(0..self.period_count);
            if r < self.throttle as usize {
                self.stashs[r] = SendItem::L4FlowLog(Box::new(f));
            }
            false
        }
    }

    pub fn update_throttle(&mut self) {
        let new = self.config.load().l4_log_collect_nps_threshold;
        if new < Self::MIN_L4_LOG_COLLECT_NPS_THRESHOLD
            || new > Self::MAX_L4_LOG_COLLECT_NPS_THRESHOLD
        {
            debug!(
                "l4 flow throttle {} is invalid, must in range[{}, {}]",
                new,
                Self::MIN_L4_LOG_COLLECT_NPS_THRESHOLD,
                Self::MAX_L4_LOG_COLLECT_NPS_THRESHOLD
            );
            return;
        }
        if self.throttle == new * Self::THROTTLE_BUCKET {
            return;
        }

        info!(
            "l4_log_collect_nps_threshold update from {} to  {}",
            self.throttle / Self::THROTTLE_BUCKET,
            new
        );
        self.throttle = new * Self::THROTTLE_BUCKET;
    }
}
