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

#ifndef __LINUX_KERN_H__
#define __LINUX_KERN_H__

/*
 * TODO: start_boottime or real_start_time ?
 */
#define STRUCT_TASK_START_BOOTTIME_OFFSET       0xa08
#define STRUCT_TASK_GROUP_LEADER_OFFSET         0x8e0
#define STRUCT_FILES_STRUCT_FDT_OFFSET          0x20
#define STRUCT_FILES_PRIVATE_DATA_OFFSET	0xc8
#define STRUCT_SOCK_FAMILY_OFFSET	0x10
#define STRUCT_SOCK_SADDR_OFFSET	0x4
#define STRUCT_SOCK_DADDR_OFFSET	0x0
#define STRUCT_SOCK_IP6SADDR_OFFSET	0x48
#define STRUCT_SOCK_IP6DADDR_OFFSET	0x38
#define STRUCT_SOCK_DPORT_OFFSET	0xc
#define STRUCT_SOCK_SPORT_OFFSET	0xe
#define STRUCT_TASK_NSPROXY_OFFSET      0xad0
#define STRUCT_NSPROXY_NS_OFFSET        0x28
#define STRUCT_NET_NS_OFFSET            0x70
#define STRUCT_NS_COMMON_INUM_OFFSET    0x10
#define STRUCT_SOCK_SKC_STATE_OFFSET    0x12
#define STRUCT_SOCK_COMMON_IPV6ONLY_OFFSET 0x13

#ifndef BPF_USE_CORE
typedef __u32 __bitwise __portpair;
typedef __u64 __bitwise __addrpair;

struct hlist_node {
    struct hlist_node *next;
    struct hlist_node **pprev;
};

typedef struct {
    void *net;
} possible_net_t;

struct sock_common {
        union {
                __addrpair      skc_addrpair;
                struct {
                        __be32  skc_daddr;
                        __be32  skc_rcv_saddr;
                };
        };
        union  {
                unsigned int    skc_hash;
                __u16           skc_u16hashes[2];
        };
        /* skc_dport && skc_num must be grouped as well */
        union {
                __portpair      skc_portpair;
                struct {
                        __be16  skc_dport;
                        __u16   skc_num;
                };
        };

        unsigned short          skc_family;
	volatile unsigned char skc_state;
	unsigned char skc_reuse : 4;
	unsigned char skc_reuseport : 1;
	unsigned char skc_ipv6only : 1;
	unsigned char skc_net_refcnt : 1;
	int skc_bound_dev_if;
	union {
		struct hlist_node skc_bind_node;
		struct hlist_node skc_portaddr_node;
	};
	void *skc_prot;
	possible_net_t skc_net;
	struct in6_addr skc_v6_daddr;
	struct in6_addr skc_v6_rcv_saddr;
};

struct sock {
        /*
         * Now struct inet_timewait_sock also uses sock_common, so please just
         * don't add nothing before this first member (__sk_common) --acme
         */
        struct sock_common      __sk_common;
#define sk_num                  __sk_common.skc_num
#define sk_dport                __sk_common.skc_dport
#define sk_addrpair             __sk_common.skc_addrpair
#define sk_daddr                __sk_common.skc_daddr
#define sk_rcv_saddr            __sk_common.skc_rcv_saddr
#define sk_family               __sk_common.skc_family
#define sk_v6_daddr		__sk_common.skc_v6_daddr
};

typedef enum {
	SS_FREE = 0,			/* not allocated		*/
	SS_UNCONNECTED,			/* unconnected to any socket	*/
	SS_CONNECTING,			/* in process of connecting	*/
	SS_CONNECTED,			/* connected to socket		*/
	SS_DISCONNECTING		/* in process of disconnecting	*/
} socket_state;

/**
 *  struct socket - general BSD socket
 *  @state: socket state (%SS_CONNECTED, etc)
 *  @type: socket type (%SOCK_STREAM, etc)
 *  @flags: socket flags (%SOCK_NOSPACE, etc)
 *  @ops: protocol specific socket operations
 *  @file: File back pointer for gc
 *  @sk: internal networking protocol agnostic socket representation
 *  @wq: wait queue for several uses
 */
struct socket {
	socket_state		state;
	short			type;
	unsigned long		flags;
	void			*wq; // kernel >= 5.3.0 remove
	void			*file; //struct file
	struct sock		*sk;
	const void		*ops;//struct proto_ops
};

struct fdtable {
	unsigned int max_fds;
	void **fd;      /* current fd array, struct file *  */
};
#endif
#endif /* __LINUX_KERN_H__ */
