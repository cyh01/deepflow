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

package jsonify

import (
	"fmt"
	"math"
	"net"
	"sync/atomic"
	"time"

	"github.com/google/gopacket/layers"

	"github.com/deepflowys/deepflow/message/trident"
	"github.com/deepflowys/deepflow/server/ingester/common"
	"github.com/deepflowys/deepflow/server/ingester/stream/geo"
	"github.com/deepflowys/deepflow/server/libs/ckdb"
	"github.com/deepflowys/deepflow/server/libs/datatype"
	"github.com/deepflowys/deepflow/server/libs/datatype/pb"
	"github.com/deepflowys/deepflow/server/libs/grpc"
	"github.com/deepflowys/deepflow/server/libs/pool"
	"github.com/deepflowys/deepflow/server/libs/zerodoc"
)

const (
	US_TO_S_DEVISOR = 1000000 // 微秒转化为秒的除数
)

type FlowLogger struct {
	pool.ReferenceCount
	_id uint64 // 用来标记全局(多节点)唯一的记录

	DataLinkLayer
	NetworkLayer
	TransportLayer
	ApplicationLayer
	Internet
	KnowledgeGraph
	FlowInfo
	Metrics
}

type DataLinkLayer struct {
	MAC0    uint64 `json:"mac_0"`
	MAC1    uint64 `json:"mac_1"`
	EthType uint16 `json:"eth_type"`
	VLAN    uint16 `json:"vlan,omitempty"`
}

var DataLinkLayerColumns = []*ckdb.Column{
	ckdb.NewColumn("mac_0", ckdb.UInt64),
	ckdb.NewColumn("mac_1", ckdb.UInt64),
	ckdb.NewColumn("eth_type", ckdb.UInt16).SetIndex(ckdb.IndexSet),
	ckdb.NewColumn("vlan", ckdb.UInt16).SetIndex(ckdb.IndexSet),
}

func (f *DataLinkLayer) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt64(f.MAC0); err != nil {
		return err
	}
	if err := block.WriteUInt64(f.MAC1); err != nil {
		return err
	}
	if err := block.WriteUInt16(f.EthType); err != nil {
		return err
	}
	if err := block.WriteUInt16(f.VLAN); err != nil {
		return err
	}

	return nil
}

type NetworkLayer struct {
	IP40         uint32 `json: "ip4_0"`
	IP41         uint32 `json: "ip4_1"`
	IP60         net.IP `json: "ip6_0"`
	IP61         net.IP `json: "ip6_1"`
	IsIPv4       bool   `json:"is_ipv4"`
	Protocol     uint8  `json:"protocol"`
	TunnelTier   uint8  `json:"tunnel_tier,omitempty"`
	TunnelType   uint16 `json:"tunnel_type,omitempty"`
	TunnelTxID   uint32 `json:"tunnel_tx_id,omitempty"`
	TunnelRxID   uint32 `json:"tunnel_rx_id,omitempty"`
	TunnelTxIP40 uint32 `json:"tunnel_tx_ip4_0,omitempty"`
	TunnelTxIP41 uint32 `json:"tunnel_tx_ip4_1,omitempty"`
	TunnelRxIP40 uint32 `json:"tunnel_rx_ip4_0,omitempty"`
	TunnelRxIP41 uint32 `json:"tunnel_rx_ip4_1,omitempty"`
	TunnelTxIP60 net.IP `json:"tunnel_tx_ip6_0,omitempty"`
	TunnelTxIP61 net.IP `json:"tunnel_tx_ip6_1,omitempty"`
	TunnelRxIP60 net.IP `json:"tunnel_rx_ip6_0,omitempty"`
	TunnelRxIP61 net.IP `json:"tunnel_rx_ip6_1,omitempty"`
	TunnelIsIPv4 bool   `json:"tunnel_is_ipv4"`
	TunnelTxMac0 uint32 `json:"tunnel_tx_mac_0,omitempty"`
	TunnelTxMac1 uint32 `json:"tunnel_tx_mac_1,omitempty"`
	TunnelRxMac0 uint32 `json:"tunnel_rx_mac_0,omitempty"`
	TunnelRxMac1 uint32 `json:"tunnel_rx_mac_1,omitempty"`
}

var NetworkLayerColumns = []*ckdb.Column{
	ckdb.NewColumn("ip4_0", ckdb.IPv4),
	ckdb.NewColumn("ip4_1", ckdb.IPv4),
	ckdb.NewColumn("ip6_0", ckdb.IPv6),
	ckdb.NewColumn("ip6_1", ckdb.IPv6),
	ckdb.NewColumn("is_ipv4", ckdb.UInt8).SetIndex(ckdb.IndexMinmax),
	ckdb.NewColumn("protocol", ckdb.UInt8),
	ckdb.NewColumn("tunnel_tier", ckdb.UInt8),
	ckdb.NewColumn("tunnel_type", ckdb.UInt16),
	ckdb.NewColumn("tunnel_tx_id", ckdb.UInt32),
	ckdb.NewColumn("tunnel_rx_id", ckdb.UInt32),
	ckdb.NewColumn("tunnel_tx_ip4_0", ckdb.IPv4),
	ckdb.NewColumn("tunnel_tx_ip4_1", ckdb.IPv4),
	ckdb.NewColumn("tunnel_rx_ip4_0", ckdb.IPv4),
	ckdb.NewColumn("tunnel_rx_ip4_1", ckdb.IPv4),
	ckdb.NewColumn("tunnel_tx_ip6_0", ckdb.IPv6),
	ckdb.NewColumn("tunnel_tx_ip6_1", ckdb.IPv6),
	ckdb.NewColumn("tunnel_rx_ip6_0", ckdb.IPv6),
	ckdb.NewColumn("tunnel_rx_ip6_1", ckdb.IPv6),
	ckdb.NewColumn("tunnel_is_ipv4", ckdb.UInt8).SetIndex(ckdb.IndexMinmax),
	ckdb.NewColumn("tunnel_tx_mac_0", ckdb.UInt32),
	ckdb.NewColumn("tunnel_tx_mac_1", ckdb.UInt32),
	ckdb.NewColumn("tunnel_rx_mac_0", ckdb.UInt32),
	ckdb.NewColumn("tunnel_rx_mac_1", ckdb.UInt32),
}

