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

pub mod consts;
mod dns;
mod http;
mod mq;
mod parser;
mod rpc;
mod sql;

pub use self::http::{
    check_http_method, get_http_request_version, get_http_resp_info, http1_check_protocol,
    http2_check_protocol, is_http_v1_payload, HttpInfo, HttpLog, Httpv2Headers,
};
pub use dns::{dns_check_protocol, DnsInfo, DnsLog};
pub use mq::{
    kafka_check_protocol, mqtt, mqtt_check_protocol, KafkaInfo, KafkaLog, MqttInfo, MqttLog,
};
pub use parser::{AppProtoLogsParser, MetaAppProto};
pub use rpc::{dubbo_check_protocol, DubboHeader, DubboInfo, DubboLog};
pub use sql::{
    decode, mysql_check_protocol, redis_check_protocol, MysqlHeader, MysqlInfo, MysqlLog,
    RedisInfo, RedisLog,
};

use std::{
    fmt,
    mem::swap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use prost::Message;
use serde::{Serialize, Serializer};

use crate::{
    common::{
        enums::{IpProtocol, PacketDirection, TapType},
        flow::L7Protocol,
        meta_packet::MetaPacket,
        tap_port::TapPort,
    },
    flow_generator::error::Result,
    metric::document::TapSide,
    proto::flow_log,
    utils::net::MacAddr,
};

const NANOS_PER_MICRO: u64 = 1000;

#[derive(Serialize, Debug, PartialEq, Copy, Clone)]
#[repr(u8)]
pub enum L7ResponseStatus {
    Ok,
    Error,
    NotExist,
    ServerError,
    ClientError,
}

impl Default for L7ResponseStatus {
    fn default() -> Self {
        L7ResponseStatus::Ok
    }
}

#[derive(Serialize, Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum LogMessageType {
    Request,
    Response,
    Session,
    Other,
    Max,
}

impl Default for LogMessageType {
    fn default() -> Self {
        LogMessageType::Other
    }
}

impl From<PacketDirection> for LogMessageType {
    fn from(d: PacketDirection) -> LogMessageType {
        match d {
            PacketDirection::ClientToServer => LogMessageType::Request,
            PacketDirection::ServerToClient => LogMessageType::Response,
        }
    }
}

#[derive(Serialize, Debug, Default, Clone)]
pub struct AppProtoHead {
    #[serde(rename = "l7_protocol")]
    pub proto: L7Protocol,
    pub msg_type: LogMessageType, // HTTP，DNS: request/response
    #[serde(rename = "response_status")]
    pub status: L7ResponseStatus, // 状态描述：0：正常，1：已废弃使用(先前用于表示异常)，2：不存在，3：服务端异常，4：客户端异常
    #[serde(rename = "response_code")]
    pub code: u16, // HTTP状态码: 1xx-5xx, DNS状态码: 0-7

    #[serde(rename = "response_duration")]
    pub rrt: u64, // HTTP，DNS时延: response-request
    #[serde(skip)]
    pub version: u8,
}

