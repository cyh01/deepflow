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

use serde::{Serialize, Serializer};

use std::{fmt, str};

use super::super::{
    value_is_default, AppProtoHead, AppProtoLogsInfo, L7LogParse, L7Protocol, L7ResponseStatus,
    LogMessageType,
};

use crate::common::enums::{IpProtocol, PacketDirection};
use crate::common::meta_packet::MetaPacket;
use crate::flow_generator::error::{Error, Result};
use crate::flow_generator::{AppProtoHeadEnum, AppProtoLogsInfoEnum};
use crate::proto::flow_log;

const SEPARATOR_SIZE: usize = 2;

#[derive(Serialize, Debug, Default, Clone)]
pub struct RedisInfo {
    #[serde(
        rename = "request_resource",
        skip_serializing_if = "value_is_default",
        serialize_with = "vec_u8_to_string"
    )]
    pub request: Vec<u8>, // 命令字段包括参数例如："set key value"
    #[serde(
        skip_serializing_if = "value_is_default",
        serialize_with = "vec_u8_to_string"
    )]
    pub request_type: Vec<u8>, // 命令类型不包括参数例如：命令为"set key value"，命令类型为："set"
    #[serde(
        rename = "response_result",
        skip_serializing_if = "value_is_default",
        serialize_with = "vec_u8_to_string"
    )]
    pub response: Vec<u8>, // 整数回复 + 批量回复 + 多条批量回复
    #[serde(skip)]
    pub status: Vec<u8>, // '+'
    #[serde(
        rename = "response_expection",
        skip_serializing_if = "value_is_default",
        serialize_with = "vec_u8_to_string"
    )]
    pub error: Vec<u8>, // '-'
}

pub fn vec_u8_to_string<S>(v: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&String::from_utf8_lossy(v))
}

impl RedisInfo {
    pub fn merge(&mut self, other: Self) {
        self.response = other.response;
        self.status = other.status;
        self.error = other.error;
    }
}

impl fmt::Display for RedisInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RedisInfo {{ request: {:?}, ",
            str::from_utf8(&self.request).unwrap_or_default()
        )?;
        write!(
            f,
            "request_type: {:?}, ",
            str::from_utf8(&self.request_type).unwrap_or_default()
        )?;
        write!(
            f,
            "response: {:?}, ",
            str::from_utf8(&self.response).unwrap_or_default()
        )?;
        write!(
            f,
            "status: {:?}, ",
            str::from_utf8(&self.status).unwrap_or_default()
        )?;
        write!(
            f,
            "error: {:?} }}",
            str::from_utf8(&self.error).unwrap_or_default()
        )
    }
}

