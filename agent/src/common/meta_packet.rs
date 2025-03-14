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

use std::fmt;
use std::mem;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::{error::Error, ffi::CStr, net::Ipv6Addr};

use pnet::packet::{
    icmp::{IcmpType, IcmpTypes},
    icmpv6::{Icmpv6Type, Icmpv6Types},
    tcp::{TcpOptionNumber, TcpOptionNumbers},
};

#[cfg(target_os = "linux")]
use super::enums::TapType;
use super::{
    consts::*,
    decapsulate::TunnelInfo,
    endpoint::EndpointData,
    enums::{EthernetType, HeaderType, IpProtocol, PacketDirection, TcpFlags},
    flow::L7Protocol,
    lookup_key::LookupKey,
    policy::PolicyData,
    tap_port::TapPort,
};

#[cfg(target_os = "linux")]
use crate::ebpf::{SK_BPF_DATA, SOCK_DIR_RCV, SOCK_DIR_SND};
use crate::error;
use crate::utils::net::{is_unicast_link_local, MacAddr};

#[derive(Debug, Default)]
pub struct MetaPacket<'a> {
    // 主机序, 不因L2End1而颠倒, 端口会在查询策略时被修改
    pub lookup_key: LookupKey,

    pub raw: Option<&'a [u8]>,
    pub packet_len: usize,
    vlan_tag_size: usize,
    pub ttl: u8,
    pub reset_ttl: bool,
    pub endpoint_data: Option<Arc<EndpointData>>,
    pub policy_data: Option<Arc<PolicyData>>,

    offset_ip_0: usize,
    offset_ip_1: usize,
    offset_mac_0: usize,
    offset_mac_1: usize,
    offset_port_0: usize,
    offset_port_1: usize,
    offset_ipv6_last_option: usize,
    offset_ipv6_fragment_option: usize,

    pub header_type: HeaderType,
    // 读取时不要直接用这个字段，用MetaPacket.GetPktSize()
    // 注意：不含镜像外层VLAN的四个字节
    l2_l3_opt_size: usize, // 802.1Q + IPv4 optional fields
    l4_opt_size: usize,    // ICMP payload / TCP optional fields
    l3_payload_len: usize,
    l4_payload_len: usize,
    npb_ignore_l4: bool, // 对于IP分片或IP Options不全的情况，分发时不对l4进行解析
    nd_reply_or_arp_request: bool, // NDP request or ARP request

    pub tunnel: Option<&'a TunnelInfo>,

    data_offset_ihl_or_fl4b: u8,
    next_header: u8, // ipv6 header中的nextHeader字段，用于包头压缩等

    tcp_options_flag: u8,
    tcp_opt_win_scale_offset: usize,
    tcp_opt_mss_offset: usize,
    tcp_opt_sack_offset: usize,

    pub tcp_data: MetaPacketTcpHeader,
    pub tap_port: TapPort, // packet与xflow复用
    pub payload_len: u16,
    pub vlan: u16,
    pub direction: PacketDirection,
    pub is_active_service: bool,
    pub queue_hash: u8,

    // for xflow
    pub packet_count: u64,
    pub packet_bytes: u64,
    pub start_time: Duration,
    pub end_time: Duration,
    pub source_ip: u32,

    // for ebpf
    pub raw_from_ebpf: Vec<u8>,

    pub socket_id: u64,
    pub cap_seq: u64,
    pub l7_protocol_from_ebpf: L7Protocol,

    pub process_id: u32,
    pub thread_id: u32,
    pub syscall_trace_id: u64,
    pub process_name: String,
}

impl<'a> MetaPacket<'a> {
    pub fn timestamp_adjust(&mut self, time_diff: i64) {
        if time_diff >= 0 {
            self.lookup_key.timestamp += Duration::from_nanos(time_diff as u64);
        } else {
            self.lookup_key.timestamp -= Duration::from_nanos(-time_diff as u64);
        }
    }

    pub fn empty() -> MetaPacket<'a> {
        MetaPacket {
            offset_mac_0: FIELD_OFFSET_SA,
            offset_mac_1: FIELD_OFFSET_DA,
            offset_ip_0: FIELD_OFFSET_SIP,
            offset_ip_1: FIELD_OFFSET_DIP,
            offset_port_0: FIELD_OFFSET_SPORT,
            offset_port_1: FIELD_OFFSET_DPORT,
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        *self = Self::empty();
    }

    pub fn is_reversed(&self) -> bool {
        self.lookup_key.l2_end_1
    }

