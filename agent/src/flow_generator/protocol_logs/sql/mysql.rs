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

use serde::Serialize;

use super::super::{
    consts::*, value_is_default, AppProtoHead, AppProtoLogsData, AppProtoLogsInfo, L7LogParse,
    L7Protocol, L7ResponseStatus, LogMessageType,
};

use crate::flow_generator::{AppProtoHeadEnum, AppProtoLogsInfoEnum};
use crate::proto::flow_log;
use crate::{
    common::enums::{IpProtocol, PacketDirection},
    common::meta_packet::MetaPacket,
    flow_generator::error::{Error, Result},
    utils::bytes,
};

#[derive(Serialize, Debug, Default, Clone)]
pub struct MysqlInfo {
    // Server Greeting
    #[serde(rename = "version", skip_serializing_if = "value_is_default")]
    pub protocol_version: u8,
    #[serde(skip)]
    pub server_version: String,
    #[serde(skip)]
    pub server_thread_id: u32,
    // request
    #[serde(rename = "request_type")]
    pub command: u8,
    #[serde(rename = "request_resource", skip_serializing_if = "value_is_default")]
    pub context: String,
    // response
    #[serde(skip)]
    pub response_code: u8,
    #[serde(skip)]
    pub error_code: u16,
    #[serde(rename = "sql_affected_rows", skip_serializing_if = "value_is_default")]
    pub affected_rows: u64,
    #[serde(
        rename = "response_execption",
        skip_serializing_if = "value_is_default"
    )]
    pub error_message: String,
}

impl MysqlInfo {
    pub fn merge(&mut self, other: Self) {
        self.response_code = other.response_code;
        self.affected_rows = other.affected_rows;
        self.error_code = other.error_code;
        self.error_message = other.error_message;
    }
}