func (n *NetworkLayer) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteIPv4(n.IP40); err != nil {
		return err
	}
	if err := block.WriteIPv4(n.IP41); err != nil {
		return err
	}
	if len(n.IP60) == 0 {
		n.IP60 = net.IPv6zero
	}
	if err := block.WriteIPv6(n.IP60); err != nil {
		return err
	}
	if len(n.IP61) == 0 {
		n.IP61 = net.IPv6zero
	}
	if err := block.WriteIPv6(n.IP61); err != nil {
		return err
	}

	if err := block.WriteBool(n.IsIPv4); err != nil {
		return err
	}

	if err := block.WriteUInt8(n.Protocol); err != nil {
		return err
	}
	if err := block.WriteUInt8(n.TunnelTier); err != nil {
		return err
	}
	if err := block.WriteUInt16(n.TunnelType); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelTxID); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelRxID); err != nil {
		return err
	}
	if err := block.WriteIPv4(n.TunnelTxIP40); err != nil {
		return err
	}
	if err := block.WriteIPv4(n.TunnelTxIP41); err != nil {
		return err
	}
	if err := block.WriteIPv4(n.TunnelRxIP40); err != nil {
		return err
	}
	if err := block.WriteIPv4(n.TunnelRxIP41); err != nil {
		return err
	}
	if len(n.TunnelTxIP60) == 0 {
		n.TunnelTxIP60 = net.IPv6zero
	}
	if len(n.TunnelTxIP61) == 0 {
		n.TunnelTxIP61 = net.IPv6zero
	}
	if len(n.TunnelRxIP60) == 0 {
		n.TunnelRxIP60 = net.IPv6zero
	}
	if len(n.TunnelRxIP61) == 0 {
		n.TunnelRxIP61 = net.IPv6zero
	}
	if err := block.WriteIPv6(n.TunnelTxIP60); err != nil {
		return err
	}
	if err := block.WriteIPv6(n.TunnelTxIP61); err != nil {
		return err
	}
	if err := block.WriteIPv6(n.TunnelRxIP60); err != nil {
		return err
	}
	if err := block.WriteIPv6(n.TunnelRxIP61); err != nil {
		return err
	}
	if err := block.WriteBool(n.TunnelIsIPv4); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelTxMac0); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelTxMac1); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelRxMac0); err != nil {
		return err
	}
	if err := block.WriteUInt32(n.TunnelRxMac1); err != nil {
		return err
	}

	return nil
}

type TransportLayer struct {
	ClientPort       uint16 `json:"client_port"`
	ServerPort       uint16 `json:"server_port"`
	TCPFlagsBit0     uint16 `json:"tcp_flags_bit_0,omitempty"`
	TCPFlagsBit1     uint16 `json:"tcp_flags_bit_1,omitempty"`
	SynSeq           uint32 `json:"syn_seq"`
	SynAckSeq        uint32 `json:"syn_ack_seq"`
	LastKeepaliveSeq uint32 `json:"last_keepalive_seq"`
	LastKeepaliveAck uint32 `json:"last_keepalive_ack"`
}

var TransportLayerColumns = []*ckdb.Column{
	// 传输层
	ckdb.NewColumn("client_port", ckdb.UInt16).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("server_port", ckdb.UInt16).SetIndex(ckdb.IndexSet),
	ckdb.NewColumn("tcp_flags_bit_0", ckdb.UInt16).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("tcp_flags_bit_1", ckdb.UInt16).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("syn_seq", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("握手包的TCP SEQ序列号"),
	ckdb.NewColumn("syn_ack_seq", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("握手回应包的TCP SEQ序列号"),
	ckdb.NewColumn("last_keepalive_seq", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("last_keepalive_ack", ckdb.UInt32).SetIndex(ckdb.IndexNone),
}

func (t *TransportLayer) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt16(t.ClientPort); err != nil {
		return err
	}
	if err := block.WriteUInt16(t.ServerPort); err != nil {
		return err
	}
	if err := block.WriteUInt16(t.TCPFlagsBit0); err != nil {
		return err
	}
	if err := block.WriteUInt16(t.TCPFlagsBit1); err != nil {
		return err
	}
	if err := block.WriteUInt32(t.SynSeq); err != nil {
		return err
	}
	if err := block.WriteUInt32(t.SynAckSeq); err != nil {
		return err
	}
	if err := block.WriteUInt32(t.LastKeepaliveSeq); err != nil {
		return err
	}
	if err := block.WriteUInt32(t.LastKeepaliveAck); err != nil {
		return err
	}
	return nil
}

type ApplicationLayer struct {
	L7Protocol uint8 `json:"l7_protocol,omitempty"` // HTTP, DNS, others
}

var ApplicationLayerColumns = []*ckdb.Column{
	// 应用层
	ckdb.NewColumn("l7_protocol", ckdb.UInt8).SetIndex(ckdb.IndexMinmax),
}

func (a *ApplicationLayer) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt8(a.L7Protocol); err != nil {
		return err
	}
	return nil
}

type Internet struct {
	Province0 string `json:"province_0"`
	Province1 string `json:"province_1"`
}

var InternetColumns = []*ckdb.Column{
	// 广域网
	ckdb.NewColumn("province_0", ckdb.LowCardinalityString),
	ckdb.NewColumn("province_1", ckdb.LowCardinalityString),
}

func (i *Internet) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteString(i.Province0); err != nil {
		return err
	}
	if err := block.WriteString(i.Province1); err != nil {
		return err
	}

	return nil
}

type KnowledgeGraph struct {
	RegionID0     uint16 `json:"region_id_0"`
	RegionID1     uint16 `json:"region_id_1"`
	AZID0         uint16 `json:"az_id_0"`
	AZID1         uint16 `json:"az_id_1"`
	HostID0       uint16 `json:"host_id_0"`
	HostID1       uint16 `json:"host_id_1"`
	L3DeviceType0 uint8  `json:"l3_device_type_0"`
	L3DeviceType1 uint8  `json:"l3_device_type_1"`
	L3DeviceID0   uint32 `json:"l3_device_id_0"`
	L3DeviceID1   uint32 `json:"l3_device_id_1"`
	PodNodeID0    uint32 `json:"pod_node_id_0"`
	PodNodeID1    uint32 `json:"pod_node_id_1"`
	PodNSID0      uint16 `json:"pod_ns_id_0"`
	PodNSID1      uint16 `json:"pod_ns_id_1"`
	PodGroupID0   uint32 `json:"pod_group_id_0"`
	PodGroupID1   uint32 `json:"pod_group_id_1"`
	PodID0        uint32 `json:"pod_id_0"`
	PodID1        uint32 `json:"pod_id_1"`
	PodClusterID0 uint16 `json:"pod_cluster_id_0"`
	PodClusterID1 uint16 `json:"pod_cluster_id_1"`
	L3EpcID0      int32  `json:"l3_epc_id_0"`
	L3EpcID1      int32  `json:"l3_epc_id_1"`
	EpcID0        int32  `json:"epc_id_0"`
	EpcID1        int32  `json:"epc_id_1"`
	SubnetID0     uint16 `json:"subnet_id_0"`
	SubnetID1     uint16 `json:"subnet_id_1"`
	ServiceID0    uint32 `json:"service_id_0"`
	ServiceID1    uint32 `json:"service_id_1"`

	ResourceGl0ID0   uint32
	ResourceGl0Type0 uint8
	ResourceGl1ID0   uint32
	ResourceGl1Type0 uint8
	ResourceGl2ID0   uint32
	ResourceGl2Type0 uint8

	ResourceGl0ID1   uint32
	ResourceGl0Type1 uint8
	ResourceGl1ID1   uint32
	ResourceGl1Type1 uint8
	ResourceGl2ID1   uint32
	ResourceGl2Type1 uint8
}