impl From<AppProtoHead> for flow_log::AppProtoHead {
    fn from(f: AppProtoHead) -> Self {
        flow_log::AppProtoHead {
            proto: f.proto as u32,
            msg_type: f.msg_type as u32,
            status: f.status as u32,
            code: f.code as u32,
            rrt: f.rrt * NANOS_PER_MICRO,
        }
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct AppProtoLogsBaseInfo {
    #[serde(serialize_with = "duration_to_micros")]
    pub start_time: Duration,
    #[serde(serialize_with = "duration_to_micros")]
    pub end_time: Duration,
    pub flow_id: u64,
    #[serde(serialize_with = "to_string_format")]
    pub tap_port: TapPort,
    pub vtap_id: u16,
    pub tap_type: TapType,
    #[serde(skip)]
    pub is_ipv6: bool,
    pub tap_side: TapSide,
    #[serde(flatten)]
    pub head: AppProtoHead,

    /* L2 */
    #[serde(serialize_with = "to_string_format")]
    pub mac_src: MacAddr,
    #[serde(serialize_with = "to_string_format")]
    pub mac_dst: MacAddr,
    /* L3 ipv4 or ipv6 */
    pub ip_src: IpAddr,
    pub ip_dst: IpAddr,
    /* L3EpcID */
    pub l3_epc_id_src: i32,
    pub l3_epc_id_dst: i32,
    /* L4 */
    pub port_src: u16,
    pub port_dst: u16,
    /* First L7 TCP Seq */
    pub req_tcp_seq: u32,
    pub resp_tcp_seq: u32,

    /* EBPF Info */
    #[serde(skip_serializing_if = "value_is_default")]
    pub process_id_0: u32,
    #[serde(skip_serializing_if = "value_is_default")]
    pub process_id_1: u32,
    #[serde(skip_serializing_if = "value_is_default")]
    pub process_kname_0: String,
    #[serde(skip_serializing_if = "value_is_default")]
    pub process_kname_1: String,
    #[serde(skip_serializing_if = "value_is_default")]
    pub syscall_trace_id_request: u64,
    #[serde(skip_serializing_if = "value_is_default")]
    pub syscall_trace_id_response: u64,
    #[serde(rename = "syscall_thread_0", skip_serializing_if = "value_is_default")]
    pub syscall_trace_id_thread_0: u32,
    #[serde(rename = "syscall_thread_1", skip_serializing_if = "value_is_default")]
    pub syscall_trace_id_thread_1: u32,
    #[serde(skip_serializing_if = "value_is_default")]
    pub syscall_cap_seq_0: u64,
    #[serde(skip_serializing_if = "value_is_default")]
    pub syscall_cap_seq_1: u64,

    pub protocol: IpProtocol,
    #[serde(skip)]
    pub is_vip_interface_src: bool,
    #[serde(skip)]
    pub is_vip_interface_dst: bool,
}

pub fn duration_to_micros<S>(d: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(d.as_micros() as u64)
}

pub fn to_string_format<D, S>(d: &D, serializer: S) -> Result<S::Ok, S::Error>
where
    D: fmt::Display,
    S: Serializer,
{
    serializer.serialize_str(&d.to_string())
}

pub fn value_is_default<T>(t: &T) -> bool
where
    T: Default + std::cmp::PartialEq,
{
    t == &T::default()
}

pub fn value_is_negative<T>(t: &T) -> bool
where
    T: Default + std::cmp::PartialEq + std::cmp::PartialOrd,
{
    t < &T::default()
}

impl From<AppProtoLogsBaseInfo> for flow_log::AppProtoLogsBaseInfo {
    fn from(f: AppProtoLogsBaseInfo) -> Self {
        let (ip4_src, ip4_dst, ip6_src, ip6_dst) = match (f.ip_src, f.ip_dst) {
            (IpAddr::V4(ip4), IpAddr::V4(ip4_1)) => {
                (ip4, ip4_1, Ipv6Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED)
            }
            (IpAddr::V6(ip6), IpAddr::V6(ip6_1)) => {
                (Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED, ip6, ip6_1)
            }
            _ => panic!("ip_src,ip_dst type mismatch"),
        };
        flow_log::AppProtoLogsBaseInfo {
            start_time: f.start_time.as_nanos() as u64,
            end_time: f.end_time.as_nanos() as u64,
            flow_id: f.flow_id,
            tap_port: f.tap_port.0,
            vtap_id: f.vtap_id as u32,
            tap_type: u16::from(f.tap_type) as u32,
            is_ipv6: f.is_ipv6 as u32,
            tap_side: f.tap_side as u32,
            head: Some(f.head.into()),
            mac_src: f.mac_src.into(),
            mac_dst: f.mac_dst.into(),
            ip_src: u32::from_be_bytes(ip4_src.octets()),
            ip_dst: u32::from_be_bytes(ip4_dst.octets()),
            ip6_src: ip6_src.octets().to_vec(),
            ip6_dst: ip6_dst.octets().to_vec(),
            l3_epc_id_src: f.l3_epc_id_src,
            l3_epc_id_dst: f.l3_epc_id_dst,
            port_src: f.port_src as u32,
            port_dst: f.port_dst as u32,
            protocol: f.protocol as u32,
            is_vip_interface_src: f.is_vip_interface_src as u32,
            is_vip_interface_dst: f.is_vip_interface_dst as u32,
            req_tcp_seq: f.req_tcp_seq,
            resp_tcp_seq: f.resp_tcp_seq,
            process_id_0: f.process_id_0,
            process_id_1: f.process_id_1,
            process_kname_0: f.process_kname_0,
            process_kname_1: f.process_kname_1,
            syscall_trace_id_request: f.syscall_trace_id_request,
            syscall_trace_id_response: f.syscall_trace_id_response,
            syscall_trace_id_thread_0: f.syscall_trace_id_thread_0,
            syscall_trace_id_thread_1: f.syscall_trace_id_thread_1,
            syscall_cap_seq_0: f.syscall_cap_seq_0 as u32,
            syscall_cap_seq_1: f.syscall_cap_seq_1 as u32,
        }
    }
}

impl AppProtoLogsBaseInfo {
    pub fn from_ebpf(
        packet: &MetaPacket,
        head: AppProtoHead,
        vtap_id: u16,
        local_epc: i32,
        remote_epc: i32,
    ) -> Self {
        let is_src = packet.lookup_key.l2_end_0;
        let direction = packet.direction;
        let mut info = Self {
            start_time: packet.lookup_key.timestamp,
            end_time: packet.lookup_key.timestamp,
            flow_id: packet.socket_id,
            tap_port: packet.tap_port,
            tap_type: TapType::Tor,
            is_ipv6: packet.lookup_key.dst_ip.is_ipv6(),
            tap_side: if is_src {
                TapSide::ClientProcess
            } else {
                TapSide::ServerProcess
            },

            mac_src: packet.lookup_key.src_mac,
            mac_dst: packet.lookup_key.dst_mac,
            ip_src: packet.lookup_key.src_ip,
            ip_dst: packet.lookup_key.dst_ip,
            port_src: packet.lookup_key.src_port,
            port_dst: packet.lookup_key.dst_port,
            protocol: packet.lookup_key.proto,

            process_id_0: if is_src { packet.process_id } else { 0 },
            process_id_1: if !is_src { packet.process_id } else { 0 },
            process_kname_0: if is_src {
                packet.process_name.clone()
            } else {
                "".to_string()
            },
            process_kname_1: if !is_src {
                packet.process_name.clone()
            } else {
                "".to_string()
            },

            syscall_trace_id_request: if direction == PacketDirection::ClientToServer {
                packet.syscall_trace_id
            } else {
                0
            },
            syscall_trace_id_response: if direction == PacketDirection::ServerToClient {
                packet.syscall_trace_id
            } else {
                0
            },
            req_tcp_seq: if direction == PacketDirection::ClientToServer {
                packet.tcp_data.seq
            } else {
                0
            },
            resp_tcp_seq: if direction == PacketDirection::ServerToClient {
                packet.tcp_data.seq
            } else {
                0
            },
            syscall_trace_id_thread_0: if direction == PacketDirection::ClientToServer {
                packet.thread_id
            } else {
                0
            },
            syscall_trace_id_thread_1: if direction == PacketDirection::ServerToClient {
                packet.thread_id
            } else {
                0
            },
            syscall_cap_seq_0: if direction == PacketDirection::ClientToServer {
                packet.cap_seq
            } else {
                0
            },
            syscall_cap_seq_1: if direction == PacketDirection::ServerToClient {
                packet.cap_seq
            } else {
                0
            },
            vtap_id,
            head,
            l3_epc_id_src: if is_src { local_epc } else { remote_epc },
            l3_epc_id_dst: if is_src { remote_epc } else { local_epc },
            is_vip_interface_src: false,
            is_vip_interface_dst: false,
        };
        if direction == PacketDirection::ServerToClient {
            swap(&mut info.mac_src, &mut info.mac_dst);
            swap(&mut info.ip_src, &mut info.ip_dst);
            swap(&mut info.l3_epc_id_src, &mut info.l3_epc_id_dst);
            swap(&mut info.port_src, &mut info.port_dst);
            swap(&mut info.process_id_0, &mut info.process_id_1);
            swap(&mut info.process_kname_0, &mut info.process_kname_1);
            info.tap_side = if info.tap_side == TapSide::ClientProcess {
                TapSide::ServerProcess
            } else {
                TapSide::ClientProcess
            };
        }

        info
    }
    // 请求调用回应来合并
    fn merge(&mut self, log: AppProtoLogsBaseInfo) {
        if log.process_id_0 > 0 {
            self.process_id_0 = log.process_id_0;
            self.process_kname_0 = log.process_kname_0;
        }
        if log.process_id_1 > 0 {
            self.process_id_1 = log.process_id_1;
            self.process_kname_1 = log.process_kname_1;
        }
        self.syscall_trace_id_thread_1 = log.syscall_trace_id_thread_1;
        self.syscall_cap_seq_1 = log.syscall_cap_seq_1;
        self.end_time = log.end_time.max(self.start_time);
        self.resp_tcp_seq = log.resp_tcp_seq;
        self.syscall_trace_id_response = log.syscall_trace_id_response;
        self.head.msg_type = LogMessageType::Session;
        self.head.code = log.head.code;
        self.head.status = log.head.status;
        self.head.rrt = log.head.rrt;
    }
}

#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum AppProtoLogsInfo {
    Dns(DnsInfo),
    Mysql(MysqlInfo),
    Redis(RedisInfo),
    Kafka(KafkaInfo),
    Mqtt(MqttInfo),
    Dubbo(DubboInfo),
    HttpV1(HttpInfo),
    HttpV2(HttpInfo),
    HttpV1TLS(HttpInfo),
}

impl AppProtoLogsInfo {
    fn session_id(&self) -> Option<u32> {
        match self {
            AppProtoLogsInfo::Dns(t) if t.trans_id > 0 => Some(t.trans_id as u32),
            AppProtoLogsInfo::Kafka(t) if t.correlation_id > 0 => Some(t.correlation_id),
            AppProtoLogsInfo::Dubbo(t) if t.serial_id > 0 => Some(t.serial_id as u32),
            AppProtoLogsInfo::HttpV2(t) if t.stream_id > 0 => Some(t.stream_id),
            _ => None,
        }
    }

