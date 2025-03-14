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

#include <stdio.h>
#include <stdbool.h>
#include <stdlib.h>
#include <errno.h>
#include <unistd.h>
#include <linux/types.h>
#include "symbol.h"
#include "tracer.h"
#include "libbpf/src/libbpf.h"
#include "libbpf/src/bpf.h"
#include "table.h"
#include "log.h"

unsigned int bpf_table_key_size(struct bpf_map *map)
{
	const struct bpf_map_def *def = bpf_map__def(map);
	if (IS_ERR(def))
		return 0;
	return def->key_size;
}

unsigned int bpf_table_value_size(struct bpf_map *map)
{
	const struct bpf_map_def *def = bpf_map__def(map);
	if (IS_ERR(def))
		return 0;
	return def->value_size;
}

unsigned int bpf_table_max_entries(struct bpf_map *map)
{
	const struct bpf_map_def *def = bpf_map__def(map);
	if (IS_ERR(def))
		return 0;
	return def->max_entries;
}

unsigned int bpf_table_flags(struct bpf_map *map)
{
	const struct bpf_map_def *def = bpf_map__def(map);
	if (IS_ERR(def))
		return 0;
	return def->map_flags;
}

bool bpf_table_get_value(struct bpf_tracer * tracer,
			 const char *tb_name, uint64_t key, void *val_buf)
{
	struct bpf_map *map =
	    bpf_object__find_map_by_name(tracer->pobj, tb_name);
	int map_fd = bpf_map__fd(map);

	if ((bpf_map_lookup_elem(map_fd, &key, val_buf)) != 0) {
		ebpf_info("[%s] bpf_map_lookup_elem, err tb_name:%s, key : %"
			  PRIu64 ", err_message:%s\n", __func__, tb_name, key,
			  strerror(errno));
		return false;
	}

	return true;
}

uint32_t bpf_table_elems_count(struct bpf_tracer * tracer, const char *tb_name)
{
	struct bpf_map *map =
	    bpf_object__find_map_by_name(tracer->pobj, tb_name);
	int map_fd = bpf_map__fd(map);

	//int key_size = bpf_table_key_size(map);
	uint64_t key, next_key, count = 0;
	key = 0;
	while (bpf_map_get_next_key(map_fd, &key, &next_key) == 0) {
		count++;
		key = next_key;
	}

	return count;
}

bool bpf_table_set_value(struct bpf_tracer * tracer,
			 const char *tb_name, uint64_t key, void *val_buf)
{
	struct bpf_map *map =
	    bpf_object__find_map_by_name(tracer->pobj, tb_name);
	int map_fd = bpf_map__fd(map);

	if (bpf_map_update_elem(map_fd, &key, val_buf, BPF_ANY) != 0) {
		ebpf_warning("[%s] bpf_map_update_elem, err tb_name:%s, key : %"
			     PRIu64 ", err_message:%s\n", __func__, tb_name,
			     key, strerror(errno));
		return false;
	}

	return true;
}