impl From<RedisInfo> for flow_log::RedisInfo {
    fn from(f: RedisInfo) -> Self {
        flow_log::RedisInfo {
            request: f.request,
            request_type: f.request_type,
            response: f.response,
            status: f.status,
            error: f.error,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RedisLog {
    info: RedisInfo,
    l7_proto: L7Protocol,
    msg_type: LogMessageType,
    status: L7ResponseStatus,
}

impl RedisLog {
    fn reset(&mut self) {
        *self = RedisLog::default();
    }

    fn fill_request(&mut self, context: Vec<u8>) {
        self.info.request_type = match (&context).iter().position(|&x| x == b' ') {
            Some(i) if i > 0 => Vec::from(&context[..i]),
            _ => context.clone(),
        };
        self.msg_type = LogMessageType::Request;
        self.info.request = context;
    }

    fn fill_response(&mut self, context: Vec<u8>, error_response: bool) {
        self.msg_type = LogMessageType::Response;
        if context.is_empty() {
            return;
        }

        self.status = L7ResponseStatus::Ok;
        match context[0] {
            b'+' => self.info.status = context,
            b'-' if error_response => {
                self.info.error = context;
                self.status = L7ResponseStatus::ServerError;
            }
            b'-' if !error_response => self.info.response = context,
            _ => self.info.response = context,
        }
    }
}

impl L7LogParse for RedisLog {
    fn parse(
        &mut self,
        payload: &[u8],
        proto: IpProtocol,
        direction: PacketDirection,
    ) -> Result<AppProtoHeadEnum> {
        if proto != IpProtocol::Tcp {
            return Err(Error::InvalidIpProtocol);
        }

        self.reset();
        let (context, _, error_response) =
            decode(payload, direction == PacketDirection::ClientToServer)
                .ok_or(Error::RedisLogParseFailed)?;
        match direction {
            PacketDirection::ClientToServer => self.fill_request(context),
            PacketDirection::ServerToClient => self.fill_response(context, error_response),
        };
        Ok(AppProtoHeadEnum::Single(AppProtoHead {
            proto: L7Protocol::Redis,
            msg_type: self.msg_type,
            status: self.status,
            code: 0,
            rrt: 0,
            version: 0,
        }))
    }

    fn info(&self) -> AppProtoLogsInfoEnum {
        AppProtoLogsInfoEnum::Single(AppProtoLogsInfo::Redis(self.info.clone()))
    }
}

// 协议解析：http://redisdoc.com/topic/protocol.html#
fn find_separator(payload: &[u8]) -> Option<usize> {
    let len = payload.len();
    if len < 2 {
        return None;
    }

    for i in 0..len - 1 {
        if payload[i] == b'\r' && payload[i + 1] == b'\n' {
            return Some(i);
        }
    }
    None
}

fn decode_integer(payload: &[u8]) -> Option<(isize, usize)> {
    let separator_pos = find_separator(payload)?;
    // 整数至少占一位
    if separator_pos < 1 {
        return None;
    }

    let integer = str::from_utf8(&payload[..separator_pos])
        .unwrap_or_default()
        .parse::<isize>()
        .ok()?;

    Some((integer, separator_pos + SEPARATOR_SIZE))
}

// 格式为"$3\r\nSET\r\n"
fn decode_dollor(payload: &[u8], strict: bool) -> Option<(&[u8], usize)> {
    let mut offset = 1; // 开头的$
    let (next_data_len, sub_offset) = decode_integer(&payload[offset..])?;

    // $-1 $0时返回
    if next_data_len <= 0 {
        return Some((
            &payload[offset..offset + sub_offset - SEPARATOR_SIZE],
            offset + sub_offset,
        ));
    }

    offset += sub_offset;
    let next_data_len = next_data_len as usize;

    if offset + next_data_len + SEPARATOR_SIZE > payload.len()
        || payload[offset + next_data_len] != b'\r'
        || payload[offset + next_data_len + 1] != b'\n'
    {
        if strict {
            return None;
        }
        // 返回所有内容
        return Some((&payload[offset..], payload.len()));
    }

    // 完全合法
    Some((
        &payload[offset..offset + next_data_len],
        offset + next_data_len + 2,
    ))
}

// 命令为"set mykey myvalue"，实际封装为"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n"
fn decode_asterisk(payload: &[u8], strict: bool) -> Option<(Vec<u8>, usize)> {
    let mut offset = 1; // 开头的 *

    // 提取请求参数个数/批量回复个数
    let (next_data_num, sub_offset) = decode_integer(&payload[offset..])?;

    if next_data_num <= 0 {
        // 无内容的多条批量回复: "*-1\r\n"
        // 空白内容的多条批量回复: "*0\r\n"
        return Some((
            payload[offset..offset + sub_offset - SEPARATOR_SIZE].to_vec(),
            offset + sub_offset,
        ));
    }
    offset += sub_offset;

    let mut ret_vec = Vec::new();
    let len = payload.len();

    for _ in 0..next_data_num {
        if let Some((sub_vec, sub_offset, _)) = decode(&payload[offset..], strict) {
            if sub_offset == 0 {
                if strict {
                    return None;
                }
                return Some((ret_vec, offset));
            }

            if !ret_vec.is_empty() {
                ret_vec.push(b' ');
            }
            ret_vec.extend_from_slice(sub_vec.as_slice());

            offset += sub_offset;
            if offset >= len {
                return Some((ret_vec, len));
            }
        }
    }
    Some((ret_vec, offset))
}

fn decode_str(payload: &[u8], limit: usize) -> Option<(&[u8], usize)> {
    let len = payload.len();
    let separator_pos = find_separator(payload).unwrap_or(len);

    if separator_pos > limit {
        return Some((
            // 截取数据后，并不会在末尾增加'...'提示
            &payload[..limit],
            limit,
        ));
    }

    Some((&payload[..separator_pos], separator_pos))
}

// 函数在入参为"$-1"或"-1"时都返回"-1", 使用第三个参数区分是否为错误回复
pub fn decode(payload: &[u8], strict: bool) -> Option<(Vec<u8>, usize, bool)> {
    if payload.len() < SEPARATOR_SIZE {
        return None;
    }

    match payload[0] {
        // 请求或多条批量回复
        b'*' => decode_asterisk(payload, strict).map(|(v, s)| (v, s, false)),
        // 状态回复,整数回复
        b'+' | b':' => decode_str(payload, 32).map(|(v, s)| (v.to_vec(), s, false)),
        // 错误回复
        b'-' => decode_str(payload, 256).map(|(v, s)| (v.to_vec(), s, true)),
        // 批量回复
        b'$' => decode_dollor(payload, strict).map(|(v, s)| (v.to_vec(), s, false)),
        _ => None,
    }
}

pub fn decode_error_code(context: &[u8]) -> Option<&[u8]> {
    for (i, ch) in context.iter().enumerate() {
        if *ch == b' ' || *ch == b'\n' {
            return Some(&context[..i]);
        }
    }
    None
}

// 通过请求识别REDIS
pub fn redis_check_protocol(bitmap: &mut u128, packet: &MetaPacket) -> bool {
    if packet.lookup_key.proto != IpProtocol::Tcp {
        *bitmap &= !(1 << u8::from(L7Protocol::Redis));
        return false;
    }

    let payload = packet.get_l4_payload();
    if payload.is_none() {
        return false;
    }
    let payload = payload.unwrap();

    if payload[0] != b'*' {
        return false;
    }
    return decode_asterisk(payload, true).is_some();
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    use crate::{common::enums::PacketDirection, utils::test::Capture};

    const FILE_DIR: &str = "resources/test/flow_generator/redis";

    fn run(name: &str) -> String {
        let pcap_file = Path::new(FILE_DIR).join(name);
        let capture = Capture::load_pcap(pcap_file, None);
        let mut packets = capture.as_meta_packets();
        if packets.is_empty() {
            return "".to_string();
        }

        let mut output: String = String::new();
        let first_dst_port = packets[0].lookup_key.dst_port;
        let mut bitmap = 0;
        for packet in packets.iter_mut() {
            packet.direction = if packet.lookup_key.dst_port == first_dst_port {
                PacketDirection::ClientToServer
            } else {
                PacketDirection::ServerToClient
            };
            let payload = match packet.get_l4_payload() {
                Some(p) => p,
                None => continue,
            };

            let mut redis = RedisLog::default();
            let _ = redis.parse(payload, packet.lookup_key.proto, packet.direction);
            let is_redis = redis_check_protocol(&mut bitmap, packet);
            output.push_str(&format!("{} is_redis: {}\r\n", redis.info, is_redis));
        }
        output
    }

    #[test]
    fn check() {
        let files = vec![
            ("redis.pcap", "redis.result"),
            ("redis-error.pcap", "redis-error.result"),
            ("redis-debug.pcap", "redis-debug.result"),
        ];

        for item in files.iter() {
            let expected = fs::read_to_string(&Path::new(FILE_DIR).join(item.1)).unwrap();
            let output = run(item.0);

            if output != expected {
                let output_path = Path::new("actual.txt");
                fs::write(&output_path, &output).unwrap();
                assert!(
                    output == expected,
                    "output different from expected {}, written to {:?}",
                    item.1,
                    output_path
                );
            }
        }
    }

    #[test]
    fn test_decode() {
        let payload = [b'*', b'-', b'1', b'\r', b'\n'];
        let (context, n, e) = decode(payload.as_slice(), true).unwrap();
        assert_eq!(context, "-1".as_bytes());
        assert_eq!(n, payload.len());
        assert_eq!(e, false);

        let payload = [
            b'*', b'3', b'\r', b'\n', b'$', b'3', b'\r', b'\n', b'S', b'E', b'T', b'\r', b'\n',
            b'$', b'5', b'\r', b'\n', b'm', b'y', b'k', b'e', b'y', b'\r', b'\n', b'$', b'7',
            b'\r', b'\n', b'm', b'y', b'v', b'a', b'l', b'u', b'e', b'\r', b'\n',
        ];

        let (context, n, e) = decode(payload.as_slice(), true).unwrap();
        assert_eq!(context, "SET mykey myvalue".as_bytes());
        assert_eq!(n, payload.len());
        assert_eq!(e, false);

        let payload = [b'$', b'0', b'\r', b'\n'];
        let (context, n, _) = decode(payload.as_slice(), true).unwrap();
        assert_eq!(context, "0".as_bytes());
        assert_eq!(n, payload.len());

        let payload = [b'$', b'-', b'1', b'\r', b'\n'];
        let (context, n, e) = decode(payload.as_slice(), false).unwrap();
        assert_eq!(context, "-1".as_bytes());
        assert_eq!(n, payload.len());
        assert_eq!(e, false);

        let payload = [b'$', b'9', b'\r', b'\n', b'1', b'2', b'3', b'4', b'5'];
        let (context, n, _) = decode(payload.as_slice(), false).unwrap();
        assert_eq!(context, "12345".as_bytes());
        assert_eq!(n, payload.len());

        let payload = [b'$', b'9', b'\r', b'\n', b'1', b'2', b'3', b'4', b'5'];
        assert_eq!(decode(payload.as_slice(), true), None);

        let payload = [b'-', b'1', b'\r', b'\n'];
        let (context, n, e) = decode(payload.as_slice(), true).unwrap();
        assert_eq!(context, "-1".as_bytes());
        assert_eq!(n, 2);
        assert_eq!(e, true);
    }
}