    pub fn is_ndp_response(&self) -> bool {
        self.nd_reply_or_arp_request && self.lookup_key.proto == IpProtocol::Icmpv6
    }

    pub fn is_syn(&self) -> bool {
        self.tcp_data.flags & TcpFlags::MASK == TcpFlags::SYN
    }

    pub fn is_syn_ack(&self) -> bool {
        self.tcp_data.flags & TcpFlags::MASK == TcpFlags::SYN_ACK && self.payload_len == 0
    }

    pub fn is_ack(&self) -> bool {
        self.tcp_data.flags & TcpFlags::MASK == TcpFlags::ACK && self.payload_len == 0
    }

    pub fn is_psh_ack(&self) -> bool {
        self.tcp_data.flags & TcpFlags::MASK == TcpFlags::PSH_ACK && self.payload_len > 1
    }

    pub fn has_valid_payload(&self) -> bool {
        self.payload_len > 1
    }

    pub fn tcp_options_size(&self) -> usize {
        if (self.header_type != HeaderType::Ipv4Tcp && self.header_type != HeaderType::Ipv6Tcp)
            && self.l4_opt_size == 0
        {
            return 0;
        }
        let mut size = 1;
        if self.tcp_options_flag & TCP_OPT_FLAG_MSS != 0 {
            size += TCP_OPT_MSS_LEN - 2;
        }
        if self.tcp_options_flag & TCP_OPT_FLAG_WIN_SCALE != 0 {
            size += TCP_OPT_WIN_SCALE_LEN - 2;
        }
        size + (self.tcp_options_flag & TCP_OPT_FLAG_SACK) as usize
    }

    fn update_tcp_opt(&mut self) {
        let packet = self.raw.as_ref().unwrap();
        let mut offset = self.header_type.min_packet_size() + self.l2_l3_opt_size;
        let payload_offset = offset + self.l4_opt_size;

        while offset + 1 < payload_offset {
            // 如果不足2B，EOL和NOP都可以忽略
            let assume_length = packet[offset + 1].max(2) as usize;
            match TcpOptionNumber::new(packet[offset]) {
                TcpOptionNumbers::EOL => return,
                TcpOptionNumbers::NOP => offset += 1,
                TcpOptionNumbers::MSS => {
                    if offset + TCP_OPT_MSS_LEN > payload_offset {
                        return;
                    }
                    self.tcp_opt_mss_offset = offset + 2;
                    self.tcp_options_flag |= TCP_OPT_FLAG_MSS;
                    offset += TCP_OPT_MSS_LEN;
                    self.tcp_data.mss = u16::from_be_bytes(
                        *<&[u8; 2]>::try_from(
                            &packet[self.tcp_opt_mss_offset..self.tcp_opt_mss_offset + 2],
                        )
                        .unwrap(),
                    );
                }
                TcpOptionNumbers::WSCALE => {
                    if offset + TCP_OPT_WIN_SCALE_LEN > payload_offset {
                        return;
                    }
                    self.tcp_opt_win_scale_offset = offset + 2;
                    self.tcp_options_flag |= TCP_OPT_FLAG_WIN_SCALE;
                    offset += TCP_OPT_WIN_SCALE_LEN;
                    self.tcp_data.win_scale = packet[self.tcp_opt_win_scale_offset];
                }
                TcpOptionNumbers::SACK_PERMITTED => {
                    self.tcp_options_flag |= TCP_OPT_FLAG_SACK_PERMIT;
                    offset += 2;
                    self.tcp_data.sack_permitted = true;
                }
                TcpOptionNumbers::SACK => {
                    if offset + assume_length > payload_offset {
                        return;
                    }
                    let sack_size = assume_length - 2;
                    if sack_size > 32 {
                        return;
                    }
                    self.tcp_opt_sack_offset = offset + 2;
                    self.tcp_options_flag |= sack_size as u8;
                    offset += assume_length;
                    let mut sack = Vec::with_capacity(sack_size);
                    sack.extend_from_slice(
                        &packet[self.tcp_opt_sack_offset..self.tcp_opt_sack_offset + sack_size],
                    );
                    self.tcp_data.sack.replace(sack);
                }
                _ => offset += assume_length,
            }
        }
    }