var KnowledgeGraphColumns = []*ckdb.Column{
	// 知识图谱
	ckdb.NewColumn("region_id_0", ckdb.UInt16),
	ckdb.NewColumn("region_id_1", ckdb.UInt16),
	ckdb.NewColumn("az_id_0", ckdb.UInt16),
	ckdb.NewColumn("az_id_1", ckdb.UInt16),
	ckdb.NewColumn("host_id_0", ckdb.UInt16),
	ckdb.NewColumn("host_id_1", ckdb.UInt16),
	ckdb.NewColumn("l3_device_type_0", ckdb.UInt8),
	ckdb.NewColumn("l3_device_type_1", ckdb.UInt8),
	ckdb.NewColumn("l3_device_id_0", ckdb.UInt32),
	ckdb.NewColumn("l3_device_id_1", ckdb.UInt32),
	ckdb.NewColumn("pod_node_id_0", ckdb.UInt32),
	ckdb.NewColumn("pod_node_id_1", ckdb.UInt32),
	ckdb.NewColumn("pod_ns_id_0", ckdb.UInt16),
	ckdb.NewColumn("pod_ns_id_1", ckdb.UInt16),
	ckdb.NewColumn("pod_group_id_0", ckdb.UInt32),
	ckdb.NewColumn("pod_group_id_1", ckdb.UInt32),
	ckdb.NewColumn("pod_id_0", ckdb.UInt32),
	ckdb.NewColumn("pod_id_1", ckdb.UInt32),
	ckdb.NewColumn("pod_cluster_id_0", ckdb.UInt16),
	ckdb.NewColumn("pod_cluster_id_1", ckdb.UInt16),
	ckdb.NewColumn("l3_epc_id_0", ckdb.Int32),
	ckdb.NewColumn("l3_epc_id_1", ckdb.Int32),
	ckdb.NewColumn("epc_id_0", ckdb.Int32),
	ckdb.NewColumn("epc_id_1", ckdb.Int32),
	ckdb.NewColumn("subnet_id_0", ckdb.UInt16),
	ckdb.NewColumn("subnet_id_1", ckdb.UInt16),
	ckdb.NewColumn("service_id_0", ckdb.UInt32),
	ckdb.NewColumn("service_id_1", ckdb.UInt32),

	ckdb.NewColumn("resource_gl0_id_0", ckdb.UInt32),
	ckdb.NewColumn("resource_gl0_type_0", ckdb.UInt8),
	ckdb.NewColumn("resource_gl1_id_0", ckdb.UInt32),
	ckdb.NewColumn("resource_gl1_type_0", ckdb.UInt8),
	ckdb.NewColumn("resource_gl2_id_0", ckdb.UInt32),
	ckdb.NewColumn("resource_gl2_type_0", ckdb.UInt8),

	ckdb.NewColumn("resource_gl0_id_1", ckdb.UInt32),
	ckdb.NewColumn("resource_gl0_type_1", ckdb.UInt8),
	ckdb.NewColumn("resource_gl1_id_1", ckdb.UInt32),
	ckdb.NewColumn("resource_gl1_type_1", ckdb.UInt8),
	ckdb.NewColumn("resource_gl2_id_1", ckdb.UInt32),
	ckdb.NewColumn("resource_gl2_type_1", ckdb.UInt8),
}

func (k *KnowledgeGraph) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt16(k.RegionID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.RegionID1); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.AZID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.AZID1); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.HostID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.HostID1); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.L3DeviceType0); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.L3DeviceType1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.L3DeviceID0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.L3DeviceID1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodNodeID0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodNodeID1); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.PodNSID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.PodNSID1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodGroupID0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodGroupID1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodID0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.PodID1); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.PodClusterID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.PodClusterID1); err != nil {
		return err
	}
	if err := block.WriteInt32(k.L3EpcID0); err != nil {
		return err
	}
	if err := block.WriteInt32(k.L3EpcID1); err != nil {
		return err
	}
	if err := block.WriteInt32(k.EpcID0); err != nil {
		return err
	}
	if err := block.WriteInt32(k.EpcID1); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.SubnetID0); err != nil {
		return err
	}
	if err := block.WriteUInt16(k.SubnetID1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ServiceID0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ServiceID1); err != nil {
		return err
	}

	if err := block.WriteUInt32(k.ResourceGl0ID0); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl0Type0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ResourceGl1ID0); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl1Type0); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ResourceGl2ID0); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl2Type0); err != nil {
		return err
	}

	if err := block.WriteUInt32(k.ResourceGl0ID1); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl0Type1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ResourceGl1ID1); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl1Type1); err != nil {
		return err
	}
	if err := block.WriteUInt32(k.ResourceGl2ID1); err != nil {
		return err
	}
	if err := block.WriteUInt8(k.ResourceGl2Type1); err != nil {
		return err
	}

	return nil
}

type FlowInfo struct {
	CloseType   uint16 `json:"close_type"`
	FlowSource  uint16 `json:"flow_source"`
	FlowID      uint64 `json:"flow_id"`
	TapType     uint16 `json:"tap_type"`
	TapPortType uint8  `json:"tap_port_type"` // 0: MAC, 1: IPv4, 2:IPv6, 3: ID
	TapPort     uint32 `json:"tap_port"`
	TapSide     string `json:"tap_side"`
	VtapID      uint16 `json:"vtap_id"`
	L2End0      bool   `json:"l2_end_0"`
	L2End1      bool   `json:"l2_end_1"`
	L3End0      bool   `json:"l3_end_0"`
	L3End1      bool   `json:"l3_end_1"`
	StartTime   int64  `json:"start_time"` // us
	EndTime     int64  `json:"end_time"`   // us
	Duration    uint64 `json:"duration"`   // us
	IsNewFlow   uint8  `json:"is_new_flow"`
	Status      uint8  `json:"status"`
}