impl From<MysqlInfo> for flow_log::MysqlInfo {
    fn from(f: MysqlInfo) -> Self {
        flow_log::MysqlInfo {
            protocol_version: f.protocol_version as u32,
            server_version: f.server_version,
            server_thread_id: f.server_thread_id,
            command: f.command as u32,
            context: f.context,
            response_code: f.response_code as u32,
            affected_rows: f.affected_rows,
            error_code: f.error_code as u32,
            error_message: f.error_message,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MysqlLog {
    info: MysqlInfo,

    l7_proto: L7Protocol,
    msg_type: LogMessageType,
    status: L7ResponseStatus,
}

fn mysql_string(payload: &[u8]) -> String {
    if payload.len() > 2 && payload[0] == 0 && payload[1] == 1 {
        // MYSQL 8.0.26返回字符串前有0x0、0x1，MYSQL 8.0.21版本没有这个问题
        // https://gitlab.yunshan.net/platform/trident/-/merge_requests/2592#note_401425
        String::from_utf8_lossy(&payload[2..]).into_owned()
    } else {
        String::from_utf8_lossy(payload).into_owned()
    }
}

impl MysqlLog {
    fn request_string(&mut self, payload: &[u8]) {
        self.info.context = mysql_string(payload);
    }

    fn reset_logs(&mut self) {
        self.info = MysqlInfo::default();
        self.status = L7ResponseStatus::Ok;
    }

    fn get_log_data_special_info(self, log_data: &mut AppProtoLogsData) {
        if (&self).msg_type == LogMessageType::Response
            && (&self).info.response_code == MYSQL_RESPONSE_CODE_ERR
        {
            log_data.base_info.head.code = (&self).info.error_code;
        }
        log_data.special_info = AppProtoLogsInfo::Mysql(self.info);
    }

    fn greeting(&mut self, payload: &[u8]) -> Result<()> {
        let mut remain = payload.len();
        if remain < PROTOCOL_VERSION_LEN {
            return Err(Error::MysqlLogParseFailed);
        }
        self.info.protocol_version = payload[PROTOCOL_VERSION_OFFSET];
        remain -= PROTOCOL_VERSION_LEN;
        let server_version_pos = payload[SERVER_VERSION_OFFSET..]
            .iter()
            .position(|&x| x == SERVER_VERSION_EOF)
            .unwrap_or_default();
        if server_version_pos <= 0 {
            return Err(Error::MysqlLogParseFailed);
        }
        self.info.server_version = String::from_utf8_lossy(
            &payload[SERVER_VERSION_OFFSET..SERVER_VERSION_OFFSET + server_version_pos],
        )
        .into_owned();
        remain -= server_version_pos as usize;
        if remain < THREAD_ID_LEN {
            return Err(Error::MysqlLogParseFailed);
        }
        let thread_id_offset = THREAD_ID_OFFSET_B + server_version_pos + 1;
        self.info.server_thread_id = bytes::read_u32_le(&payload[thread_id_offset..]);
        self.l7_proto = L7Protocol::Mysql;
        Ok(())
    }

    fn request(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() < COMMAND_LEN {
            return Err(Error::MysqlLogParseFailed);
        }
        self.info.command = payload[COMMAND_OFFSET];
        match self.info.command {
            COM_QUIT | COM_FIELD_LIST | COM_STMT_EXECUTE | COM_STMT_CLOSE | COM_STMT_FETCH => (),
            COM_INIT_DB | COM_QUERY | COM_STMT_PREPARE => {
                self.request_string(&payload[COMMAND_OFFSET + COMMAND_LEN..]);
            }
            _ => return Err(Error::MysqlLogParseFailed),
        }
        self.l7_proto = L7Protocol::Mysql;
        Ok(())
    }

    fn decode_compress_int(payload: &[u8]) -> u64 {
        let remain = payload.len();
        if remain == 0 {
            return 0;
        }
        let value = payload[0];
        match value {
            INT_FLAGS_2 if remain > INT_BASE_LEN + 2 => {
                bytes::read_u16_le(&payload[INT_BASE_LEN..]) as u64
            }
            INT_FLAGS_3 if remain > INT_BASE_LEN + 3 => {
                bytes::read_u16_le(&payload[INT_BASE_LEN..]) as u64
                    | ((payload[INT_BASE_LEN + 2] as u64) << 16)
            }
            INT_FLAGS_8 if remain > INT_BASE_LEN + 8 => {
                bytes::read_u64_le(&payload[INT_BASE_LEN..])
            }
            _ => value as u64,
        }
    }

    fn set_status(&mut self, status_code: u16) {
        if status_code != 0 {
            if status_code >= 2000 && status_code <= 2999 {
                self.status = L7ResponseStatus::ClientError;
            } else {
                self.status = L7ResponseStatus::ServerError;
            }
        } else {
            self.status = L7ResponseStatus::Ok;
        }
    }

    fn response(&mut self, payload: &[u8]) -> Result<()> {
        let mut remain = payload.len();
        if remain < RESPONSE_CODE_LEN {
            return Err(Error::MysqlLogParseFailed);
        }
        self.info.response_code = payload[RESPONSE_CODE_OFFSET];
        remain -= RESPONSE_CODE_LEN;
        match self.info.response_code {
            MYSQL_RESPONSE_CODE_ERR => {
                if remain > ERROR_CODE_LEN {
                    self.info.error_code = bytes::read_u16_le(&payload[ERROR_CODE_OFFSET..]);
                    remain -= ERROR_CODE_LEN;
                }
                self.set_status(self.info.error_code);
                let error_message_offset =
                    if remain > SQL_STATE_LEN && payload[SQL_STATE_OFFSET] == SQL_STATE_MARKER {
                        SQL_STATE_OFFSET + SQL_STATE_LEN
                    } else {
                        SQL_STATE_OFFSET
                    };
                self.info.error_message =
                    String::from_utf8_lossy(&payload[error_message_offset..]).into_owned();
            }
            MYSQL_RESPONSE_CODE_OK => {
                self.status = L7ResponseStatus::Ok;
                self.info.affected_rows =
                    MysqlLog::decode_compress_int(&payload[AFFECTED_ROWS_OFFSET..]);
            }
            _ => (),
        }
        Ok(())
    }
}

impl L7LogParse for MysqlLog {
    fn parse(
        &mut self,
        payload: &[u8],
        proto: IpProtocol,
        direction: PacketDirection,
    ) -> Result<AppProtoHeadEnum> {
        if proto != IpProtocol::Tcp {
            return Err(Error::InvalidIpProtocol);
        }
        self.reset_logs();

        let mut header = MysqlHeader::default();
        let offset = header.decode(payload);
        if offset < 0 {
            return Err(Error::MysqlLogParseFailed);
        }
        let offset = offset as usize;
        let msg_type = header
            .check(direction, offset, payload, self.l7_proto)
            .ok_or(Error::MysqlLogParseFailed)?;

        match msg_type {
            LogMessageType::Request => self.request(&payload[offset..])?,
            LogMessageType::Response => self.response(&payload[offset..])?,
            LogMessageType::Other => self.greeting(&payload[offset..])?,
            _ => return Err(Error::MysqlLogParseFailed),
        };
        self.msg_type = msg_type;

        Ok(AppProtoHeadEnum::Single(AppProtoHead {
            proto: L7Protocol::Mysql,
            msg_type,
            status: self.status,
            code: self.info.error_code,
            rrt: 0,
            version: 0,
        }))
    }

    fn info(&self) -> AppProtoLogsInfoEnum {
        AppProtoLogsInfoEnum::Single(AppProtoLogsInfo::Mysql(self.info.clone()))
    }
}

#[derive(Debug, Default)]
pub struct MysqlHeader {
    length: u32,
    number: u8,
}

impl MysqlHeader {
    pub fn decode(&mut self, payload: &[u8]) -> isize {
        if payload.len() < 5 {
            return -1;
        }
        let len = bytes::read_u32_le(payload) & 0xffffff;
        if payload[HEADER_LEN + RESPONSE_CODE_OFFSET] == MYSQL_RESPONSE_CODE_OK
            || payload[HEADER_LEN + RESPONSE_CODE_OFFSET] == MYSQL_RESPONSE_CODE_ERR
            || payload[HEADER_LEN + RESPONSE_CODE_OFFSET] == MYSQL_RESPONSE_CODE_EOF
            || payload[NUMBER_OFFSET] == 0
        {
            self.length = len;
            self.number = payload[NUMBER_OFFSET];
            return HEADER_LEN as isize;
        }
        let offset = len as usize + HEADER_LEN;
        if offset >= payload.len() {
            return 0;
        }
        let offset = offset as isize;
        offset + self.decode(&payload[offset as usize..])
    }

    pub fn check(
        &self,
        direction: PacketDirection,
        offset: usize,
        payload: &[u8],
        l7_proto: L7Protocol,
    ) -> Option<LogMessageType> {
        if offset >= payload.len() || self.length == 0 {
            return None;
        }
        if self.number != 0 && l7_proto == L7Protocol::Unknown {
            return None;
        }

        match direction {
            PacketDirection::ServerToClient if self.number == 0 => {
                let payload = &payload[offset..];
                if payload.len() < PROTOCOL_VERSION_LEN {
                    return None;
                }
                let protocol_version = payload[PROTOCOL_VERSION_OFFSET];
                let index = payload[SERVER_VERSION_OFFSET..]
                    .iter()
                    .position(|&x| x == SERVER_VERSION_EOF)?;
                if index != 0 && protocol_version == PROTOCOL_VERSION {
                    Some(LogMessageType::Other)
                } else {
                    None
                }
            }
            PacketDirection::ServerToClient => Some(LogMessageType::Response),
            PacketDirection::ClientToServer if self.number == 0 => Some(LogMessageType::Request),
            _ => None,
        }
    }
}

// 通过请求和Greeting来识别MYSQL
pub fn mysql_check_protocol(bitmap: &mut u128, packet: &MetaPacket) -> bool {
    if packet.lookup_key.proto != IpProtocol::Tcp {
        *bitmap &= !(1 << u8::from(L7Protocol::Mysql));
        return false;
    }

    let payload = packet.get_l4_payload();
    if payload.is_none() {
        return false;
    }
    let payload = payload.unwrap();

    let mut header = MysqlHeader::default();
    let offset = header.decode(payload);
    if offset < 0 {
        *bitmap &= !(1 << u8::from(L7Protocol::Mysql));
        return false;
    }
    let offset = offset as usize;

    if header.number != 0 || offset + header.length as usize > payload.len() {
        return false;
    }

    let protocol_version_or_query_type = payload[offset];
    match protocol_version_or_query_type {
        COM_QUERY | COM_STMT_PREPARE => {
            let context = mysql_string(&payload[offset + 1..]);
            return context.is_ascii();
        }
        _ => {}
    }
    return false;
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    use crate::{common::enums::PacketDirection, utils::test::Capture};

    const FILE_DIR: &str = "resources/test/flow_generator/mysql";

    fn run(name: &str) -> String {
        let pcap_file = Path::new(FILE_DIR).join(name);
        let capture = Capture::load_pcap(pcap_file, Some(1400));
        let mut packets = capture.as_meta_packets();
        if packets.is_empty() {
            return "".to_string();
        }

        let mut mysql = MysqlLog::default();
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
            let _ = mysql.parse(payload, packet.lookup_key.proto, packet.direction);
            let is_mysql = mysql_check_protocol(&mut bitmap, packet);
            output.push_str(&format!("{:?} is_mysql: {}\r\n", mysql.info, is_mysql));
        }
        output
    }

    #[test]
    fn check() {
        let files = vec![
            ("mysql-statement.pcap", "mysql-statement.result"),
            ("mysql.pcap", "mysql.result"),
            ("mysql-error.pcap", "mysql-error.result"),
            ("mysql-table-desc.pcap", "mysql-table-desc.result"),
            ("mysql-table-insert.pcap", "mysql-table-insert.result"),
            ("mysql-table-delete.pcap", "mysql-table-delete.result"),
            ("mysql-table-update.pcap", "mysql-table-update.result"),
            ("mysql-table-select.pcap", "mysql-table-select.result"),
            ("mysql-table-create.pcap", "mysql-table-create.result"),
            ("mysql-table-destroy.pcap", "mysql-table-destroy.result"),
            ("mysql-table-alter.pcap", "mysql-table-alter.result"),
            ("mysql-database.pcap", "mysql-database.result"),
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
}
