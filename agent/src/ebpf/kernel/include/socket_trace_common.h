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

#ifndef __BPF_SOCKET_TRACE_COMMON_H__
#define __BPF_SOCKET_TRACE_COMMON_H__
#define CAP_DATA_SIZE 1024

enum endpoint_role {
	ROLE_UNKNOWN,
	ROLE_CLIENT,
	ROLE_SERVER
};

struct __tuple_t {
	__u8 daddr[16];
	__u8 rcv_saddr[16];
	__u8 addr_len;
	__u8 l4_protocol;
	__u16 dport;
	__u16 num;
};

struct __socket_data {
	/* 进程/线程信息 */
	__u32 pid;  // 表示线程号 如果'pid == tgid'表示一个进程, 否则是线程
	__u32 tgid; // 进程号
	__u64 coroutine_id; // CoroutineID, i.e., golang goroutine id
	__u8  comm[16]; // 进程或线程名

	/* 连接（socket）信息 */
	__u64 socket_id;     /* 通信socket唯一ID， 从启动时的时钟开始自增1 */
	struct __tuple_t tuple;

	/*
	 * 携带数据， 比如：MySQL第一次读取的数据，被第二次读取的数据携带一并发给用户
	 * 注意携带数据只有4字节大小。
	 */
	__u32 extra_data;
	__u32 extra_data_count;

	/* 追踪信息 */
	__u32 tcp_seq;
	__u64 thread_trace_id;

	/* 追踪数据信息 */
	__u64 timestamp;     // 数据捕获时间戳
	__u8  direction: 1;  // bits[0]: 方向，值为T_EGRESS(0), T_INGRESS(1)
	__u8  msg_type:  7;  // bits[1-7]: 信息类型，值为MSG_UNKNOWN(0), MSG_REQUEST(1), MSG_RESPONSE(2)

	__u64 syscall_len;   // 本次系统调用读、写数据的总长度
	__u64 data_seq;      // cap_data在Socket中的相对顺序号
	__u16 data_type;     // HTTP, DNS, MySQL
	__u16 data_len;      // 数据长度
	char data[CAP_DATA_SIZE];
} __attribute__((packed));

/*
 * 整个结构大小为2^15（强制为2的次幂），目的是用（2^n - 1）与数据
 * 长度作位与操作使eBPF程序进行安全的bpf_perf_event_output()操作。
 */
struct __socket_data_buffer {
	__u32 events_num;
	__u32 len; // data部分长度
	char data[32760]; // 32760 + len(4bytes) + events_num(4bytes) = 2^15 = 32768
};

struct trace_uid_t {
	__u64 socket_id;       // 会话标识
	__u64 coroutine_trace_id;  // 同一协程的数据转发关联
	__u64 thread_trace_id; // 同一进程/线程的数据转发关联，用于多事务流转场景
};

struct trace_stats {
	__u64 socket_map_count;     // 对socket 链接表进行统计
	__u64 trace_map_count;     // 对同一进程/线程的多次转发表进行统计
};

struct socket_info_t {
	__u64 l7_proto: 8;
	__u64 seq: 56; // socket 读写数据的序列号，用于排序

	/*
	 * mysql, kafka这种类型在读取数据时，先读取4字节
	 * 然后再读取剩下的数据，这里用于对预先读取的数据存储
	 * 用于后续的协议分析。
	 */
	__u8 prev_data[4];
	__u8 direction: 1;
	__u8 msg_type: 2;	// 保存数据类型，值为MSG_UNKNOWN(0), MSG_REQUEST(1), MSG_RESPONSE(2)
	__u8 role: 5;           // 标识socket角色：ROLE_CLIENT, ROLE_SERVER, ROLE_UNKNOWN
	bool need_reconfirm;    // l7协议推断是否需要再次确认。
	__s32 correlation_id;   // 目前用于kafka协议推断。

	__u32 peer_fd;		// 用于记录socket间数据转移的对端fd。

	/*
	 * 一旦有数据读/写就会更新这个时间，这个时间是从系统开机开始
	 * 到更新时的间隔时间单位是秒。
	 */
	__u32 update_time;
	__u32 prev_data_len;
	__u64 trace_id;
	__u64 uid; // socket唯一标识ID
} __attribute__((packed));

struct trace_info_t {
	__u32 update_time; // 从系统开机开始到创建/更新时的间隔时间单位是秒
	__u32 peer_fd;	   // 用于socket之间的关联
	__u64 thread_trace_id; // 线程追踪ID
	__u64 socket_id; // Records the socket associated when tracing was created (记录创建追踪时关联的socket)
};

// struct member_offsets -> data[]  arrays index.
enum offsets_index {
	runtime_g_goid_offset = 0,
	crypto_tls_conn_conn_offset,
	net_poll_fd_sysfd,
	offsets_num,
};

// Store the member_offsets to eBPF Map.
struct member_offsets {
	__u32 version;
	__u16 data[offsets_num];
};

enum {
	/*
	 * 0 ~ 16 for L7 socket event (struct socket_data_buffer),
	 * indicates the number of socket data in socket_data_buffer.
	 */

	/*
	 * For event registrion
	 */
	EVENT_TYPE_MIN = 1 << 5,
	EVENT_TYPE_PROC_EXEC = 1 << 5,
	EVENT_TYPE_PROC_EXIT = 1 << 6
	// Add new event type here.
};

// Description Provides basic information about an event 
struct event_meta {
	__u32 event_type;
};

// Process execution or exit event data 
struct process_event_t {
	struct event_meta meta;
	__u32 pid; // process ID
	__u8 name[16]; // process name
};

#define GO_VERSION(a, b, c) (((a) << 16) + ((b) << 8) + ((c) > 255 ? 255 : (c)))

#endif /* BPF_SOCKET_TRACE_COMMON */
