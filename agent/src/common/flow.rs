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

use std::{
    fmt,
    mem::swap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    process,
    time::Duration,
};

use log::{error, warn};
use serde::Serialize;

use super::{
    decapsulate::TunnelType,
    enums::{EthernetType, IpProtocol, TapType, TcpFlags},
    tap_port::TapPort,
};

use crate::proto::flow_log;
use crate::utils::net::MacAddr;
use crate::{
    common::endpoint::EPC_FROM_INTERNET, metric::document::Direction, proto::common::TridentType,
};
use crate::{flow_generator::FlowState, metric::document::TapSide};

const COUNTER_FLOW_ID_MASK: u64 = 0x00FFFFFF;

#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum CloseType {
    Unknown = 0,
    TcpFin = 1,                 //  1: 正常结束
    TcpServerRst = 2,           //  2: 传输-服务端重置
    Timeout = 3,                //  3: 连接超时
    ForcedReport = 5,           //  5: 周期性上报
    ClientSynRepeat = 7,        //  7: 建连-客户端SYN结束
    ServerHalfClose = 8,        //  8: 断连-服务端半关
    TcpClientRst = 9,           //  9: 传输-客户端重置
    ServerSynAckRepeat = 10,    // 10: 建连-服务端SYN结束
    ClientHalfClose = 11,       // 11: 断连-客户端半关
    ClientSourcePortReuse = 13, // 13: 建连-客户端端口复用
    ServerReset = 15,           // 15: 建连-服务端直接重置
    ServerQueueLack = 17,       // 17: 传输-服务端队列溢出
    ClientEstablishReset = 18,  // 18: 建连-客户端其他重置
    ServerEstablishReset = 19,  // 19: 建连-服务端其他重置
    Max = 20,
}

impl CloseType {
    pub fn is_client_error(self) -> bool {
        self == CloseType::ClientSynRepeat
            || self == CloseType::TcpClientRst
            || self == CloseType::ClientHalfClose
            || self == CloseType::ClientSourcePortReuse
            || self == CloseType::ClientEstablishReset
    }

    pub fn is_server_error(self) -> bool {
        self == CloseType::TcpServerRst
            || self == CloseType::Timeout
            || self == CloseType::ServerHalfClose
            || self == CloseType::ServerSynAckRepeat
            || self == CloseType::ServerReset
            || self == CloseType::ServerQueueLack
            || self == CloseType::ServerEstablishReset
    }
}

impl Default for CloseType {
    fn default() -> Self {
        CloseType::Unknown
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
pub struct FlowKey {
    pub vtap_id: u16,
    pub tap_type: TapType,
    pub tap_port: TapPort,
    /* L2 */
    pub mac_src: MacAddr,
    pub mac_dst: MacAddr,
    /* L3 ipv4 or ipv6 */
    pub ip_src: IpAddr,
    pub ip_dst: IpAddr,
    /* L4 */
    pub port_src: u16,
    pub port_dst: u16,
    pub proto: IpProtocol,
}

fn append_key(dst: &mut String, key: &str) {
    dst.push_str(",\"");
    dst.push_str(key);
    dst.push_str("\":");
}

fn append_keys(dst: &mut String, key1: &str, key2: &str) {
    dst.push_str(",\"");
    dst.push_str(key1);
    dst.push_str(key2);
    dst.push_str("\":");
}

fn append_key_value(dst: &mut String, key: &str, value: &str) {
    append_key(dst, key);
    dst.push_str(value);
}

fn append_keys_value(dst: &mut String, key1: &str, key2: &str, value: &str) {
    append_keys(dst, key1, key2);
    dst.push_str(value);
}

fn append_key_string(dst: &mut String, key: &str, value: &str) {
    append_key(dst, key);
    dst.push('\"');
    dst.push_str(value);
    dst.push('\"');
}

fn append_key_bool(dst: &mut String, key: &str, value: bool) {
    append_key(dst, key);
    if value {
        dst.push_str("true");
    } else {
        dst.push_str("false");
    }
}

fn append_keys_bool(dst: &mut String, key1: &str, key2: &str, value: bool) {
    append_keys(dst, key1, key2);
    if value {
        dst.push_str("true");
    } else {
        dst.push_str("false");
    }
}

impl FlowKey {
    pub fn reverse(&mut self) {
        swap(&mut self.mac_src, &mut self.mac_dst);
        swap(&mut self.ip_src, &mut self.ip_dst);
        swap(&mut self.port_src, &mut self.port_dst);
    }
    pub fn to_kv_string(&self, dst: &mut String) {
        append_key_value(dst, "vtap_id", &self.vtap_id.to_string());
        append_key_string(dst, "tap_type", &self.tap_type.to_string());
        append_key_string(dst, "tap_port", &self.tap_port.to_string());
        append_key_string(dst, "mac_src", &self.mac_src.to_string());
        append_key_string(dst, "mac_dst", &self.mac_dst.to_string());
        append_key_string(dst, "ip_src", &self.ip_src.to_string());
        append_key_string(dst, "ip_dst", &self.ip_dst.to_string());
        append_key_value(dst, "port_src", &self.port_src.to_string());
        append_key_value(dst, "port_dst", &self.port_dst.to_string());
        append_key_string(dst, "protocol", &format!("{:?}", self.proto));
    }
}

impl Default for FlowKey {
    fn default() -> Self {
        FlowKey {
            ip_src: Ipv4Addr::UNSPECIFIED.into(),
            ip_dst: Ipv4Addr::UNSPECIFIED.into(),
            vtap_id: 0,
            tap_type: TapType::default(),
            tap_port: TapPort::default(),
            mac_src: MacAddr::default(),
            mac_dst: MacAddr::default(),
            port_src: 0,
            port_dst: 0,
            proto: IpProtocol::default(),
        }
    }
}

impl fmt::Display for FlowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "vtap_id:{} tap_type:{} tap_port:{} mac_src:{} mac_dst:{} ip_src:{} ip_dst:{} proto:{:?} port_src:{} port_dst:{}",
            self.vtap_id,
            self.tap_type,
            self.tap_port,
            self.mac_src,
            self.mac_dst,
            self.ip_src,
            self.ip_dst,
            self.proto,
            self.port_src,
            self.port_dst
        )
    }
}