    fn update_ip6_opt(&mut self, l2_opt_size: usize) -> (u8, usize) {
        let packet = self.raw.as_ref().unwrap();
        let mut next_header = packet[IPV6_PROTO_OFFSET + l2_opt_size];
        let original_offset = ETH_HEADER_SIZE + IPV6_HEADER_SIZE + l2_opt_size;
        let mut option_offset = original_offset;
        self.next_header = next_header;
        let mut size_checker = packet.len() as isize - option_offset as isize;
        loop {
            if let Ok(header) = IpProtocol::try_from(next_header) {
                match header {
                    IpProtocol::Ah => {
                        if size_checker < 2 {
                            break;
                        }
                        self.offset_ipv6_last_option = option_offset;
                        next_header = packet[option_offset];
                        let length = (packet[option_offset + 1] as usize + 2) * 4;
                        option_offset += length;
                        size_checker -= length as isize;
                        if size_checker < 0 {
                            break;
                        }
                        continue;
                    }
                    IpProtocol::Ipv6Destination
                    | IpProtocol::Ipv6HopByHop
                    | IpProtocol::Ipv6Routing => {
                        size_checker -= 8;
                        if size_checker < 0 {
                            break;
                        }
                        self.offset_ipv6_last_option = option_offset;
                        next_header = packet[option_offset];
                        let length = packet[option_offset + 1] as usize;
                        option_offset += length * 8 + 8;
                        size_checker -= length as isize * 8;
                        if size_checker < 0 {
                            break;
                        }
                        continue;
                    }
                    IpProtocol::Ipv6Fragment => {
                        size_checker -= 8;
                        if size_checker < 0 {
                            break;
                        }
                        self.offset_ipv6_last_option = option_offset;
                        self.offset_ipv6_fragment_option = option_offset;
                        next_header = packet[option_offset];
                        option_offset += 8;
                        continue;
                    }
                    IpProtocol::Icmpv6 => {
                        self.offset_ipv6_last_option = option_offset;
                        return (next_header, option_offset - original_offset);
                    }
                    IpProtocol::Esp => {
                        self.offset_ipv6_last_option = option_offset;
                        option_offset += size_checker as usize;
                        return (next_header, option_offset - original_offset);
                    }
                    _ => (),
                }
            }
            // header types unknown or not matched
            return (next_header, option_offset - original_offset);
        }
        self.offset_ipv6_last_option = 0;
        self.offset_ipv6_fragment_option = 0;
        (packet[IPV6_PROTO_OFFSET + l2_opt_size], 0)
    }

    pub fn get_pkt_size(&self) -> u16 {
        if self.packet_len < u16::MAX as usize {
            self.packet_len as u16
        } else {
            u16::MAX
        }
    }

    pub fn get_restored_packet_size(&self) -> u16 {
        // 压缩包头仅支持发送最内层的VLAN，所以QINQ场景下长度不能计算外层的VLAN
        let mut skip_vlan_header_size = 0;
        if self.vlan_tag_size > VLAN_HEADER_SIZE {
            // QinQ
            skip_vlan_header_size = self.vlan_tag_size - VLAN_HEADER_SIZE;
        }
        let packet_size = self.get_pkt_size();
        if packet_size == 0 {
            packet_size
        } else {
            packet_size - skip_vlan_header_size as u16
        }
    }

    // 目前仅支持获取UDP或TCP的Paylaod
    pub fn get_l4_payload(&self) -> Option<&[u8]> {
        if self.lookup_key.proto != IpProtocol::Tcp && self.lookup_key.proto != IpProtocol::Udp {
            return None;
        }
        if self.tap_port.is_from(TapPort::FROM_EBPF) {
            return Some(&self.raw_from_ebpf);
        }

        let packet_header_size =
            self.header_type.min_packet_size() + self.l2_l3_opt_size + self.l4_opt_size;
        if let Some(raw) = self.raw.as_ref() {
            if raw.len() > packet_header_size {
                return Some(&raw[packet_header_size..]);
            }
        }
        None
    }