    fn merge(&mut self, other: Self) {
        match (self, other) {
            (Self::Dns(m), Self::Dns(o)) => m.merge(o),
            (Self::Mysql(m), Self::Mysql(o)) => m.merge(o),
            (Self::Redis(m), Self::Redis(o)) => m.merge(o),
            (Self::Kafka(m), Self::Kafka(o)) => m.merge(o),
            (Self::Mqtt(m), Self::Mqtt(o)) => m.merge(o),
            (Self::Dubbo(m), Self::Dubbo(o)) => m.merge(o),
            (Self::HttpV1(m), Self::HttpV1(o)) => m.merge(o),
            (Self::HttpV2(m), Self::HttpV2(o)) => m.merge(o),
            (Self::HttpV1TLS(m), Self::HttpV1TLS(o)) => m.merge(o),
            _ => unreachable!(),
        }
    }
}

impl fmt::Display for AppProtoLogsInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dns(l) => write!(f, "{:?}", l),
            Self::Mysql(l) => write!(f, "{:?}", l),
            Self::Redis(l) => write!(f, "{}", l),
            Self::Dubbo(l) => write!(f, "{:?}", l),
            Self::Kafka(l) => write!(f, "{:?}", l),
            Self::Mqtt(l) => write!(f, "{:?}", l),
            Self::HttpV1(l) => write!(f, "{:?}", l),
            Self::HttpV2(l) => write!(f, "{:?}", l),
            Self::HttpV1TLS(l) => write!(f, "{:?}", l),
        }
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct AppProtoLogsData {
    #[serde(flatten)]
    pub base_info: AppProtoLogsBaseInfo,
    #[serde(flatten)]
    pub special_info: AppProtoLogsInfo,
}