impl From<FlowKey> for flow_log::FlowKey {
    fn from(f: FlowKey) -> Self {
        let (ip4_src, ip4_dst, ip6_src, ip6_dst) = match (f.ip_src, f.ip_dst) {
            (IpAddr::V4(ip4), IpAddr::V4(ip4_1)) => {
                (ip4, ip4_1, Ipv6Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED)
            }
            (IpAddr::V6(ip6), IpAddr::V6(ip6_1)) => {
                (Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED, ip6, ip6_1)
            }
            _ => panic!("ip_src,ip_dst type mismatch"),
        };
        flow_log::FlowKey {
            vtap_id: f.vtap_id as u32,
            tap_type: u16::from(f.tap_type) as u32,
            tap_port: f.tap_port.0,
            mac_src: f.mac_src.into(),
            mac_dst: f.mac_dst.into(),
            ip_src: u32::from_be_bytes(ip4_src.octets()),
            ip_dst: u32::from_be_bytes(ip4_dst.octets()),
            ip6_src: ip6_src.octets().to_vec(),
            ip6_dst: ip6_dst.octets().to_vec(),
            port_src: f.port_src as u32,
            port_dst: f.port_dst as u32,
            proto: f.proto as u32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FlowSource {
    Normal = 0,
    Sflow = 1,
    NetFlow = 2,
}

impl Default for FlowSource {
    fn default() -> Self {
        FlowSource::Normal
    }
}

#[derive(Debug, Clone)]
pub struct TunnelField {
    pub tx_ip0: Ipv4Addr, // 对应发送方向的源隧道IP
    pub tx_ip1: Ipv4Addr, // 对应发送方向的目的隧道IP
    pub rx_ip0: Ipv4Addr, // 对应接收方向的源隧道IP
    pub rx_ip1: Ipv4Addr, // 对应接收方向的目的隧道IP
    pub tx_mac0: u32,     // 对应发送方向的源隧道MAC，低4字节
    pub tx_mac1: u32,     // 对应发送方向的目的隧道MAC，低4字节
    pub rx_mac0: u32,     // 对应接收方向的源隧道MAC，低4字节
    pub rx_mac1: u32,     // 对应接收方向的目的隧道MAC，低4字节
    pub tx_id: u32,
    pub rx_id: u32,
    pub tunnel_type: TunnelType,
    pub tier: u8,
    pub is_ipv6: bool,
}

impl Default for TunnelField {
    fn default() -> Self {
        TunnelField {
            tx_ip0: Ipv4Addr::UNSPECIFIED,
            tx_ip1: Ipv4Addr::UNSPECIFIED,
            rx_ip0: Ipv4Addr::UNSPECIFIED,
            rx_ip1: Ipv4Addr::UNSPECIFIED,
            tx_mac0: 0,
            tx_mac1: 0,
            rx_mac0: 0,
            rx_mac1: 0,
            tx_id: 0,
            rx_id: 0,
            tunnel_type: TunnelType::default(),
            tier: 0,
            is_ipv6: false,
        }
    }
}

impl TunnelField {
    pub fn reverse(&mut self) {
        swap(&mut self.tx_ip0, &mut self.rx_ip0);
        swap(&mut self.tx_ip1, &mut self.rx_ip1);
        swap(&mut self.tx_mac0, &mut self.rx_mac0);
        swap(&mut self.tx_mac1, &mut self.rx_mac1);
        swap(&mut self.tx_id, &mut self.rx_id);
    }

    pub fn to_kv_string(&self, dst: &mut String) {
        append_key_string(dst, "tunnel_type", &self.tunnel_type.to_string());
        append_key_string(dst, "tunnel_tx_ip_0", &self.tx_ip0.to_string());
        append_key_string(dst, "tunnel_tx_ip_1", &self.tx_ip1.to_string());
        append_key_string(dst, "tunnel_rx_ip_0", &self.rx_ip0.to_string());
        append_key_string(dst, "tunnel_rx_ip_1", &self.rx_ip1.to_string());
        append_key_string(dst, "tunnel_tx_mac_0", &format!("{:08x}", self.tx_mac0));
        append_key_string(dst, "tunnel_tx_mac_1", &format!("{:08x}", self.tx_mac1));
        append_key_string(dst, "tunnel_rx_mac_0", &format!("{:08x}", self.tx_mac0));
        append_key_string(dst, "tunnel_rx_mac_1", &format!("{:08x}", self.tx_mac1));
        append_key_value(dst, "tunnel_tx_id", &self.tx_id.to_string());
        append_key_value(dst, "tunnel_rx_id", &self.rx_id.to_string());
        append_key_value(dst, "tunnel_tier", &self.tier.to_string());
    }
}

impl fmt::Display for TunnelField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tunnel_type == TunnelType::None {
            write!(f, "none")
        } else {
            write!(
            f,
            "{}, tx_id:{}, rx_id:{}, tier:{}, tx_0:{} {:08x}, tx_1:{} {:08x}, rx_0:{} {:08x}, rx_1:{} {:08x}",
            self.tunnel_type, self.tx_id, self.rx_id, self.tier,
            self.tx_ip0, self.tx_mac0,
            self.tx_ip1, self.tx_mac1,
            self.rx_ip0, self.rx_mac0,
            self.rx_ip1, self.rx_mac1,
            )
        }
    }
}

impl From<TunnelField> for flow_log::TunnelField {
    fn from(f: TunnelField) -> Self {
        flow_log::TunnelField {
            tx_ip0: u32::from_be_bytes(f.tx_ip0.octets()),
            tx_ip1: u32::from_be_bytes(f.tx_ip1.octets()),
            rx_ip0: u32::from_be_bytes(f.rx_ip0.octets()),
            rx_ip1: u32::from_be_bytes(f.rx_ip1.octets()),
            tx_mac0: f.tx_mac0.into(),
            tx_mac1: f.tx_mac1.into(),
            rx_mac0: f.rx_mac0.into(),
            rx_mac1: f.rx_mac1.into(),
            tx_id: f.tx_id,
            rx_id: f.rx_id,
            tunnel_type: f.tunnel_type as u32,
            tier: f.tier as u32,
            is_ipv6: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TcpPerfCountsPeer {
    pub retrans_count: u32,
    pub zero_win_count: u32,
}

impl TcpPerfCountsPeer {
    pub fn sequential_merge(&mut self, other: &TcpPerfCountsPeer) {
        self.retrans_count += other.retrans_count;
        self.zero_win_count += other.zero_win_count;
    }
}

impl From<TcpPerfCountsPeer> for flow_log::TcpPerfCountsPeer {
    fn from(p: TcpPerfCountsPeer) -> Self {
        flow_log::TcpPerfCountsPeer {
            retrans_count: p.retrans_count,
            zero_win_count: p.zero_win_count,
        }
    }
}

#[derive(Debug, Default, Clone)]
// UDPPerfStats仅有2个字段，复用art_max, art_sum, art_count
pub struct TcpPerfStats {
    // 除特殊说明外，均为每个流统计周期（目前是自然分）清零
    pub rtt_client_max: u32, // us, agent保证时延最大值不会超过3600s，能容纳在u32内
    pub rtt_server_max: u32, // us
    pub srt_max: u32,        // us
    pub art_max: u32,        // us, UDP复用
    pub cit_max: u32, // us, the max time between the client request and the last server response (Payload > 1)

    pub rtt: u32,            // us, TCP建连过程, 只会计算出一个RTT
    pub rtt_client_sum: u32, // us, 假定一条流在一分钟内的时延加和不会超过u32
    pub rtt_server_sum: u32, // us
    pub srt_sum: u32,        // us
    pub art_sum: u32,        // us
    pub cit_sum: u32,        // us

    pub rtt_client_count: u32,
    pub rtt_server_count: u32,
    pub srt_count: u32,
    pub art_count: u32, // UDP复用
    pub cit_count: u32,

    pub syn_count: u32,
    pub synack_count: u32,

    pub retrans_syn_count: u32,
    pub retrans_synack_count: u32,

    pub counts_peers: [TcpPerfCountsPeer; 2],
    pub total_retrans_count: u32,
}

impl TcpPerfStats {
    pub fn to_kv_string(&self, dst: &mut String) {
        append_key_value(dst, "rtt", &self.rtt.to_string());

        append_key_value(dst, "rtt_client_max", &self.rtt_client_max.to_string());
        append_key_value(dst, "rtt_server_max", &self.rtt_server_max.to_string());
        append_key_value(dst, "srt_max", &self.srt_max.to_string());
        append_key_value(dst, "art_max", &self.art_max.to_string());
        append_key_value(dst, "cit_max", &self.cit_max.to_string());

        append_key_value(dst, "rtt_client_sum", &self.rtt_client_sum.to_string());
        append_key_value(dst, "rtt_server_sum", &self.rtt_server_sum.to_string());
        append_key_value(dst, "srt_sum", &self.srt_sum.to_string());
        append_key_value(dst, "art_sum", &self.art_sum.to_string());
        append_key_value(dst, "cit_sum", &self.cit_sum.to_string());

        append_key_value(dst, "rtt_client_count", &self.rtt_client_count.to_string());
        append_key_value(dst, "rtt_server_count", &self.rtt_server_count.to_string());
        append_key_value(dst, "srt_count", &self.srt_count.to_string());
        append_key_value(dst, "art_count", &self.art_count.to_string());
        append_key_value(dst, "cit_count", &self.cit_sum.to_string());
        append_key_value(dst, "syn_count", &self.syn_count.to_string());
        append_key_value(dst, "synack_count", &self.synack_count.to_string());
        append_key_value(
            dst,
            "retrans_syn_count",
            &self.retrans_syn_count.to_string(),
        );
        append_key_value(
            dst,
            "retrans_synack_count",
            &self.retrans_synack_count.to_string(),
        );

        append_key_value(
            dst,
            "retrans_tx",
            &self.counts_peers[0].retrans_count.to_string(),
        );
        append_key_value(
            dst,
            "retrans_rx",
            &self.counts_peers[1].retrans_count.to_string(),
        );
        append_key_value(
            dst,
            "zero_win_tx",
            &self.counts_peers[0].zero_win_count.to_string(),
        );
        append_key_value(
            dst,
            "zero_win_rx",
            &self.counts_peers[1].zero_win_count.to_string(),
        );
    }

    pub fn sequential_merge(&mut self, other: &TcpPerfStats) {
        if self.rtt_client_max < other.rtt_client_max {
            self.rtt_client_max = other.rtt_client_max;
        }
        if self.rtt_server_max < other.rtt_server_max {
            self.rtt_server_max = other.rtt_server_max;
        }
        if self.srt_max < other.srt_max {
            self.srt_max = other.srt_max;
        }
        if self.art_max < other.art_max {
            self.art_max = other.art_max;
        }
        if self.rtt < other.rtt {
            self.rtt = other.rtt;
        }
        if self.cit_max < other.cit_max {
            self.cit_max = other.cit_max;
        }

        self.rtt_client_sum += other.rtt_client_sum;
        self.rtt_server_sum += other.rtt_server_sum;
        self.srt_sum += other.srt_sum;
        self.art_sum += other.art_sum;
        self.cit_sum += other.cit_sum;

        self.rtt_client_count += other.rtt_client_count;
        self.rtt_server_count += other.rtt_server_count;
        self.srt_count += other.srt_count;
        self.art_count += other.art_count;
        self.syn_count += other.syn_count;
        self.cit_count += other.cit_count;
        self.synack_count += other.synack_count;
        self.retrans_syn_count += other.retrans_syn_count;
        self.retrans_synack_count += other.retrans_synack_count;
        self.counts_peers[0].sequential_merge(&other.counts_peers[0]);
        self.counts_peers[1].sequential_merge(&other.counts_peers[1]);
        self.total_retrans_count += other.total_retrans_count;
    }

    pub fn reverse(&mut self) {
        swap(&mut self.rtt_client_sum, &mut self.rtt_server_sum);
        swap(&mut self.rtt_client_count, &mut self.rtt_server_count);
        self.counts_peers.swap(0, 1);
    }
}

impl From<TcpPerfStats> for flow_log::TcpPerfStats {
    fn from(p: TcpPerfStats) -> Self {
        flow_log::TcpPerfStats {
            rtt_client_max: p.rtt_client_max,
            rtt_server_max: p.rtt_server_max,
            srt_max: p.srt_max,
            art_max: p.art_max,
            rtt: p.rtt,
            rtt_client_sum: p.rtt_client_sum,
            rtt_server_sum: p.rtt_server_sum,
            srt_sum: p.srt_sum,
            art_sum: p.art_sum,
            rtt_client_count: p.rtt_client_count,
            rtt_server_count: p.rtt_server_count,
            srt_count: p.srt_count,
            art_count: p.art_count,
            counts_peer_tx: Some(p.counts_peers[0].into()),
            counts_peer_rx: Some(p.counts_peers[1].into()),
            total_retrans_count: p.total_retrans_count,
            cit_count: p.cit_count,
            cit_sum: p.cit_sum,
            cit_max: p.cit_max,
            syn_count: p.syn_count,
            synack_count: p.synack_count,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct FlowPerfStats {
    pub tcp: TcpPerfStats,
    pub l7: L7PerfStats,
    pub l4_protocol: L4Protocol,
    pub l7_protocol: L7Protocol,
}

impl FlowPerfStats {
    pub fn to_kv_string(&self, dst: &mut String) {
        self.tcp.to_kv_string(dst);
        self.l7.to_kv_string(dst);
        append_key_string(dst, "l4_protocol", &format!("{:?}", self.l4_protocol));
        append_key_string(dst, "l7_protocol", &format!("{:?}", self.l7_protocol));
    }

    pub fn sequential_merge(&mut self, other: &FlowPerfStats) {
        if self.l4_protocol == L4Protocol::Unknown {
            self.l4_protocol = other.l4_protocol;
        }

        if self.l7_protocol == L7Protocol::Unknown
            || (self.l7_protocol == L7Protocol::Other && other.l7_protocol != L7Protocol::Unknown)
        {
            self.l7_protocol = other.l7_protocol;
        }
        self.tcp.sequential_merge(&other.tcp);
        self.l7.sequential_merge(&other.l7);
    }

    pub fn reverse(&mut self) {
        self.tcp.reverse()
    }
}

impl fmt::Display for FlowPerfStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "l4_protocol:{:?} tcp_perf_stats:{:?} \n\t l7_protocol:{:?} l7_perf_stats:{:?}",
            self.l4_protocol, self.tcp, self.l7_protocol, self.l7
        )
    }
}

impl From<FlowPerfStats> for flow_log::FlowPerfStats {
    fn from(p: FlowPerfStats) -> Self {
        flow_log::FlowPerfStats {
            tcp: Some(p.tcp.into()),
            l7: Some(p.l7.into()),
            l4_protocol: p.l4_protocol as u32,
            l7_protocol: p.l7_protocol as u32,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct L7PerfStats {
    pub request_count: u32,
    pub response_count: u32,
    pub err_client_count: u32, // client端原因导致的响应异常数量
    pub err_server_count: u32, // server端原因导致的响应异常数量
    pub err_timeout: u32,      // request请求timeout数量
    pub rrt_count: u32,        // u32可记录40000M时延, 一条流在一分钟内的请求数远无法达到此数值
    pub rrt_sum: u64,          // us RRT(Request Response Time)
    pub rrt_max: u32,          // us agent保证在3600s以内
}

impl L7PerfStats {
    pub fn to_kv_string(&self, dst: &mut String) {
        append_key_value(dst, "l7_request", &self.request_count.to_string());
        append_key_value(dst, "l7_response", &self.response_count.to_string());
        append_key_value(dst, "l7_client_err", &self.err_client_count.to_string());
        append_key_value(dst, "l7_server_err", &self.err_server_count.to_string());
        append_key_value(dst, "l7_server_timeout", &self.err_timeout.to_string());
        append_key_value(dst, "rrt_count", &self.rrt_count.to_string());
        append_key_value(dst, "rrt_sum", &self.rrt_sum.to_string());
        append_key_value(dst, "rrt_max", &self.rrt_max.to_string());
    }

    pub fn sequential_merge(&mut self, other: &L7PerfStats) {
        self.request_count += other.request_count;
        self.response_count += other.response_count;
        self.err_client_count += other.err_client_count;
        self.err_server_count += other.err_server_count;
        self.err_timeout += other.err_timeout;
        self.rrt_count += other.rrt_count;
        self.rrt_sum += other.rrt_sum;
        if self.rrt_max < other.rrt_max {
            self.rrt_max = other.rrt_max
        }
    }
}

impl From<L7PerfStats> for flow_log::L7PerfStats {
    fn from(p: L7PerfStats) -> Self {
        flow_log::L7PerfStats {
            request_count: p.request_count,
            response_count: p.response_count,
            err_client_count: p.err_client_count,
            err_server_count: p.err_server_count,
            err_timeout: p.err_timeout,
            rrt_count: p.rrt_count,
            rrt_sum: p.rrt_sum,
            rrt_max: p.rrt_max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum L4Protocol {
    Unknown = 0,
    Tcp = 1,
    Udp = 2,
}

impl From<IpProtocol> for L4Protocol {
    fn from(proto: IpProtocol) -> Self {
        match proto {
            IpProtocol::Tcp => Self::Tcp,
            IpProtocol::Udp => Self::Udp,
            _ => Self::Unknown,
        }
    }
}

impl Default for L4Protocol {
    fn default() -> Self {
        L4Protocol::Unknown
    }
}

const L7_PROTOCOL_UNKNOWN: u8 = 0;
const L7_PROTOCOL_OTHER: u8 = 1;
const L7_PROTOCOL_HTTP1: u8 = 20;
const L7_PROTOCOL_HTTP2: u8 = 21;
const L7_PROTOCOL_HTTP1_TLS: u8 = 22;
const L7_PROTOCOL_HTTP2_TLS: u8 = 23;
const L7_PROTOCOL_DUBBO: u8 = 40;
const L7_PROTOCOL_MYSQL: u8 = 60;
const L7_PROTOCOL_REDIS: u8 = 80;
const L7_PROTOCOL_KAFKA: u8 = 100;
const L7_PROTOCOL_MQTT: u8 = 101;
const L7_PROTOCOL_DNS: u8 = 120;
const L7_PROTOCOL_MAX: u8 = 255;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Hash, Eq)]
#[repr(u8)]
pub enum L7Protocol {
    Unknown = L7_PROTOCOL_UNKNOWN,
    Other = L7_PROTOCOL_OTHER,
    Http1 = L7_PROTOCOL_HTTP1,
    Http2 = L7_PROTOCOL_HTTP2,
    Http1TLS = L7_PROTOCOL_HTTP1_TLS,
    Http2TLS = L7_PROTOCOL_HTTP2_TLS,
    Dubbo = L7_PROTOCOL_DUBBO,
    Mysql = L7_PROTOCOL_MYSQL,
    Redis = L7_PROTOCOL_REDIS,
    Kafka = L7_PROTOCOL_KAFKA,
    Mqtt = L7_PROTOCOL_MQTT,
    Dns = L7_PROTOCOL_DNS,
    Max = L7_PROTOCOL_MAX,
}

impl Default for L7Protocol {
    fn default() -> Self {
        L7Protocol::Unknown
    }
}

impl From<u8> for L7Protocol {
    fn from(v: u8) -> Self {
        match v {
            L7_PROTOCOL_OTHER => L7Protocol::Other,
            L7_PROTOCOL_HTTP1 => L7Protocol::Http1,
            L7_PROTOCOL_HTTP2 => L7Protocol::Http2,
            L7_PROTOCOL_HTTP1_TLS => L7Protocol::Http1TLS,
            L7_PROTOCOL_HTTP2_TLS => L7Protocol::Http2TLS,
            L7_PROTOCOL_DUBBO => L7Protocol::Dubbo,
            L7_PROTOCOL_MYSQL => L7Protocol::Mysql,
            L7_PROTOCOL_REDIS => L7Protocol::Redis,
            L7_PROTOCOL_KAFKA => L7Protocol::Kafka,
            L7_PROTOCOL_MQTT => L7Protocol::Mqtt,
            L7_PROTOCOL_DNS => L7Protocol::Dns,
            _ => L7Protocol::Unknown,
        }
    }
}

impl From<L7Protocol> for u8 {
    fn from(v: L7Protocol) -> u8 {
        match v {
            L7Protocol::Other => L7_PROTOCOL_OTHER,
            L7Protocol::Http1 => L7_PROTOCOL_HTTP1,
            L7Protocol::Http2 => L7_PROTOCOL_HTTP2,
            L7Protocol::Http1TLS => L7_PROTOCOL_HTTP1_TLS,
            L7Protocol::Http2TLS => L7_PROTOCOL_HTTP2_TLS,
            L7Protocol::Dubbo => L7_PROTOCOL_DUBBO,
            L7Protocol::Mysql => L7_PROTOCOL_MYSQL,
            L7Protocol::Redis => L7_PROTOCOL_REDIS,
            L7Protocol::Kafka => L7_PROTOCOL_KAFKA,
            L7Protocol::Mqtt => L7_PROTOCOL_MQTT,
            L7Protocol::Dns => L7_PROTOCOL_DNS,
            _ => L7_PROTOCOL_UNKNOWN,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlowMetricsPeer {
    pub nat_real_ip: IpAddr, // IsVIP为true，通过MAC查询对应的IP

    pub byte_count: u64,         // 每个流统计周期（目前是自然秒）清零
    pub l3_byte_count: u64,      // 每个流统计周期的L3载荷量
    pub l4_byte_count: u64,      // 每个流统计周期的L4载荷量
    pub packet_count: u64,       // 每个流统计周期（目前是自然秒）清零
    pub total_byte_count: u64,   // 整个Flow生命周期的统计量
    pub total_packet_count: u64, // 整个Flow生命周期的统计量
    pub first: Duration,         // 整个Flow生命周期首包的时间戳
    pub last: Duration,          // 整个Flow生命周期尾包的时间戳

    pub l3_epc_id: i32,
    pub is_l2_end: bool,
    pub is_l3_end: bool,
    pub is_active_host: bool,
    pub is_device: bool,        // ture表明是从平台数据获取的
    pub tcp_flags: TcpFlags,    // 所有TCP的Flags的或运算结果
    pub is_vip_interface: bool, // 目前仅支持微软Mux设备，从grpc Interface中获取
    pub is_vip: bool,           // 从grpc cidr中获取
    pub is_local_mac: bool,     // 同EndpointInfo中的IsLocalMac, 流日志中不需要存储
    pub is_local_ip: bool,      // 同EndpointInfo中的IsLocalIp, 流日志中不需要存储
}

impl Default for FlowMetricsPeer {
    fn default() -> Self {
        FlowMetricsPeer {
            nat_real_ip: Ipv4Addr::UNSPECIFIED.into(),
            byte_count: 0,
            l3_byte_count: 0,
            l4_byte_count: 0,
            packet_count: 0,
            total_byte_count: 0,
            total_packet_count: 0,
            first: Duration::default(),
            last: Duration::default(),

            l3_epc_id: 0,
            is_l2_end: false,
            is_l3_end: false,
            is_active_host: false,
            is_device: false,
            tcp_flags: TcpFlags::empty(),
            is_vip_interface: false,
            is_vip: false,
            is_local_mac: false,
            is_local_ip: false,
        }
    }
}

impl FlowMetricsPeer {
    pub const SRC: u8 = 0;
    pub const DST: u8 = 1;

    pub fn to_kv_string(&self, dst: &mut String, direction: u8) {
        let mut subfix = ["_tx", "_0"];
        if direction == Self::DST {
            subfix = ["_rx", "_1"];
        }

        append_keys_value(dst, "byte", subfix[0], &self.byte_count.to_string());
        append_keys_value(dst, "l3_byte", subfix[0], &self.l3_byte_count.to_string());
        append_keys_value(dst, "l4_byte", subfix[0], &self.l4_byte_count.to_string());
        append_keys_value(dst, "packet", subfix[0], &self.packet_count.to_string());
        append_keys_value(
            dst,
            "total_byte",
            subfix[0],
            &self.total_byte_count.to_string(),
        );
        append_keys_value(
            dst,
            "total_packet",
            subfix[1],
            &self.total_packet_count.to_string(),
        );

        append_keys_value(dst, "l3_epc_id", subfix[1], &self.l3_epc_id.to_string());
        append_keys_bool(dst, "l2_end", subfix[1], self.is_l2_end);
        append_keys_bool(dst, "l3_end", subfix[1], self.is_l3_end);
        append_key_string(dst, "tcp_flags", &self.tcp_flags.to_string());
    }

    pub fn sequential_merge(&mut self, other: &FlowMetricsPeer) {
        self.byte_count += other.byte_count;
        self.l3_byte_count += other.l3_byte_count;
        self.l4_byte_count += other.l4_byte_count;
        self.packet_count += other.packet_count;
        self.total_byte_count += other.total_byte_count;
        self.total_packet_count += other.total_packet_count;
        self.first = other.first;
        self.last = other.last;

        self.l3_epc_id = other.l3_epc_id;
        self.is_l2_end = other.is_l2_end;
        self.is_l3_end = other.is_l3_end;
        self.is_active_host = other.is_active_host;
        self.is_device = other.is_device;
        self.tcp_flags |= other.tcp_flags;
        self.is_vip_interface = other.is_vip_interface;
        self.is_vip = other.is_vip;
        self.is_local_mac = other.is_local_mac;
        self.is_local_ip = other.is_local_ip;
    }
}

impl From<FlowMetricsPeer> for flow_log::FlowMetricsPeer {
    fn from(m: FlowMetricsPeer) -> Self {
        flow_log::FlowMetricsPeer {
            byte_count: m.byte_count,
            l3_byte_count: m.l3_byte_count,
            l4_byte_count: m.l4_byte_count,
            packet_count: m.packet_count,
            total_byte_count: m.total_byte_count,
            total_packet_count: m.total_packet_count,
            first: m.first.as_nanos() as u64,
            last: m.last.as_nanos() as u64,

            l3_epc_id: m.l3_epc_id,
            is_l2_end: m.is_l2_end as u32,
            is_l3_end: m.is_l3_end as u32,
            is_active_host: m.is_active_host as u32,
            is_device: m.is_device as u32,
            tcp_flags: m.tcp_flags.bits() as u32,
            is_vip_interface: m.is_vip_interface as u32,
            is_vip: m.is_vip as u32,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct Flow {
    pub flow_key: FlowKey,
    pub flow_metrics_peers: [FlowMetricsPeer; 2],

    pub tunnel: TunnelField,

    pub flow_id: u64,

    /* TCP Seq */
    pub syn_seq: u32,
    pub synack_seq: u32,
    pub last_keepalive_seq: u32,
    pub last_keepalive_ack: u32,

    pub start_time: Duration,
    pub end_time: Duration,
    pub duration: Duration,
    pub flow_stat_time: Duration,

    /* L2 */
    pub vlan: u16,
    pub eth_type: EthernetType,

    /* TCP Perf Data*/
    pub flow_perf_stats: Option<FlowPerfStats>,

    pub close_type: CloseType,
    pub flow_source: FlowSource,
    pub is_active_service: bool,
    pub queue_hash: u8,
    pub is_new_flow: bool,
    pub reversed: bool,
    pub tap_side: TapSide,
}

impl Flow {
    pub fn to_kv_string(&self, dst: &mut String) {
        self.flow_key.to_kv_string(dst);
        self.flow_metrics_peers[0].to_kv_string(dst, 0);
        self.flow_metrics_peers[1].to_kv_string(dst, 1);

        if self.tunnel.tunnel_type != TunnelType::None {
            self.tunnel.to_kv_string(dst);
        }

        append_key_value(dst, "flow_id", &self.flow_id.to_string());
        append_key_value(dst, "syn_seq", &self.syn_seq.to_string());
        append_key_value(dst, "syn_ack_seq", &self.synack_seq.to_string());
        append_key_value(
            dst,
            "last_keepalive_seq",
            &self.last_keepalive_seq.to_string(),
        );
        append_key_value(
            dst,
            "last_keepalive_ack",
            &self.last_keepalive_ack.to_string(),
        );
        append_key_value(dst, "start_time", &self.start_time.as_micros().to_string());
        append_key_value(dst, "end_time", &self.end_time.as_micros().to_string());
        append_key_value(dst, "duration", &self.duration.as_micros().to_string());
        append_key_value(dst, "vlan", &self.vlan.to_string());
        append_key_string(dst, "eth_type", &format!("{:?}", self.eth_type));
        if let Some(flow_perf_stats) = &self.flow_perf_stats {
            flow_perf_stats.to_kv_string(dst);
        }
        append_key_string(dst, "close_type", &format!("{:?}", self.close_type));
        append_key_string(dst, "flow_source", &format!("{:?}", self.flow_source));
        append_key_bool(dst, "is_new_flow", self.is_new_flow);
        append_key_string(dst, "tap_side", &format!("{:?}", self.tap_side));
    }

    pub fn sequential_merge(&mut self, other: &Flow) {
        self.flow_metrics_peers[0].sequential_merge(&other.flow_metrics_peers[0]);
        self.flow_metrics_peers[1].sequential_merge(&other.flow_metrics_peers[1]);

        self.end_time = other.end_time;
        self.duration = other.duration;

        if other.flow_perf_stats.is_some() {
            let x = other.flow_perf_stats.as_ref().unwrap();
            if self.flow_perf_stats.is_none() {
                self.flow_perf_stats = Some(x.clone());
            } else {
                self.flow_perf_stats.as_mut().unwrap().sequential_merge(&x)
            }
        }

        self.close_type = other.close_type;
        self.is_active_service = other.is_active_service;
        self.reversed = other.reversed;
        if other.vlan > 0 {
            self.vlan = other.vlan
        }

        if other.last_keepalive_seq != 0 {
            self.last_keepalive_seq = other.last_keepalive_seq;
        }
        if other.last_keepalive_ack != 0 {
            self.last_keepalive_ack = other.last_keepalive_ack;
        }
    }

    // FIXME 注意：由于FlowGenerator中TcpPerfStats在Flow方向调整之后才获取到，
    // 因此这里不包含对TcpPerfStats的反向。
    pub fn reverse(&mut self, no_stats: bool) {
        // 如果没有统计数据不需要标记reversed来反向数据
        self.reversed = !self.reversed && !no_stats;
        self.tap_side = TapSide::Rest;
        self.tunnel.reverse();
        self.flow_key.reverse();
        self.flow_metrics_peers.swap(0, 1);
    }

    pub fn update_close_type(&mut self, flow_state: FlowState) {
        self.close_type = match flow_state {
            FlowState::Exception => CloseType::Unknown,
            FlowState::Opening1 => CloseType::ClientSynRepeat,
            FlowState::Opening2 => CloseType::ServerSynAckRepeat,
            FlowState::Established => CloseType::Timeout,
            FlowState::ClosingTx1 => CloseType::ServerHalfClose,
            FlowState::ClosingRx1 => CloseType::ClientHalfClose,
            FlowState::ClosingTx2 | FlowState::ClosingRx2 | FlowState::Closed => CloseType::TcpFin,
            FlowState::Reset => {
                if self.flow_metrics_peers[FlowMetricsPeer::DST as usize]
                    .tcp_flags
                    .contains(TcpFlags::RST)
                {
                    CloseType::TcpServerRst
                } else {
                    CloseType::TcpClientRst
                }
            }
            FlowState::Syn1 | FlowState::ClientL4PortReuse => CloseType::ClientSourcePortReuse,
            FlowState::ServerReset => CloseType::ServerReset,
            FlowState::SynAck1 => CloseType::ServerQueueLack,
            FlowState::ServerCandidateQueueLack => {
                const TCP_SYN_RETRANSE_MIN_TIMES: u64 = 3;
                if self.flow_metrics_peers[FlowMetricsPeer::DST as usize].total_packet_count
                    > TCP_SYN_RETRANSE_MIN_TIMES
                {
                    CloseType::ServerQueueLack
                } else {
                    CloseType::TcpClientRst
                }
            }
            FlowState::EstablishReset => {
                if self.flow_metrics_peers[FlowMetricsPeer::DST as usize]
                    .tcp_flags
                    .contains(TcpFlags::RST)
                {
                    CloseType::ServerEstablishReset
                } else {
                    CloseType::ClientEstablishReset
                }
            }
            _ => {
                warn!(
                    "unexpected 'unknown' close type, flow id is {}",
                    self.flow_id
                );
                CloseType::Unknown
            }
        }
    }

    pub fn set_tap_side(
        &mut self,
        trident_type: TridentType,
        cloud_gateway_traffic: bool, // 从static config 获取
    ) {
        if self.tap_side != TapSide::Rest {
            return;
        }
        // 链路追踪统计位置
        let (src_tap_side, dst_tap_side, _) =
            get_direction(&*self, trident_type, cloud_gateway_traffic);

        if src_tap_side != Direction::None && dst_tap_side == Direction::None {
            self.tap_side = src_tap_side.into();
        } else if src_tap_side == Direction::None && dst_tap_side != Direction::None {
            self.tap_side = dst_tap_side.into();
        }
    }
}

impl fmt::Display for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "flow_id:{} flow_source:{:?} tunnel:{} close_type:{:?} is_active_service:{} is_new_flow:{} queue_hash:{} \
        syn_seq:{} synack_seq:{} last_keepalive_seq:{} last_keepalive_ack:{} flow_stat_time:{:?} \
        \t start_time:{:?} end_time:{:?} duration:{:?} \
        \t vlan:{} eth_type:{:?} reversed:{} flow_key:{} \
        \n\t flow_metrics_peers_src:{:?} \
        \n\t flow_metrics_peers_dst:{:?} \
        \n\t flow_perf_stats:{:?}",
            self.flow_id, self.flow_source, self.tunnel, self.close_type, self.is_active_service, self.is_new_flow, self.queue_hash,
            self.syn_seq, self.synack_seq, self.last_keepalive_seq, self.last_keepalive_ack, self.flow_stat_time,
            self.start_time, self.end_time, self.duration,
            self.vlan, self.eth_type, self.reversed, self.flow_key,
            self.flow_metrics_peers[0],
            self.flow_metrics_peers[1],
            self.flow_perf_stats
        )
    }
}

impl From<Flow> for flow_log::Flow {
    fn from(f: Flow) -> Self {
        flow_log::Flow {
            flow_key: Some(f.flow_key.into()),
            metrics_peer_src: Some(f.flow_metrics_peers[0].into()),
            metrics_peer_dst: Some(f.flow_metrics_peers[1].into()),
            tunnel: {
                if f.tunnel.tunnel_type == TunnelType::None {
                    None
                } else {
                    Some(f.tunnel.into())
                }
            },
            flow_id: f.flow_id,
            start_time: f.start_time.as_nanos() as u64,
            end_time: f.end_time.as_nanos() as u64,
            duration: f.duration.as_nanos() as u64,
            eth_type: f.eth_type as u32,
            vlan: f.vlan as u32,
            has_perf_stats: f.flow_perf_stats.is_some() as u32,
            perf_stats: {
                if f.flow_perf_stats.is_none() {
                    None
                } else {
                    Some(f.flow_perf_stats.unwrap().into())
                }
            },
            close_type: f.close_type as u32,
            flow_source: f.flow_source as u32,
            is_active_service: f.is_active_service as u32,
            queue_hash: f.queue_hash as u32,
            is_new_flow: f.is_new_flow as u32,
            tap_side: f.tap_side as u32,
            syn_seq: f.syn_seq,
            synack_seq: f.synack_seq,
            last_keepalive_seq: f.last_keepalive_seq,
            last_keepalive_ack: f.last_keepalive_ack,
        }
    }
}

pub fn get_direction(
    flow: &Flow,
    trident_type: TridentType,
    cloud_gateway_traffic: bool, // 从static config 获取
) -> (Direction, Direction, bool) {
    // 返回值分别为统计点对应的zerodoc.DirectionEnum以及及是否添加追踪数据的开关，在微软
    // 云MUX场景中，云内和云外通过VIP通信，在MUX和宿主机中采集到的流量IP地址为VIP，添加追
    // 踪数据后会将VIP替换为实际虚拟机的IP。
    fn inner(
        tap_type: TapType,
        tunnel: &TunnelField,
        l2_end: bool,
        l3_end: bool,
        is_vip: bool,
        is_unicast: bool,
        is_local_mac: bool,
        is_local_ip: bool,
        l3_epc_id: i32,
        cloud_gateway_traffic: bool, // 从static config 获取
        trident_type: TridentType,
    ) -> (Direction, Direction, bool) {
        let is_ep = l2_end && l3_end;
        let tunnel_tier = tunnel.tier;
        let mut add_tracing_doc = false;

        match trident_type {
            TridentType::TtDedicatedPhysicalMachine => {
                //  接入网络
                if tap_type != TapType::Tor {
                    if l3_epc_id != EPC_FROM_INTERNET {
                        return (
                            Direction::ClientToServer,
                            Direction::ServerToClient,
                            add_tracing_doc,
                        );
                    }
                } else {
                    // 虚拟网络
                    // 腾讯TCE场景，NFV区域的镜像流量规律如下（---表示无隧道路径，===表示有隧道路径）：
                    //   WAN ---> NFV1 ===> NFV2 ===> CVM
                    //         ^       ^ ^       ^
                    //         |       | |       `镜像流量有隧道（GRE）、左侧L2End=True
                    //         |       | `镜像流量有隧道（VXLAN/IPIP）、右侧L2End=True
                    //         |       |   <不同类NFV串联时，中间必过路由，MAC会变化>
                    //         |       `镜像流量有隧道（VXLAN/IPIP）、左侧L2End=True
                    //         `镜像流量无隧道、右侧L2End=True
                    //
                    //   CVM ===> NFV1 ===> NFV2 ===> CVM
                    //         ^
                    //         `镜像流量有隧道（GRE）、右侧L2End=True
                    //
                    //   当从WAN访问CVM时，必定有一侧是Internet IP；当云内资源经由NFV互访时，两端都不是Internet IP。
                    //   另外，穿越NFV的过程中内层IP不会变，直到目的端CVM宿主机上才会从GRE Key中提取出RSIP进行替换。
                    //
                    // 腾讯TCE场景下，通过手动录入Type=Gateway类型的宿主机，控制器下发的RemoteSegment等于Gateway的MAC。
                    // 其他场景下不会有此类宿主机，控制器下发的RemoteSegment等于**没有**KVM/K8s等本地采集器覆盖的资源MAC。
                    if l2_end {
                        if cloud_gateway_traffic {
                            // 云网关镜像（腾讯TCE等）
                            // 注意c/s方向与0/1相反
                            return (
                                Direction::ServerGatewayToClient,
                                Direction::ClientGatewayToServer,
                                add_tracing_doc,
                            );
                        } else {
                            return (
                                Direction::ClientToServer,
                                Direction::ServerToClient,
                                add_tracing_doc,
                            );
                        }
                    }
                }
            }
            TridentType::TtHyperVCompute => {
                // 仅采集宿主机物理口
                if l2_end {
                    // SNAT、LB Backend
                    // IP地址为VIP: 将双端(若不是vip_iface)的VIP替换为其MAC对对应的RIP,生成另一份doc
                    add_tracing_doc = is_vip;
                    return (
                        Direction::ClientHypervisorToServer,
                        Direction::ServerHypervisorToClient,
                        add_tracing_doc,
                    );
                }
            }
            TridentType::TtHyperVNetwork => {
                // 仅采集宿主机物理口
                if is_ep {
                    return (
                        Direction::ClientHypervisorToServer,
                        Direction::ServerHypervisorToClient,
                        add_tracing_doc,
                    );
                }

                if l2_end && is_unicast {
                    if !is_vip {
                        // Router
                        // windows hyper-v场景采集到的流量ttl还未减1，这里需要屏蔽ttl避免l3end为true
                        // 注意c/s方向与0/1相反
                        return (
                            Direction::ServerGatewayHypervisorToClient,
                            Direction::ClientGatewayHypervisorToServer,
                            add_tracing_doc,
                        );
                    } else {
                        //MUX
                        add_tracing_doc = tunnel_tier > 0;
                        return (
                            Direction::ServerGatewayHypervisorToClient,
                            Direction::ClientGatewayHypervisorToServer,
                            add_tracing_doc,
                        );
                    }
                }
            }
            TridentType::TtPublicCloud | TridentType::TtPhysicalMachine => {
                // 该采集器类型中统计位置为客户端网关/服务端网关或存在VIP时，需要增加追踪数据
                // VIP：
                //     浦发云内SLB通信场景，在VM内采集的流量无隧道IP地址使用VIP,
                //     将对端的VIP替换为其mac对应的RIP，生成另一份doc
                add_tracing_doc = is_vip;
                if is_ep {
                    return (
                        Direction::ClientToServer,
                        Direction::ServerToClient,
                        add_tracing_doc,
                    );
                } else if l2_end {
                    if is_unicast {
                        // 注意c/s方向与0/1相反
                        return (
                            Direction::ServerGatewayToClient,
                            Direction::ClientGatewayToServer,
                            add_tracing_doc,
                        );
                    }
                }
            }
            TridentType::TtHostPod | TridentType::TtVmPod => {
                if is_ep {
                    if tunnel_tier == 0 {
                        return (
                            Direction::ClientToServer,
                            Direction::ServerToClient,
                            add_tracing_doc,
                        );
                    } else {
                        // tunnelTier > 0：容器节点的出口做隧道封装
                        return (
                            Direction::ClientNodeToServer,
                            Direction::ServerNodeToClient,
                            add_tracing_doc,
                        );
                    }
                } else if l2_end {
                    if is_local_ip {
                        // 本机IP：容器节点的出口做路由转发
                        return (
                            Direction::ClientNodeToServer,
                            Direction::ServerNodeToClient,
                            add_tracing_doc,
                        );
                    } else if tunnel_tier > 0 {
                        // tunnelTier > 0：容器节点的出口做隧道封装
                        // 例如：两个容器节点之间打隧道，隧道内层IP为tunl0接口的/32隧道专用IP
                        // 但由于tunl0接口有时候没有MAC，不会被控制器记录，因此不会匹配isLocalIp的条件
                        return (
                            Direction::ClientNodeToServer,
                            Direction::ServerNodeToClient,
                            add_tracing_doc,
                        );
                    }
                    // 其他情况
                    // 举例：在tun0接收到的、本地POD发送到容器节点外部的流量
                    //       其目的MAC为tun0且l2End为真，但目的IP不是本机的IP
                } else if l3_end {
                    if is_local_mac {
                        // 本机MAC：容器节点的出口做交换转发
                        // 平安Serverless容器集群中，容器POD访问的流量特征为：
                        //   POD -> 外部：源MAC=Node MAC（Node路由转发）
                        //   POD <- 外部：目MAC=POD MAC（Node交换转发）
                        return (
                            Direction::ClientNodeToServer,
                            Direction::ServerNodeToClient,
                            add_tracing_doc,
                        );
                    }
                }
                //其他情况: BUM流量
            }
            TridentType::TtProcess => {
                if is_ep {
                    if tunnel_tier == 0 {
                        return (
                            Direction::ClientToServer,
                            Direction::ServerToClient,
                            add_tracing_doc,
                        );
                    } else {
                        // 宿主机隧道转发
                        if is_local_ip {
                            // 端点VTEP
                            return (
                                Direction::ClientHypervisorToServer,
                                Direction::ServerHypervisorToClient,
                                add_tracing_doc,
                            );
                        }
                        // 其他情况
                        // 中间VTEP：VXLAN网关（二层网关）
                    }
                } else if l2_end {
                    if is_local_ip {
                        if tunnel_tier > 0 {
                            // 容器节点作为路由器时，在宿主机出口上抓到隧道封装流量
                            return (
                                Direction::ClientHypervisorToServer,
                                Direction::ServerHypervisorToClient,
                                add_tracing_doc,
                            );
                        } else {
                            // 虚拟机或容器作为路由器时，在虚接口上抓到路由转发流量
                            // 额外追踪数据：新增的追踪数据添加MAC地址，后端通过MAC地址获取设备信息
                            return (
                                Direction::ServerGatewayToClient,
                                Direction::ClientGatewayToServer,
                                add_tracing_doc,
                            );
                        }
                    } else if is_local_mac {
                        // 本地MAC、已知单播
                        if tunnel_tier > 0 {
                            // 虚拟机作为路由器时，在宿主机出口上抓到隧道封装流量
                            if tunnel.tunnel_type == TunnelType::Ipip {
                                // 腾讯TCE的Underlay母机使用IPIP封装，外层IP为本机Underlay CVM的IP，内层IP为CLB的VIP
                                // FIXME: 目前还没有看到其他KVM使用IPIP封装的场景，这里用IPIP判断是否为TCE Underlay隧道
                                return (
                                    Direction::ClientHypervisorToServer,
                                    Direction::ServerHypervisorToClient,
                                    add_tracing_doc,
                                );
                            } else {
                                return (
                                    Direction::ServerGatewayHypervisorToClient,
                                    Direction::ClientGatewayHypervisorToServer,
                                    add_tracing_doc,
                                );
                            }
                        } else {
                            if tunnel_tier > 0 && tunnel.tunnel_type == TunnelType::TencentGre {
                                // 腾讯TCE场景，TCE-GRE隧道解封装后我们伪造了MAC地址（因此不是LocalMac）
                                // 在JNSGW场景中，Underlay CVM直接封装了GRE协议且内层IP为VIP（因此不是LocalIP）、外层IP为实IP
                                return (
                                    Direction::ClientHypervisorToServer,
                                    Direction::ServerHypervisorToClient,
                                    add_tracing_doc,
                                );
                            }
                            //其他情况:  由隧道封装的BUM包
                        }
                    } else if l3_end {
                        if is_local_mac {
                            // 交换转发：被宿主机的虚拟交换机转发的（和客户端/服务端完全一样）流量，记录为客户端宿主机、服务端宿主机
                            return (
                                Direction::ClientHypervisorToServer,
                                Direction::ServerHypervisorToClient,
                                add_tracing_doc,
                            );
                        }
                        //其他情况: BUM流量
                    } else {
                        if is_local_mac {
                            if is_local_ip {
                                // 容器节点作为路由器时，路由流量在宿主机出接口上直接做交换转发
                                // 举例：青云环境中，如果网卡做VXLAN Offload，流量会从vfXXX口经过，此时没有做隧道封装
                                //       POD与外部通信时在vfXXX口看到的MAC是容器节点的，因此l2End和l3End同时为假
                                //       此时只能通过isLocalIp来判断统计数据的direction
                                return (
                                    Direction::ClientHypervisorToServer,
                                    Direction::ServerHypervisorToClient,
                                    add_tracing_doc,
                                );
                            } else if tunnel_tier > 0 {
                                // 腾讯TCE的Underlay母机使用IPIP封装，外层IP为本机Underlay CVM的IP和MAC，内层IP为CLB的VIP
                                // 宽泛来讲，如果隧道内层是本机MAC、且L2End=false（即隧道外层不是本机MAC），也认为是到达了端点
                                return (
                                    Direction::ClientHypervisorToServer,
                                    Direction::ServerHypervisorToClient,
                                    add_tracing_doc,
                                );
                            } else {
                                return (
                                    Direction::ServerGatewayHypervisorToClient,
                                    Direction::ClientGatewayHypervisorToServer,
                                    add_tracing_doc,
                                );
                            }
                        }
                        //其他情况: BUM流量
                    }
                }
            }
            TridentType::TtVm => {
                if tunnel_tier == 0 && is_ep {
                    return (
                        Direction::ClientToServer,
                        Direction::ServerToClient,
                        add_tracing_doc,
                    );
                }
            }
            _ => {
                // 采集器类型不正确，不应该发生
                error!("invalid trident type, trident will stop");
                process::exit(1)
            }
        }
        (Direction::None, Direction::None, false)
    }

    const FLOW_METRICS_PEER_SRC: usize = 0;
    const FLOW_METRICS_PEER_DST: usize = 1;

    let flow_key = &flow.flow_key;

    // Workload和容器采集器需采集loopback口流量
    if flow_key.mac_src == flow_key.mac_dst {
        match trident_type {
            TridentType::TtPublicCloud
            | TridentType::TtPhysicalMachine
            | TridentType::TtHostPod
            | TridentType::TtVmPod => {
                return (Direction::LocalToLocal, Direction::None, false);
            }
            _ => (),
        }
    }

    // 全景图统计
    let tunnel = &flow.tunnel;
    let src_ep = &flow.flow_metrics_peers[FLOW_METRICS_PEER_SRC];
    let dst_ep = &flow.flow_metrics_peers[FLOW_METRICS_PEER_DST];
    let is_vip = src_ep.is_vip || dst_ep.is_vip;
    let (mut src_direct, _, is_extra_tracing_doc0) = inner(
        flow_key.tap_type,
        tunnel,
        src_ep.is_l2_end,
        src_ep.is_l3_end,
        is_vip,
        true,
        src_ep.is_local_mac,
        src_ep.is_local_ip,
        src_ep.l3_epc_id,
        cloud_gateway_traffic,
        trident_type,
    );
    let (_, mut dst_direct, is_extra_tracing_doc1) = inner(
        flow_key.tap_type,
        tunnel,
        dst_ep.is_l2_end,
        dst_ep.is_l3_end,
        is_vip,
        MacAddr::is_unicast(flow_key.mac_dst),
        dst_ep.is_local_mac,
        dst_ep.is_local_ip,
        dst_ep.l3_epc_id,
        cloud_gateway_traffic,
        trident_type,
    );
    // 双方向都有统计位置优先级为：client/server侧 > L2End侧 > IsLocalMac侧 > 其他
    if src_direct != Direction::None && dst_direct != Direction::None {
        if (src_direct == Direction::ClientToServer || src_ep.is_l2_end)
            && dst_direct != Direction::ServerToClient
        {
            dst_direct = Direction::None;
        } else if (dst_direct == Direction::ServerToClient || dst_ep.is_l2_end)
            && src_direct != Direction::ClientToServer
        {
            src_direct = Direction::None;
        } else if src_ep.is_local_mac {
            dst_direct = Direction::None;
        } else if dst_ep.is_local_mac {
            src_direct = Direction::None;
        }
    }

    (
        src_direct,
        dst_direct,
        is_extra_tracing_doc0 || is_extra_tracing_doc1,
    )
}

// 生成32位flowID,确保在1分钟内1个thread的flowID不重复
pub fn get_uniq_flow_id_in_one_minute(flow_id: u64) -> u64 {
    // flowID中时间低8位可保证1分钟内时间的唯一，counter可保证一秒内流的唯一性（假设fps < 2^24）
    (flow_id >> 32 & 0xff << 24) | (flow_id & COUNTER_FLOW_ID_MASK)
}