var FlowInfoColumns = []*ckdb.Column{
	// 流信息
	ckdb.NewColumn("close_type", ckdb.UInt16).SetIndex(ckdb.IndexSet),
	ckdb.NewColumn("flow_source", ckdb.UInt16),
	ckdb.NewColumn("flow_id", ckdb.UInt64).SetIndex(ckdb.IndexMinmax),
	ckdb.NewColumn("tap_type", ckdb.UInt16),
	ckdb.NewColumn("tap_port_type", ckdb.UInt8),
	ckdb.NewColumn("tap_port", ckdb.UInt32),
	ckdb.NewColumn("tap_side", ckdb.LowCardinalityString),
	ckdb.NewColumn("vtap_id", ckdb.UInt16).SetIndex(ckdb.IndexSet),
	ckdb.NewColumn("l2_end_0", ckdb.UInt8).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l2_end_1", ckdb.UInt8).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l3_end_0", ckdb.UInt8).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l3_end_1", ckdb.UInt8).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("start_time", ckdb.DateTime64us).SetComment("精度: 微秒"),
	ckdb.NewColumn("end_time", ckdb.DateTime64us).SetComment("精度: 微秒"),
	ckdb.NewColumn("time", ckdb.DateTime).SetComment("精度: 秒，等同end_time的秒精度"),
	ckdb.NewColumn("duration", ckdb.UInt64).SetComment("单位: 微秒"),
	ckdb.NewColumn("is_new_flow", ckdb.UInt8),
	ckdb.NewColumn("status", ckdb.UInt8).SetComment("状态 0:正常, 1:异常 ,2:不存在，3:服务端异常, 4:客户端异常"),
}

func (f *FlowInfo) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt16(f.CloseType); err != nil {
		return err
	}
	if err := block.WriteUInt16(f.FlowSource); err != nil {
		return err
	}
	if err := block.WriteUInt64(f.FlowID); err != nil {
		return err
	}
	if err := block.WriteUInt16(f.TapType); err != nil {
		return err
	}
	if err := block.WriteUInt8(f.TapPortType); err != nil {
		return err
	}
	if err := block.WriteUInt32(f.TapPort); err != nil {
		return err
	}
	if err := block.WriteString(f.TapSide); err != nil {
		return err
	}
	if err := block.WriteUInt16(f.VtapID); err != nil {
		return err
	}
	if err := block.WriteBool(f.L2End0); err != nil {
		return err
	}
	if err := block.WriteBool(f.L2End1); err != nil {
		return err
	}
	if err := block.WriteBool(f.L3End0); err != nil {
		return err
	}
	if err := block.WriteBool(f.L3End1); err != nil {
		return err
	}
	if err := block.WriteInt64(f.StartTime); err != nil {
		return err
	}
	if err := block.WriteInt64(f.EndTime); err != nil {
		return err
	}
	if err := block.WriteDateTime(uint32(f.EndTime / US_TO_S_DEVISOR)); err != nil {
		return err
	}
	if err := block.WriteUInt64(f.Duration); err != nil {
		return err
	}
	if err := block.WriteUInt8(f.IsNewFlow); err != nil {
		return err
	}
	if err := block.WriteUInt8(f.Status); err != nil {
		return err
	}

	return nil
}

type Metrics struct {
	PacketTx      uint64 `json:"packet_tx,omitempty"`
	PacketRx      uint64 `json:"packet_rx,omitempty"`
	ByteTx        uint64 `json:"byte_tx,omitempty"`
	ByteRx        uint64 `json:"byte_rx,omitempty"`
	L3ByteTx      uint64 `json:"l3_byte_tx,omitempty"`
	L3ByteRx      uint64 `json:"l3_byte_rx,omitempty"`
	L4ByteTx      uint64 `json:"l4_byte_tx,omitempty"`
	L4ByteRx      uint64 `json:"l4_byte_rx,omitempty"`
	TotalPacketTx uint64 `json:"total_packet_tx,omitempty"`
	TotalPacketRx uint64 `json:"total_packet_rx,omitempty"`
	TotalByteTx   uint64 `json:"total_byte_tx,omitempty"`
	TotalByteRx   uint64 `json:"total_byte_rx,omitempty"`
	L7Request     uint32 `json:"l7_request,omitempty"`
	L7Response    uint32 `json:"l7_response,omitempty"`

	RTT          uint32 `json:"rtt,omitempty"` // us
	RTTClientSum uint32 `json:"rtt_client_sum,omitempty"`
	RTTServerSum uint32 `json:"rtt_server_sum,omitempty"`
	SRTSum       uint32 `json:"srt_sum,omitempty"`
	ARTSum       uint32 `json:"art_sum,omitempty"`
	RRTSum       uint64 `json:"rrt_sum,omitempty"`
	CITSum       uint32 `json:"cit_sum,omitempty"`

	RTTClientCount uint32 `json:"rtt_client_count,omitempty"`
	RTTServerCount uint32 `json:"rtt_server_count,omitempty"`
	SRTCount       uint32 `json:"srt_count,omitempty"`
	ARTCount       uint32 `json:"art_count,omitempty"`
	RRTCount       uint32 `json:"rrt_count,omitempty"`
	CITCount       uint32 `json:"cit_count,omitempty"`

	RTTClientMax uint32 `json:"rtt_client_max,omitempty"` // us
	RTTServerMax uint32 `json:"rtt_server_max,omitempty"` // us
	SRTMax       uint32 `json:"srt_max,omitempty"`        // us
	ARTMax       uint32 `json:"art_max,omitempty"`        // us
	RRTMax       uint32 `json:"rrt_max,omitempty"`        // us
	CITMax       uint32 `json:"cit_max,omitempty"`        // us

	RetransTx       uint32 `json:"retrans_tx,omitempty"`
	RetransRx       uint32 `json:"retrans_rx,omitempty"`
	ZeroWinTx       uint32 `json:"zero_win_tx,omitempty"`
	ZeroWinRx       uint32 `json:"zero_win_rx,omitempty"`
	SynCount        uint32 `json:"syn_count,omitempty"`
	SynackCount     uint32 `json:"synack_count,omitempty"`
	L7ClientError   uint32 `json:"l7_client_error,omitempty"`
	L7ServerError   uint32 `json:"l7_server_error,omitempty"`
	L7ServerTimeout uint32 `json:"l7_server_timeout,omitempty"`
	L7Error         uint32 `json:"l7_error,omitempty"`
}