impl fmt::Display for AppProtoLogsData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n", self.base_info)?;
        write!(f, "\t{}", self.special_info)
    }
}

impl AppProtoLogsData {
    pub fn new(base_info: AppProtoLogsBaseInfo, special_info: AppProtoLogsInfo) -> Self {
        Self {
            base_info,
            special_info,
        }
    }

    pub fn encode(self, buf: &mut Vec<u8>) -> Result<usize, prost::EncodeError> {
        let mut pb_proto_logs_data = flow_log::AppProtoLogsData {
            base: Some(self.base_info.into()),
            ..Default::default()
        };
        match self.special_info {
            AppProtoLogsInfo::Dns(t) => pb_proto_logs_data.dns = Some(t.into()),
            AppProtoLogsInfo::Mysql(t) => pb_proto_logs_data.mysql = Some(t.into()),
            AppProtoLogsInfo::Redis(t) => pb_proto_logs_data.redis = Some(t.into()),
            AppProtoLogsInfo::Kafka(t) => pb_proto_logs_data.kafka = Some(t.into()),
            AppProtoLogsInfo::Mqtt(t) => pb_proto_logs_data.mqtt = Some(t.into()),
            AppProtoLogsInfo::Dubbo(t) => pb_proto_logs_data.dubbo = Some(t.into()),
            AppProtoLogsInfo::HttpV1(t) => pb_proto_logs_data.http = Some(t.into()),
            AppProtoLogsInfo::HttpV2(t) => pb_proto_logs_data.http = Some(t.into()),
            AppProtoLogsInfo::HttpV1TLS(t) => pb_proto_logs_data.http = Some(t.into()),
        };

        pb_proto_logs_data
            .encode(buf)
            .map(|_| pb_proto_logs_data.encoded_len())
    }