    pub fn update(
        &mut self,
        packet: &'a [u8],
        src_endpoint: bool,
        dst_endpoint: bool,
        timestamp: Duration,
        original_length: usize,
    ) -> error::Result<()> {
        fn read_u16_be(bs: &[u8]) -> u16 {
            assert!(bs.len() >= 2);
            u16::from_be_bytes(*<&[u8; 2]>::try_from(&bs[..2]).unwrap())
        }
        fn read_u32_be(bs: &[u8]) -> u32 {
            assert!(bs.len() >= 4);
            u32::from_be_bytes(*<&[u8; 4]>::try_from(&bs[..4]).unwrap())
        }
        self.lookup_key.timestamp = timestamp;
        self.raw = Some(packet);
        let packet = self.raw.as_ref().unwrap();
        self.lookup_key.l2_end_0 = src_endpoint;
        self.lookup_key.l2_end_1 = dst_endpoint;
        self.packet_len = packet.len();
        let mut size_checker = packet.len() as isize;

        // eth
        size_checker -= HeaderType::Eth.min_header_size() as isize;
        if size_checker < 0 {
            return Err(error::Error::ParsePacketFailed("packet truncated".into()));
        }
        let mut vlan_tag_size = 0;
        let mut eth_type = EthernetType::try_from(read_u16_be(&packet[FIELD_OFFSET_ETH_TYPE..]))
            .map_err(|e| {
                error::Error::ParsePacketFailed(format!("parse eth_type failed: {}", e))
            })?;
        if eth_type == EthernetType::Dot1Q {
            vlan_tag_size = VLAN_HEADER_SIZE;
            size_checker -= VLAN_HEADER_SIZE as isize;
            if size_checker < 0 {
                return Err(error::Error::ParsePacketFailed("packet truncated".into()));
            }
            let vlan_tag = read_u16_be(&packet[FIELD_OFFSET_ETH_TYPE + ETH_TYPE_LEN..]);
            self.vlan = vlan_tag & VLAN_ID_MASK;
            eth_type = EthernetType::try_from(read_u16_be(
                &packet[FIELD_OFFSET_ETH_TYPE + vlan_tag_size..],
            ))
            .map_err(|e| {
                error::Error::ParsePacketFailed(format!("parse eth_type failed: {}", e))
            })?;
            if eth_type == EthernetType::Dot1Q {
                vlan_tag_size = VLAN_HEADER_SIZE;
                size_checker -= VLAN_HEADER_SIZE as isize;
                if size_checker < 0 {
                    return Err(error::Error::ParsePacketFailed("packet truncated".into()));
                }
                let vlan_tag = read_u16_be(
                    &packet
                        [FIELD_OFFSET_ETH_TYPE + ETH_TYPE_LEN + ETH_TYPE_LEN + VLAN_HEADER_SIZE..],
                );
                self.vlan = vlan_tag & VLAN_ID_MASK;
                eth_type = EthernetType::try_from(read_u16_be(
                    &packet[FIELD_OFFSET_ETH_TYPE + vlan_tag_size..],
                ))
                .map_err(|e| {
                    error::Error::ParsePacketFailed(format!("parse eth_type failed: {}", e))
                })?;
            }
        }
        self.lookup_key.eth_type = eth_type;
        self.lookup_key.src_mac =
            MacAddr::try_from(&packet[FIELD_OFFSET_SA..FIELD_OFFSET_SA + MAC_ADDR_LEN]).unwrap();
        self.lookup_key.dst_mac =
            MacAddr::try_from(&packet[FIELD_OFFSET_DA..FIELD_OFFSET_DA + MAC_ADDR_LEN]).unwrap();

        self.header_type = HeaderType::Eth;
        self.vlan_tag_size = vlan_tag_size;
        if dst_endpoint {
            // inbound
            mem::swap(&mut self.offset_mac_0, &mut self.offset_mac_1);
        }
        let ip_protocol;
        match eth_type {
            EthernetType::Arp => {
                size_checker -= HeaderType::Arp.min_header_size() as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                self.header_type = HeaderType::Arp;
                let spa_offset = ARP_SPA_OFFSET + vlan_tag_size;
                let tpa_offset = ARP_TPA_OFFSET + vlan_tag_size;
                self.lookup_key.src_ip = IpAddr::from(
                    *<&[u8; 4]>::try_from(&packet[spa_offset..spa_offset + IPV4_ADDR_LEN]).unwrap(),
                );
                self.lookup_key.dst_ip = IpAddr::from(
                    *<&[u8; 4]>::try_from(&packet[tpa_offset..tpa_offset + IPV4_ADDR_LEN]).unwrap(),
                );
                self.nd_reply_or_arp_request =
                    read_u16_be(&packet[self.vlan_tag_size + ARP_OP_OFFSET..]) == arp::OP_REQUEST;
                if dst_endpoint {
                    self.offset_ip_1 = spa_offset;
                    self.offset_ip_0 = tpa_offset;
                } else {
                    self.offset_ip_0 = spa_offset;
                    self.offset_ip_1 = tpa_offset;
                }
                return Ok(());
            }
            EthernetType::Ipv6 => {
                size_checker -= HeaderType::Ipv6.min_header_size() as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                self.header_type = HeaderType::Ipv6;
                self.offset_ip_0 = FIELD_OFFSET_IPV6_SRC + vlan_tag_size;
                self.offset_ip_1 = FIELD_OFFSET_IPV6_DST + vlan_tag_size;
                self.lookup_key.src_ip = IpAddr::from(
                    *<&[u8; 16]>::try_from(
                        &packet[self.offset_ip_0..self.offset_ip_0 + IPV6_ADDR_LEN],
                    )
                    .unwrap(),
                );
                self.lookup_key.dst_ip = IpAddr::from(
                    *<&[u8; 16]>::try_from(
                        &packet[self.offset_ip_1..self.offset_ip_1 + IPV6_ADDR_LEN],
                    )
                    .unwrap(),
                );
                self.ttl = packet[IPV6_HOP_LIMIT_OFFSET + vlan_tag_size];
                if dst_endpoint {
                    mem::swap(&mut self.offset_ip_0, &mut self.offset_ip_1);
                }
                // 为了不影响L4层的字段偏移，ipv6比ipv4多的20个字节放入m.l2l3OptSize
                self.l2_l3_opt_size = IPV6_HEADER_ADJUST + vlan_tag_size;
                let mut payload = read_u16_be(&packet[FIELD_OFFSET_PAYLOAD_LEN + vlan_tag_size..]);
                // e1000网卡驱动，在开启TSO功能时，IPv6的payload可能为0
                // e1000网卡驱动：https://elixir.bootlin.com/linux/v3.0/source/drivers/net/e1000e/netdev.c#L4423
                if payload == 0 {
                    payload = size_checker as u16;
                }
                let label = read_u32_be(&packet[FIELD_OFFSET_PAYLOAD_LEN + vlan_tag_size..]);
                self.data_offset_ihl_or_fl4b |= ((label >> 16) & 0xf) as u8;
                let r = self.update_ip6_opt(vlan_tag_size);
                ip_protocol = IpProtocol::try_from(r.0).map_err(|e| {
                    error::Error::ParsePacketFailed(format!("parse ip_protocol failed: {}", e))
                })?;
                let options_length = r.1;
                self.l2_l3_opt_size += options_length;
                self.packet_len = payload as usize
                    + HeaderType::Ipv6.min_packet_size()
                    + vlan_tag_size
                    + IPV6_HEADER_ADJUST;
                self.lookup_key.proto = ip_protocol;

                size_checker -= options_length as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                self.l3_payload_len = size_checker as usize;
            }
            EthernetType::Ipv4 => {
                size_checker -= HeaderType::Ipv4.min_header_size() as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                self.header_type = HeaderType::Ipv4;
                let ihl = packet[FIELD_OFFSET_IHL + vlan_tag_size] & 0xF;
                self.data_offset_ihl_or_fl4b = ihl;

                self.offset_ip_0 += vlan_tag_size;
                self.offset_ip_1 += vlan_tag_size;
                self.lookup_key.src_ip = IpAddr::from(
                    *<&[u8; 4]>::try_from(
                        &packet[self.offset_ip_0..self.offset_ip_0 + IPV4_ADDR_LEN],
                    )
                    .unwrap(),
                );
                self.lookup_key.dst_ip = IpAddr::from(
                    *<&[u8; 4]>::try_from(
                        &packet[self.offset_ip_1..self.offset_ip_1 + IPV4_ADDR_LEN],
                    )
                    .unwrap(),
                );
                self.ttl = packet[IPV4_TTL_OFFSET + vlan_tag_size];
                if dst_endpoint {
                    mem::swap(&mut self.offset_ip_0, &mut self.offset_ip_1);
                }

                let mut total_length =
                    read_u16_be(&packet[FIELD_OFFSET_TOTAL_LEN + vlan_tag_size..]) as usize;
                // e1000网卡驱动，在开启TSO功能时，存在IPv4的totalLength为0
                // e1000网卡驱动：https://elixir.bootlin.com/linux/v3.0/source/drivers/net/e1000e/netdev.c#L4423
                if total_length == 0 {
                    total_length = size_checker as usize + HeaderType::Ipv4.min_header_size();
                }
                self.packet_len = total_length + HeaderType::Eth.min_packet_size() + vlan_tag_size;
                // 错包时取最小包长
                self.packet_len = self
                    .packet_len
                    .max(HeaderType::Ipv4.min_packet_size() + vlan_tag_size);

                let mut l3_opt_size = ihl as isize * 4 - 20;
                // wrong ihl
                if l3_opt_size < 0 {
                    l3_opt_size = 0;
                }
                size_checker -= l3_opt_size;
                if size_checker < 0 {
                    self.npb_ignore_l4 = true;
                    return Ok(());
                }
                self.l2_l3_opt_size = vlan_tag_size + l3_opt_size as usize;
                self.l3_payload_len = self.packet_len - (packet.len() - size_checker as usize);

                ip_protocol = IpProtocol::try_from(packet[IPV4_PROTO_OFFSET + vlan_tag_size])
                    .map_err(|e| {
                        error::Error::ParsePacketFailed(format!("parse ip_protocol failed: {}", e))
                    })?;
                self.lookup_key.proto = ip_protocol;

                if read_u16_be(&packet[FIELD_OFFSET_FRAG + vlan_tag_size..]) & 0xFFF != 0 {
                    // fragment
                    self.header_type = HeaderType::Ipv4;
                    self.npb_ignore_l4 = true;
                    self.l4_payload_len = self.l3_payload_len;
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }

        let packet = self.raw.as_ref().unwrap();
        match ip_protocol {
            IpProtocol::Icmpv4 => {
                // 错包时取最小包长
                self.packet_len = self
                    .packet_len
                    .max(HeaderType::Ipv4Icmp.min_packet_size() + self.l2_l3_opt_size);
                size_checker -= HeaderType::Ipv4Icmp.min_header_size() as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                match IcmpType::new(packet[FIELD_OFFSET_ICMP_TYPE_CODE + self.l2_l3_opt_size]) {
                    IcmpTypes::DestinationUnreachable
                    | IcmpTypes::SourceQuench
                    | IcmpTypes::RedirectMessage
                    | IcmpTypes::ParameterProblem => {
                        self.l4_opt_size = FIELD_LEN_ICMP_REST;
                        size_checker -= self.l4_opt_size as isize;
                        if size_checker < 0 {
                            self.l4_opt_size = 0;
                            return Ok(());
                        }
                    }
                    _ => (),
                }
                self.payload_len =
                    (self.packet_len - (packet.len() - size_checker as usize)) as u16;
                self.header_type = HeaderType::Ipv4Icmp;
                return Ok(());
            }
            IpProtocol::Udp => {
                match eth_type {
                    EthernetType::Ipv4 => {
                        self.packet_len = self
                            .packet_len
                            .max(HeaderType::Ipv4Udp.min_packet_size() + self.l2_l3_opt_size)
                    }
                    EthernetType::Ipv6 => {
                        self.packet_len = self
                            .packet_len
                            .max(HeaderType::Ipv6Udp.min_packet_size() + self.l2_l3_opt_size)
                    }
                    _ => unreachable!(),
                }
                let header_type = if self.header_type == HeaderType::Ipv6 {
                    HeaderType::Ipv6Udp
                } else {
                    HeaderType::Ipv4Udp
                };
                size_checker -= header_type.min_header_size() as isize;
                if size_checker < 0 {
                    return Ok(());
                }
                self.l4_payload_len = self.packet_len - (packet.len() - size_checker as usize);
                self.payload_len = self.l4_payload_len as u16;
                self.header_type = header_type;
            }
            IpProtocol::Tcp => {
                match eth_type {
                    EthernetType::Ipv4 => {
                        self.packet_len = self
                            .packet_len
                            .max(HeaderType::Ipv4Tcp.min_packet_size() + self.l2_l3_opt_size)
                    }
                    EthernetType::Ipv6 => {
                        self.packet_len = self
                            .packet_len
                            .max(HeaderType::Ipv6Tcp.min_packet_size() + self.l2_l3_opt_size)
                    }
                    _ => unreachable!(),
                }
                let header_type = if self.header_type == HeaderType::Ipv6 {
                    HeaderType::Ipv6Tcp
                } else {
                    HeaderType::Ipv4Tcp
                };
                size_checker -= header_type.min_header_size() as isize;
                if size_checker < 0 {
                    self.npb_ignore_l4 = true;
                    return Ok(());
                }
                let data_offset = packet[FIELD_OFFSET_TCP_DATAOFF + self.l2_l3_opt_size] >> 4;
                self.data_offset_ihl_or_fl4b |= data_offset << 4;
                let mut l4_opt_size = data_offset as isize * 4 - 20;
                if l4_opt_size < 0 {
                    // dataOffset可能为一个错误的值
                    l4_opt_size = 0;
                }
                self.l4_opt_size = l4_opt_size as usize;
                size_checker -= l4_opt_size;
                if size_checker < 0 {
                    self.npb_ignore_l4 = true;
                    return Ok(());
                }
                self.l4_payload_len = self.packet_len - (packet.len() - size_checker as usize);
                self.payload_len = self.l4_payload_len as u16;
                self.header_type = header_type;
                self.tcp_data.data_offset = data_offset;
                self.tcp_data.win_size =
                    read_u16_be(&packet[FIELD_OFFSET_TCP_WIN + self.l2_l3_opt_size..]);
                self.tcp_data.flags = TcpFlags::from_bits_truncate(
                    packet[FIELD_OFFSET_TCP_FLAG + self.l2_l3_opt_size],
                );
                self.tcp_data.seq =
                    read_u32_be(&packet[FIELD_OFFSET_TCP_SEQ + self.l2_l3_opt_size..]);
                self.tcp_data.ack =
                    read_u32_be(&packet[FIELD_OFFSET_TCP_ACK + self.l2_l3_opt_size..]);
                if data_offset > 5 {
                    self.update_tcp_opt();
                }
            }
            IpProtocol::Icmpv6 => {
                if size_checker > 0 {
                    // ICMPV6_TYPE_OFFSET使用ipv6的头长，实际ipv6比ipv4多的已经加在l3optSize中，这里再去掉
                    self.nd_reply_or_arp_request = Icmpv6Type::new(
                        packet[ICMPV6_TYPE_OFFSET + self.l2_l3_opt_size - IPV6_HEADER_ADJUST],
                    ) == Icmpv6Types::NeighborAdvert;
                    // 忽略link-local address并只考虑ND reply, i.e. neighbour advertisement
                    if let IpAddr::V6(ip) = self.lookup_key.src_ip {
                        self.nd_reply_or_arp_request =
                            self.nd_reply_or_arp_request && !is_unicast_link_local(&ip);
                    } else {
                        unreachable!()
                    }
                }
                self.payload_len =
                    (self.packet_len - (packet.len() - size_checker as usize)) as u16;
                return Ok(());
            }
            _ => {
                self.payload_len =
                    (self.packet_len - (packet.len() - size_checker as usize)) as u16;
                return Ok(());
            }
        }
        let packet = self.raw.as_ref().unwrap();
        if self.header_type >= HeaderType::Ipv4 {
            self.offset_port_0 += self.l2_l3_opt_size;
            self.offset_port_1 += self.l2_l3_opt_size;
            self.lookup_key.src_port = read_u16_be(&packet[self.offset_port_0..]);
            self.lookup_key.dst_port = read_u16_be(&packet[self.offset_port_1..]);
            if dst_endpoint {
                // inbound
                mem::swap(&mut self.offset_port_0, &mut self.offset_port_1);
            }
        }
        const PACKET_MAX_PADDING: usize = 16;
        if self.packet_len + PACKET_MAX_PADDING < original_length {
            // 因为采集包是有padding的, 正常场景PacketLen根据ip.total_len计算出准确的值
            // 在有些场景采集包会被截断，或者由于tso等功能多个报文会合并为一个，但是采集
            // 到的ip.total_len远远小于实际包长，考虑到其中的tcp.seq和tcp.ack可能未改变
            // 的，m.PacketLen在最后使用originalLength校准，但不会修改PayloadLen，不影响
            // RTT计算。
            self.packet_len = original_length;
        }
        Ok(())
    }

    /// Get the meta packet's l3 payload len.
    pub fn l3_payload_len(&self) -> usize {
        self.l3_payload_len
    }

    /// Get the meta packet's l4 payload len.
    pub fn l4_payload_len(&self) -> usize {
        self.l4_payload_len
    }

    #[cfg(target_os = "linux")]
    pub unsafe fn from_ebpf(
        data: *mut SK_BPF_DATA,
        capture_size: usize,
    ) -> Result<MetaPacket<'a>, Box<dyn Error>> {
        let data = &mut (*data);
        let (local_ip, remote_ip) = if data.tuple.addr_len == 4 {
            (
                {
                    let addr: [u8; 4] = data.tuple.laddr[..4].try_into()?;
                    IpAddr::from(Ipv4Addr::from(addr))
                },
                {
                    let addr: [u8; 4] = data.tuple.raddr[..4].try_into()?;
                    IpAddr::from(Ipv4Addr::from(addr))
                },
            )
        } else {
            (
                IpAddr::from(Ipv6Addr::from(data.tuple.laddr)),
                IpAddr::from(Ipv6Addr::from(data.tuple.raddr)),
            )
        };

        let (src_ip, dst_ip, src_port, dst_port) = if data.direction == SOCK_DIR_SND {
            (local_ip, remote_ip, data.tuple.lport, data.tuple.rport)
        } else {
            (remote_ip, local_ip, data.tuple.rport, data.tuple.lport)
        };

        let mut packet = MetaPacket::default();

        packet.lookup_key = LookupKey {
            timestamp: Duration::from_micros(data.timestamp),
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            eth_type: if data.tuple.addr_len == 4 {
                EthernetType::Ipv4
            } else {
                EthernetType::Ipv6
            },
            l2_end_0: data.direction == SOCK_DIR_SND,
            l2_end_1: data.direction == SOCK_DIR_RCV,
            proto: IpProtocol::try_from(data.tuple.protocol)?,
            tap_type: TapType::Tor,
            ..Default::default()
        };

        let cap_len = capture_size.min(data.cap_len as usize);

        packet.raw_from_ebpf = vec![0u8; cap_len as usize];
        data.cap_data
            .copy_to_nonoverlapping(packet.raw_from_ebpf.as_mut_ptr() as *mut i8, cap_len);
        packet.packet_len = data.syscall_len as usize + 54; // 目前仅支持TCP
        packet.payload_len = data.cap_len as u16;
        packet.l4_payload_len = data.cap_len as usize;
        packet.tap_port = TapPort::from_ebpf(data.process_id);
        packet.cap_seq = data.cap_seq;
        packet.process_id = data.process_id;
        packet.thread_id = data.thread_id;
        packet.syscall_trace_id = data.syscall_trace_id_call;
        packet.process_name = CStr::from_ptr(data.process_name.as_ptr() as *const i8)
            .to_str()?
            .to_string();
        packet.socket_id = data.socket_id;
        packet.tcp_data.seq = data.tcp_seq as u32;
        packet.l7_protocol_from_ebpf = L7Protocol::from(data.l7_protocal_hint as u8);
        packet.direction = PacketDirection::ClientToServer;
        return Ok(packet);
    }

    pub fn ebpf_flow_id(&self) -> u128 {
        let protocol = u8::from(self.l7_protocol_from_ebpf) as u128;

        (self.socket_id as u128) | protocol << u64::BITS
    }

    pub fn set_loopback_mac(&mut self, mac: MacAddr) {
        if self.lookup_key.src_ip.is_loopback() {
            self.lookup_key.src_mac = mac;
        }
        if self.lookup_key.dst_ip.is_loopback() {
            self.lookup_key.dst_mac = mac;
        }
    }
}

impl<'a> fmt::Display for MetaPacket<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\t\t{}\n", self.lookup_key)?;
        write!(f, "\t\tsource_ip: {}, packet_count: {}, packet_bytes: {}, tap_port: {}, packet_len: {}, payload_len: {}, vlan: {}, direction: {:?}\n",
            Ipv4Addr::from(self.source_ip), self.packet_count, self.packet_bytes, self.tap_port, self.packet_len, self.payload_len, self.vlan, self.direction
            )?;
        if let Some(t) = &self.tunnel {
            write!(f, "\t\ttunnel: {}\n", t)?;
        }
        if self.lookup_key.proto == IpProtocol::Tcp {
            write!(f, "\t\ttcp: {:?}\n", self.tcp_data)?;
        }
        if let Some(r) = &self.raw {
            if r.len() > 0 {
                let print_bytes = 64.min(r.len());
                write!(f, "\t\t raw_len: {}, raw: ", r.len())?;
                for b in &r[..print_bytes] {
                    write!(f, "{:02x}", b)?;
                }
                write!(f, "\n")?;
            }
        }
        write!(f, "")
    }
}

#[derive(Debug, Default)]
pub struct MetaPacketTcpHeader {
    pub seq: u32,
    pub ack: u32,
    pub win_size: u16,
    pub mss: u16,
    pub flags: TcpFlags,
    pub data_offset: u8,
    pub win_scale: u8,
    pub sack_permitted: bool,
    pub sack: Option<Vec<u8>>, // sack value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_pkt_size() {
        let pkt = MetaPacket {
            packet_len: 65530,
            ..Default::default()
        };
        assert_eq!(
            pkt.get_pkt_size(),
            65530,
            "packet size incorrect for\n{}",
            pkt
        );
        let pkt = MetaPacket {
            packet_len: 131072,
            ..Default::default()
        };
        assert_eq!(
            pkt.get_pkt_size(),
            65535,
            "packet size incorrect for\n{}",
            pkt
        );
    }
}