var MetricsColumns = []*ckdb.Column{
	// 指标量
	ckdb.NewColumn("packet_tx", ckdb.UInt64),
	ckdb.NewColumn("packet_rx", ckdb.UInt64),
	ckdb.NewColumn("byte_tx", ckdb.UInt64),
	ckdb.NewColumn("byte_rx", ckdb.UInt64),
	ckdb.NewColumn("l3_byte_tx", ckdb.UInt64),
	ckdb.NewColumn("l3_byte_rx", ckdb.UInt64),
	ckdb.NewColumn("l4_byte_tx", ckdb.UInt64),
	ckdb.NewColumn("l4_byte_rx", ckdb.UInt64),
	ckdb.NewColumn("total_packet_tx", ckdb.UInt64),
	ckdb.NewColumn("total_packet_rx", ckdb.UInt64),
	ckdb.NewColumn("total_byte_tx", ckdb.UInt64),
	ckdb.NewColumn("total_byte_rx", ckdb.UInt64),
	ckdb.NewColumn("l7_request", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l7_response", ckdb.UInt32).SetIndex(ckdb.IndexNone),

	ckdb.NewColumn("rtt", ckdb.Float64).SetComment("单位: 微秒"),
	ckdb.NewColumn("rtt_client_sum", ckdb.Float64),
	ckdb.NewColumn("rtt_server_sum", ckdb.Float64),
	ckdb.NewColumn("srt_sum", ckdb.Float64),
	ckdb.NewColumn("art_sum", ckdb.Float64),
	ckdb.NewColumn("rrt_sum", ckdb.Float64),
	ckdb.NewColumn("cit_sum", ckdb.Float64),

	ckdb.NewColumn("rtt_client_count", ckdb.UInt64),
	ckdb.NewColumn("rtt_server_count", ckdb.UInt64),
	ckdb.NewColumn("srt_count", ckdb.UInt64),
	ckdb.NewColumn("art_count", ckdb.UInt64),
	ckdb.NewColumn("rrt_count", ckdb.UInt64),
	ckdb.NewColumn("cit_count", ckdb.UInt64),

	ckdb.NewColumn("rtt_client_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),
	ckdb.NewColumn("rtt_server_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),
	ckdb.NewColumn("srt_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),
	ckdb.NewColumn("art_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),
	ckdb.NewColumn("rrt_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),
	ckdb.NewColumn("cit_max", ckdb.UInt32).SetIndex(ckdb.IndexNone).SetComment("单位: 微秒"),

	ckdb.NewColumn("retrans_tx", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("retrans_rx", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("zero_win_tx", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("zero_win_rx", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("syn_count", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("synack_count", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l7_client_error", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l7_server_error", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l7_server_timeout", ckdb.UInt32).SetIndex(ckdb.IndexNone),
	ckdb.NewColumn("l7_error", ckdb.UInt32).SetIndex(ckdb.IndexNone),
}

func (m *Metrics) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt64(m.PacketTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.PacketRx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.ByteTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.ByteRx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.L3ByteTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.L3ByteRx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.L4ByteTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.L4ByteRx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.TotalPacketTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.TotalPacketRx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.TotalByteTx); err != nil {
		return err
	}
	if err := block.WriteUInt64(m.TotalByteRx); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7Request); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7Response); err != nil {
		return err
	}

	if err := block.WriteFloat64(float64(m.RTT)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.RTTClientSum)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.RTTServerSum)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.SRTSum)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.ARTSum)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.RRTSum)); err != nil {
		return err
	}
	if err := block.WriteFloat64(float64(m.CITSum)); err != nil {
		return err
	}

	if err := block.WriteUInt64(uint64(m.RTTClientCount)); err != nil {
		return err
	}
	if err := block.WriteUInt64(uint64(m.RTTServerCount)); err != nil {
		return err
	}
	if err := block.WriteUInt64(uint64(m.SRTCount)); err != nil {
		return err
	}
	if err := block.WriteUInt64(uint64(m.ARTCount)); err != nil {
		return err
	}
	if err := block.WriteUInt64(uint64(m.RRTCount)); err != nil {
		return err
	}
	if err := block.WriteUInt64(uint64(m.CITCount)); err != nil {
		return err
	}

	if err := block.WriteUInt32(m.RTTClientMax); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.RTTServerMax); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.SRTMax); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.ARTMax); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.RRTMax); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.CITMax); err != nil {
		return err
	}

	if err := block.WriteUInt32(m.RetransTx); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.RetransRx); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.ZeroWinTx); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.ZeroWinRx); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.SynCount); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.SynackCount); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7ClientError); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7ServerError); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7ServerTimeout); err != nil {
		return err
	}
	if err := block.WriteUInt32(m.L7Error); err != nil {
		return err
	}
	return nil
}

func parseUint32EpcID(v uint32) int32 {
	switch int16(v) {
	case datatype.EPC_FROM_DEEPFLOW:
		fallthrough
	case datatype.EPC_FROM_INTERNET:
		return int32(int16(v))
	}
	return int32(math.MaxUint16 & v)
}

func (d *DataLinkLayer) Fill(f *pb.Flow) {
	d.MAC0 = f.FlowKey.MacSrc
	d.MAC1 = f.FlowKey.MacDst
	d.EthType = uint16(f.EthType)
	d.VLAN = uint16(f.Vlan)
}

func cloneIP(src net.IP) net.IP {
	l := len(src)
	if l == 0 {
		return nil
	}
	dst := make([]byte, l)
	copy(dst, src)
	return dst
}