    pub fn ebpf_flow_session_id(&self) -> u64 {
        // 取flow_id(即ebpf底层的socket id)的高8位(cpu id)+低24位(socket id的变化增量), 作为聚合id的高32位
        let flow_id_part =
            (self.base_info.flow_id >> 56 << 56) | (self.base_info.flow_id << 40 >> 8);
        if let Some(session_id) = self.special_info.session_id() {
            flow_id_part
                | (self.base_info.head.proto as u64) << 24
                | ((session_id as u64) & 0xffffff)
        } else {
            let mut cap_seq = self
                .base_info
                .syscall_cap_seq_0
                .max(self.base_info.syscall_cap_seq_1);
            if self.base_info.head.msg_type == LogMessageType::Request {
                cap_seq += 1;
            };
            flow_id_part | ((self.base_info.head.proto as u64) << 24) | (cap_seq & 0xffffff)
        }
    }

    pub fn session_merge(&mut self, log: AppProtoLogsData) {
        self.base_info.merge(log.base_info);
        self.special_info.merge(log.special_info);
    }

    pub fn to_kv_string(&self, dst: &mut String) {
        let json = serde_json::to_string(&self).unwrap();
        dst.push_str(&json);
        dst.push('\n');
    }
}

impl fmt::Display for AppProtoLogsBaseInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Timestamp: {:?} Vtap_id: {} Flow_id: {} TapType: {} TapPort: {} TapSide: {:?}\n \
                \t{}_{}_{} -> {}_{}_{} Proto: {:?} Seq: {} -> {} VIP: {} -> {} EPC: {} -> {}\n \
                \tProcess: {}:{} -> {}:{} Trace-id: {} -> {} Thread: {} -> {} cap_seq: {} -> {}\n \
                \tL7Protocol: {:?} MsgType: {:?} Status: {:?} Code: {} Rrt: {}",
            self.start_time,
            self.vtap_id,
            self.flow_id,
            self.tap_type,
            self.tap_port,
            self.tap_side,
            self.mac_src,
            self.ip_src,
            self.port_src,
            self.mac_dst,
            self.ip_dst,
            self.port_dst,
            self.protocol,
            self.req_tcp_seq,
            self.resp_tcp_seq,
            self.is_vip_interface_src,
            self.is_vip_interface_dst,
            self.l3_epc_id_src,
            self.l3_epc_id_dst,
            self.process_kname_0,
            self.process_id_0,
            self.process_kname_1,
            self.process_id_1,
            self.syscall_trace_id_request,
            self.syscall_trace_id_response,
            self.syscall_trace_id_thread_0,
            self.syscall_trace_id_thread_1,
            self.syscall_cap_seq_0,
            self.syscall_cap_seq_1,
            self.head.proto,
            self.head.msg_type,
            self.head.status,
            self.head.code,
            self.head.rrt
        )
    }
}

pub trait L7LogParse: Send + Sync {
    fn parse(
        &mut self,
        payload: &[u8],
        proto: IpProtocol,
        direction: PacketDirection,
    ) -> Result<AppProtoHeadEnum>;
    fn info(&self) -> AppProtoLogsInfoEnum;
}

#[derive(Debug, Clone)]
pub enum AppProtoHeadEnum {
    Single(AppProtoHead),
    Multi(Vec<AppProtoHead>),
}

#[derive(Debug, Clone)]
pub enum AppProtoLogsInfoEnum {
    Single(AppProtoLogsInfo),
    Multi(Vec<AppProtoLogsInfo>),
}

impl AppProtoLogsInfoEnum {
    /// 假设所有应用日志都只携带一个info，如果有多个则不要用该方法
    /// Assuming that all application logs carry only one info, do not use this method if there are multiple
    pub fn into_inner(self) -> AppProtoLogsInfo {
        match self {
            Self::Single(v) => v,
            Self::Multi(_) => unreachable!(),
        }
    }
}

impl IntoIterator for AppProtoHeadEnum {
    type Item = AppProtoHead;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Single(v) => vec![v].into_iter(),
            Self::Multi(v) => v.into_iter(),
        }
    }
}

impl IntoIterator for AppProtoLogsInfoEnum {
    type Item = AppProtoLogsInfo;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Single(v) => vec![v].into_iter(),
            Self::Multi(v) => v.into_iter(),
        }
    }
}