func (n *NetworkLayer) Fill(f *pb.Flow, isIPV6 bool) {
	// 广域网IP为0.0.0.0或::
	if isIPV6 {
		n.IsIPv4 = false
		n.IP60 = cloneIP(f.FlowKey.Ip6Src)
		n.IP61 = cloneIP(f.FlowKey.Ip6Dst)
	} else {
		n.IsIPv4 = true
		n.IP40 = f.FlowKey.IpSrc
		n.IP41 = f.FlowKey.IpDst
	}

	n.Protocol = uint8(f.FlowKey.Proto)
	if f.Tunnel.TunnelType != uint32(datatype.TUNNEL_TYPE_NONE) {
		n.TunnelTier = uint8(f.Tunnel.Tier)
		n.TunnelTxID = f.Tunnel.TxId
		n.TunnelRxID = f.Tunnel.RxId
		n.TunnelType = uint16(f.Tunnel.TunnelType)
		n.TunnelTxIP40 = f.Tunnel.TxIp0
		n.TunnelTxIP41 = f.Tunnel.TxIp1
		n.TunnelRxIP40 = f.Tunnel.RxIp0
		n.TunnelRxIP41 = f.Tunnel.RxIp1
		n.TunnelIsIPv4 = true
		n.TunnelTxMac0 = f.Tunnel.TxMac0
		n.TunnelTxMac1 = f.Tunnel.TxMac1
		n.TunnelRxMac0 = f.Tunnel.RxMac0
		n.TunnelRxMac1 = f.Tunnel.RxMac1
	}
}

func (t *TransportLayer) Fill(f *pb.Flow) {
	t.ClientPort = uint16(f.FlowKey.PortSrc)
	t.ServerPort = uint16(f.FlowKey.PortDst)
	t.TCPFlagsBit0 = uint16(f.MetricsPeerSrc.TcpFlags)
	t.TCPFlagsBit1 = uint16(f.MetricsPeerDst.TcpFlags)
	t.SynSeq = f.SynSeq
	t.SynAckSeq = f.SynackSeq
	t.LastKeepaliveSeq = f.LastKeepaliveSeq
	t.LastKeepaliveAck = f.LastKeepaliveAck
}

func (a *ApplicationLayer) Fill(f *pb.Flow) {
	if f.HasPerfStats == 1 {
		a.L7Protocol = uint8(f.PerfStats.L7Protocol)
	}
}

func (i *Internet) Fill(f *pb.Flow) {
	i.Province0 = geo.QueryProvince(f.FlowKey.IpSrc)
	i.Province1 = geo.QueryProvince(f.FlowKey.IpDst)
}

func (k *KnowledgeGraph) fill(
	platformData *grpc.PlatformInfoTable,
	isIPv6, isVipInterface0, isVipInterface1 bool,
	l3EpcID0, l3EpcID1 int16,
	ip40, ip41 uint32,
	ip60, ip61 net.IP,
	mac0, mac1 uint64,
	port uint16,
	tapSide uint32,
	protocol layers.IPProtocol) {

	var info0, info1 *grpc.Info
	// 对于VIP的流量，需要使用MAC来匹配
	lookupByMac0, lookupByMac1 := isVipInterface0, isVipInterface1
	// 对于本地的流量，也需要使用MAC来匹配
	if tapSide == uint32(zerodoc.Local) {
		lookupByMac0, lookupByMac1 = true, true
	} else if tapSide == uint32(zerodoc.ClientProcess) || tapSide == uint32(zerodoc.ServerProcess) {
		// For ebpf traffic, if MAC is valid, MAC lookup is preferred
		if mac0 != 0 {
			lookupByMac0 = true
		}
		if mac1 != 0 {
			lookupByMac1 = true
		}
	}
	l3EpcMac0, l3EpcMac1 := mac0|uint64(l3EpcID0)<<48, mac1|uint64(l3EpcID1)<<48 // 使用l3EpcID和mac查找，防止跨AZ mac冲突

	if lookupByMac0 && lookupByMac1 {
		info0, info1 = platformData.QueryMacInfosPair(l3EpcMac0, l3EpcMac1)
		if info0 == nil {
			info0 = common.RegetInfoFromIP(isIPv6, ip60, ip40, l3EpcID0, platformData)
		}
		if info1 == nil {
			info1 = common.RegetInfoFromIP(isIPv6, ip61, ip41, l3EpcID1, platformData)
		}
	} else if lookupByMac0 {
		info0 = platformData.QueryMacInfo(l3EpcMac0)
		if info0 == nil {
			info0 = common.RegetInfoFromIP(isIPv6, ip60, ip40, l3EpcID0, platformData)
		}
		if isIPv6 {
			info1 = platformData.QueryIPV6Infos(l3EpcID1, ip61)
		} else {
			info1 = platformData.QueryIPV4Infos(l3EpcID1, ip41)
		}
	} else if lookupByMac1 {
		if isIPv6 {
			info0 = platformData.QueryIPV6Infos(l3EpcID0, ip60)
		} else {
			info0 = platformData.QueryIPV4Infos(l3EpcID0, ip40)
		}
		info1 = platformData.QueryMacInfo(l3EpcMac1)
		if info1 == nil {
			info1 = common.RegetInfoFromIP(isIPv6, ip61, ip41, l3EpcID1, platformData)
		}
	} else if isIPv6 {
		info0, info1 = platformData.QueryIPV6InfosPair(l3EpcID0, ip60, l3EpcID1, ip61)
	} else {
		info0, info1 = platformData.QueryIPV4InfosPair(l3EpcID0, ip40, int16(l3EpcID1), ip41)
	}

	var l2Info0, l2Info1 *grpc.Info
	if l3EpcID0 > 0 && l3EpcID1 > 0 {
		l2Info0, l2Info1 = platformData.QueryMacInfosPair(l3EpcMac0, l3EpcMac1)
	} else if l3EpcID0 > 0 {
		l2Info0 = platformData.QueryMacInfo(l3EpcMac0)
	} else if l3EpcID1 > 0 {
		l2Info1 = platformData.QueryMacInfo(l3EpcMac1)
	}

	if info0 != nil {
		k.RegionID0 = uint16(info0.RegionID)
		k.AZID0 = uint16(info0.AZID)
		k.HostID0 = uint16(info0.HostID)
		k.L3DeviceType0 = uint8(info0.DeviceType)
		k.L3DeviceID0 = info0.DeviceID
		k.PodNodeID0 = info0.PodNodeID
		k.PodNSID0 = uint16(info0.PodNSID)
		k.PodGroupID0 = info0.PodGroupID
		k.PodID0 = info0.PodID
		k.PodClusterID0 = uint16(info0.PodClusterID)
		k.SubnetID0 = uint16(info0.SubnetID)
	}
	if info1 != nil {
		k.RegionID1 = uint16(info1.RegionID)
		k.AZID1 = uint16(info1.AZID)
		k.HostID1 = uint16(info1.HostID)
		k.L3DeviceType1 = uint8(info1.DeviceType)
		k.L3DeviceID1 = info1.DeviceID
		k.PodNodeID1 = info1.PodNodeID
		k.PodNSID1 = uint16(info1.PodNSID)
		k.PodGroupID1 = info1.PodGroupID
		k.PodID1 = info1.PodID
		k.PodClusterID1 = uint16(info1.PodClusterID)
		k.SubnetID1 = uint16(info1.SubnetID)
	}
	k.L3EpcID0, k.L3EpcID1 = int32(l3EpcID0), int32(l3EpcID1)
	if l2Info0 != nil {
		k.EpcID0 = parseUint32EpcID(l2Info0.L2EpcID)
	}
	if l2Info1 != nil {
		k.EpcID1 = parseUint32EpcID(l2Info1.L2EpcID)
	}

	if isIPv6 {
		// 0端如果是clusterIP或后端podIP需要匹配service_id
		if k.L3DeviceType0 == uint8(trident.DeviceType_DEVICE_TYPE_POD_SERVICE) ||
			k.PodID0 != 0 {
			_, k.ServiceID0 = platformData.QueryIPv6IsKeyServiceAndID(l3EpcID0, ip60, protocol, 0)
		}
		// 1端如果是NodeIP,clusterIP或后端podIP需要匹配service_id
		if k.L3DeviceType1 == uint8(trident.DeviceType_DEVICE_TYPE_POD_SERVICE) ||
			k.PodID1 != 0 ||
			k.PodNodeID1 != 0 {
			_, k.ServiceID1 = platformData.QueryIPv6IsKeyServiceAndID(l3EpcID1, ip61, protocol, port)
		}
	} else {
		// 0端如果是clusterIP或后端podIP需要匹配service_id
		if k.L3DeviceType0 == uint8(trident.DeviceType_DEVICE_TYPE_POD_SERVICE) ||
			k.PodID0 != 0 {
			_, k.ServiceID0 = platformData.QueryIsKeyServiceAndID(l3EpcID0, ip40, protocol, 0)
		}
		// 1端如果是NodeIP,clusterIP或后端podIP需要匹配service_id
		if k.L3DeviceType1 == uint8(trident.DeviceType_DEVICE_TYPE_POD_SERVICE) ||
			k.PodID1 != 0 ||
			k.PodNodeID1 != 0 {
			_, k.ServiceID1 = platformData.QueryIsKeyServiceAndID(l3EpcID1, ip41, protocol, port)
		}
	}

	k.ResourceGl0ID0, k.ResourceGl0Type0 = common.GetResourceGl0(k.PodID0, k.PodNodeID0, k.L3DeviceID0, k.L3DeviceType0, int16(k.L3EpcID0))
	k.ResourceGl1ID0, k.ResourceGl1Type0 = common.GetResourceGl1(k.PodGroupID0, k.PodNodeID0, k.L3DeviceID0, k.L3DeviceType0, int16(k.L3EpcID0))
	k.ResourceGl2ID0, k.ResourceGl2Type0 = common.GetResourceGl2(k.ServiceID0, k.PodGroupID0, k.PodNodeID0, k.L3DeviceID0, k.L3DeviceType0, int16(k.L3EpcID0))

	k.ResourceGl0ID1, k.ResourceGl0Type1 = common.GetResourceGl0(k.PodID1, k.PodNodeID1, k.L3DeviceID1, k.L3DeviceType1, int16(k.L3EpcID1))
	k.ResourceGl1ID1, k.ResourceGl1Type1 = common.GetResourceGl1(k.PodGroupID1, k.PodNodeID1, k.L3DeviceID1, k.L3DeviceType1, int16(k.L3EpcID1))
	k.ResourceGl2ID1, k.ResourceGl2Type1 = common.GetResourceGl2(k.ServiceID1, k.PodGroupID1, k.PodNodeID1, k.L3DeviceID1, k.L3DeviceType1, int16(k.L3EpcID1))
}

func (k *KnowledgeGraph) FillL4(f *pb.Flow, isIPv6 bool, platformData *grpc.PlatformInfoTable) {
	k.fill(platformData,
		isIPv6, f.MetricsPeerSrc.IsVipInterface == 1, f.MetricsPeerDst.IsVipInterface == 1,
		int16(f.MetricsPeerSrc.L3EpcId), int16(f.MetricsPeerDst.L3EpcId),
		f.FlowKey.IpSrc, f.FlowKey.IpDst,
		f.FlowKey.Ip6Src, f.FlowKey.Ip6Dst,
		f.FlowKey.MacSrc, f.FlowKey.MacDst,
		uint16(f.FlowKey.PortDst),
		f.TapSide,
		layers.IPProtocol(f.FlowKey.Proto))
}

func getStatus(t datatype.CloseType) uint8 {
	if t == datatype.CloseTypeTCPFin || t == datatype.CloseTypeForcedReport {
		return datatype.STATUS_OK
	} else if t.IsClientError() {
		return datatype.STATUS_CLIENT_ERROR
	} else if t.IsServerError() {
		return datatype.STATUS_SERVER_ERROR
	} else {
		return datatype.STATUS_NOT_EXIST
	}
}

func (i *FlowInfo) Fill(f *pb.Flow) {
	i.CloseType = uint16(f.CloseType)
	i.FlowSource = uint16(f.FlowSource)
	i.FlowID = f.FlowId
	i.TapType = uint16(f.FlowKey.TapType)
	i.TapPort, i.TapPortType, _ = datatype.TapPort(f.FlowKey.TapPort).SplitToPortTypeTunnel()
	i.TapSide = zerodoc.TAPSideEnum(f.TapSide).String()
	i.VtapID = uint16(f.FlowKey.VtapId)

	i.L2End0 = f.MetricsPeerSrc.IsL2End == 1
	i.L2End1 = f.MetricsPeerDst.IsL2End == 1
	i.L3End0 = f.MetricsPeerSrc.IsL3End == 1
	i.L3End1 = f.MetricsPeerDst.IsL3End == 1

	i.StartTime = int64(f.StartTime) / int64(time.Microsecond)
	i.EndTime = int64(f.EndTime) / int64(time.Microsecond)
	i.Duration = f.Duration / uint64(time.Microsecond)
	i.IsNewFlow = uint8(f.IsNewFlow)
	i.Status = getStatus(datatype.CloseType(i.CloseType))
}

func (m *Metrics) Fill(f *pb.Flow) {
	m.PacketTx = f.MetricsPeerSrc.PacketCount
	m.PacketRx = f.MetricsPeerDst.PacketCount
	m.ByteTx = f.MetricsPeerSrc.ByteCount
	m.ByteRx = f.MetricsPeerDst.ByteCount
	m.L3ByteTx = f.MetricsPeerSrc.L3ByteCount
	m.L3ByteRx = f.MetricsPeerDst.L3ByteCount
	m.L4ByteTx = f.MetricsPeerSrc.L4ByteCount
	m.L4ByteRx = f.MetricsPeerDst.L4ByteCount

	m.TotalPacketTx = f.MetricsPeerSrc.TotalPacketCount
	m.TotalPacketRx = f.MetricsPeerDst.TotalPacketCount
	m.TotalByteTx = f.MetricsPeerSrc.TotalByteCount
	m.TotalByteRx = f.MetricsPeerDst.TotalByteCount

	if f.HasPerfStats == 1 {
		p := f.PerfStats
		m.L7Request = p.L7.RequestCount
		m.L7Response = p.L7.ResponseCount
		m.L7ClientError = p.L7.ErrClientCount
		m.L7ServerError = p.L7.ErrServerCount
		m.L7ServerTimeout = p.L7.ErrTimeout
		m.L7Error = m.L7ClientError + m.L7ServerError

		m.RTT = p.Tcp.Rtt
		m.RTTClientSum = p.Tcp.RttClientSum
		m.RTTClientCount = p.Tcp.RttClientCount

		m.RTTServerSum = p.Tcp.RttServerSum
		m.RTTServerCount = p.Tcp.RttServerCount

		m.SRTSum = p.Tcp.SrtSum
		m.SRTCount = p.Tcp.SrtCount

		m.ARTSum = p.Tcp.ArtSum
		m.ARTCount = p.Tcp.ArtCount

		m.RRTSum = p.L7.RrtSum
		m.RRTCount = p.L7.RrtCount

		m.CITSum = p.Tcp.CitSum
		m.CITCount = p.Tcp.CitCount

		m.RTTClientMax = p.Tcp.RttClientMax
		m.RTTServerMax = p.Tcp.RttServerMax
		m.SRTMax = p.Tcp.SrtMax
		m.ARTMax = p.Tcp.ArtMax
		m.RRTMax = p.L7.RrtMax
		m.CITMax = p.Tcp.CitMax

		if p.Tcp.CountsPeerTx != nil {
			m.RetransTx = p.Tcp.CountsPeerTx.RetransCount
			m.ZeroWinTx = p.Tcp.CountsPeerTx.ZeroWinCount
		}
		if p.Tcp.CountsPeerRx != nil {
			m.RetransRx = p.Tcp.CountsPeerRx.RetransCount
			m.ZeroWinRx = p.Tcp.CountsPeerRx.ZeroWinCount
		}
		m.SynCount = p.Tcp.SynCount
		m.SynackCount = p.Tcp.SynackCount
	}
}

func (f *FlowLogger) Release() {
	ReleaseFlowLogger(f)
}

func FlowLoggerColumns() []*ckdb.Column {
	columns := []*ckdb.Column{}
	columns = append(columns, ckdb.NewColumn("_id", ckdb.UInt64).SetCodec(ckdb.CodecDoubleDelta))
	columns = append(columns, DataLinkLayerColumns...)
	columns = append(columns, KnowledgeGraphColumns...)
	columns = append(columns, NetworkLayerColumns...)
	columns = append(columns, TransportLayerColumns...)
	columns = append(columns, ApplicationLayerColumns...)
	columns = append(columns, InternetColumns...)
	columns = append(columns, FlowInfoColumns...)
	columns = append(columns, MetricsColumns...)
	return columns
}

func (f *FlowLogger) WriteBlock(block *ckdb.Block) error {
	if err := block.WriteUInt64(f._id); err != nil {
		return err
	}

	if err := f.DataLinkLayer.WriteBlock(block); err != nil {
		return err
	}

	if err := f.KnowledgeGraph.WriteBlock(block); err != nil {
		return err
	}

	if err := f.NetworkLayer.WriteBlock(block); err != nil {
		return err
	}

	if err := f.TransportLayer.WriteBlock(block); err != nil {
		return err
	}

	if err := f.ApplicationLayer.WriteBlock(block); err != nil {
		return err
	}

	if err := f.Internet.WriteBlock(block); err != nil {
		return err
	}

	if err := f.FlowInfo.WriteBlock(block); err != nil {
		return err
	}

	if err := f.Metrics.WriteBlock(block); err != nil {
		return err
	}

	return nil
}

func (f *FlowLogger) EndTime() time.Duration {
	return time.Duration(f.FlowInfo.EndTime) * time.Second
}

func (f *FlowLogger) String() string {
	return fmt.Sprintf("flow: %+v\n", *f)
}

var poolFlowLogger = pool.NewLockFreePool(func() interface{} {
	l := new(FlowLogger)
	return l
})

func AcquireFlowLogger() *FlowLogger {
	l := poolFlowLogger.Get().(*FlowLogger)
	l.ReferenceCount.Reset()
	return l
}

func ReleaseFlowLogger(l *FlowLogger) {
	if l == nil {
		return
	}
	if l.SubReferenceCount() {
		return
	}
	*l = FlowLogger{}
	poolFlowLogger.Put(l)
}

var L4FlowCounter uint32

func genID(time uint32, counter *uint32, shardID int) uint64 {
	count := atomic.AddUint32(counter, 1)
	// 高32位时间，24-32位 表示 shardid, 低24位是counter
	return uint64(time)<<32 | (uint64(shardID) << 24) | (uint64(count) & 0xffffff)
}

func TaggedFlowToLogger(f *pb.TaggedFlow, shardID int, platformData *grpc.PlatformInfoTable) *FlowLogger {
	isIPV6 := f.Flow.EthType == uint32(layers.EthernetTypeIPv6)

	s := AcquireFlowLogger()
	s._id = genID(uint32(f.Flow.EndTime/uint64(time.Second)), &L4FlowCounter, shardID)
	s.DataLinkLayer.Fill(f.Flow)
	s.NetworkLayer.Fill(f.Flow, isIPV6)
	s.TransportLayer.Fill(f.Flow)
	s.ApplicationLayer.Fill(f.Flow)
	s.Internet.Fill(f.Flow)
	s.KnowledgeGraph.FillL4(f.Flow, isIPV6, platformData)
	s.FlowInfo.Fill(f.Flow)
	s.Metrics.Fill(f.Flow)

	return s
}
