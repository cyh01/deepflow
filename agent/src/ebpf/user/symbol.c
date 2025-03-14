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

#include <stdint.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <libgen.h>
#include <stdio.h>
#include <stdlib.h>
#include <limits.h>
#include <gelf.h>
#include <libelf.h>
#include <stdint.h>
#include <unistd.h>
#include <limits.h>		//PATH_MAX(4096)
#include "bcc/bcc_proc.h"
#include "bcc/bcc_elf.h"
#include "bcc/bcc_syms.h"
#include "log.h"
#include "common.h"
#include "symbol.h"
#include "tracer.h"
#include "bddisasm/bddisasm.h"
#include "bddisasm/disasmtypes.h"

void free_uprobe_symbol(struct symbol_uprobe *u_sym,
			struct tracer_probes_conf *conf)
{
	if (u_sym == NULL)
		return;

	if (u_sym->list.prev != NULL && u_sym->list.next != NULL) {
		list_head_del(&u_sym->list);
		if (conf)
			conf->uprobe_count--;
	}

	if (u_sym->name)
		free((void *)u_sym->name);
	if (u_sym->binary_path)
		free((void *)u_sym->binary_path);
	if (u_sym->probe_func)
		free((void *)u_sym->probe_func);

	free(u_sym);
}

void add_uprobe_symbol(int pid, struct symbol_uprobe *u_sym,
		       struct tracer_probes_conf *conf)
{
	u_sym->starttime = get_process_starttime(pid);
	u_sym->in_probe = false;
	list_add_tail(&u_sym->list, &conf->uprobe_syms_head);
	conf->uprobe_count++;
}

int copy_uprobe_symbol(struct symbol_uprobe *src, struct symbol_uprobe *dst)
{
	if (src == NULL || dst == NULL)
		return ETR_NOTEXIST;

	memcpy((void *)dst, src, sizeof(struct symbol_uprobe));
	if (src->name)
		dst->name = strdup(src->name);
	if (src->binary_path)
		dst->binary_path = strdup(src->binary_path);
	if (src->probe_func)
		dst->probe_func = strdup(src->probe_func);

	return ETR_OK;
}

static int find_sym(const char *symname, uint64_t addr, uint64_t size,
		    void *payload)
{
	struct symbol_uprobe *sym = (struct symbol_uprobe *)payload;
	char *pos;
	if ((pos = strstr(symname, sym->name))) {
		if (pos[strlen(sym->name)] == '\0') {
			sym->entry = addr;
			sym->size = size;
			return -1;
		}
	}
	return 0;
}

int find_load(uint64_t v_addr, uint64_t mem_sz, uint64_t file_offset,
	      void *payload)
{
	struct load_addr_t *addr = (struct load_addr_t *)payload;

	if (addr->target_addr >= v_addr
	    && addr->target_addr < (v_addr + mem_sz)) {
		addr->binary_addr = addr->target_addr - v_addr + file_offset;
		return -1;
	}

	return 0;
}

static void resolve_func_ret_addr(struct symbol_uprobe *uprobe_sym)
{
	NDSTATUS status;
	INSTRUX ix;
	int fd;
	size_t pc;
	int remian;
	int cnt = 0;
	size_t offset = 0;
	char *buffer = NULL;

	fd = open(uprobe_sym->binary_path, O_RDONLY);
	if (fd == -1)
		goto out;

	if (lseek(fd, uprobe_sym->entry, SEEK_SET) == -1)
		goto close_file;

	buffer = malloc(uprobe_sym->size);
	if (!buffer)
		goto close_file;

	if (read(fd, buffer, uprobe_sym->size) == -1)
		goto free_buffer;

	memset(uprobe_sym->rets, 0, sizeof(uprobe_sym->rets));
	pc = uprobe_sym->entry;
	while (offset < uprobe_sym->size && cnt < FUNC_RET_MAX) {
		remian = uprobe_sym->size - offset;
		status = NdDecodeEx(&ix, (ND_UINT8 *) (buffer + offset), remian,
				    ND_CODE_64, ND_DATA_64);
		if (!ND_SUCCESS(status))
			break;

		if (ix.Instruction == ND_INS_RETF ||
		    ix.Instruction == ND_INS_RETN) {
			uprobe_sym->rets[cnt++] = pc;
		}
		offset += ix.Length;
		pc += ix.Length;
	}

free_buffer:
	free(buffer);
close_file:
	close(fd);
out:
	uprobe_sym->rets_count = cnt;
}

/**
 * resolve_and_gen_uprobe_symbol -- 完成二进制文件中对给定符号的解析并生成uprobe_symbol
 * @bin_file: 二进制文件，如果是可执行文件需要指定文件的全路径,
 * 	      如果是库文件只需要给出库的名字即可,
 *            例如：libssl.so 只需提供名字"ssl"。
 * @sym: 符号信息。
 * @addr: 目标地址。非0，已经给定不需要bcc_elf_foreach_sym()进行获取了，否则需要遍历符号表。
 * @pid: 用于在指定的进程中查找使用的库(/proc/<pid>/maps)。
 * 返回值：
 *   成功返回0，失败返回非0
 */
struct symbol_uprobe *resolve_and_gen_uprobe_symbol(const char *bin_file,
						    struct symbol *sym,
						    const uint64_t addr,
						    int pid)
{
	static struct bcc_symbol_option default_option = {
		.use_debug_file = 1,
		.check_debug_file_crc = 1,
		.lazy_symbolize = 1,
#if defined(__powerpc64__) && defined(_CALL_ELF) && _CALL_ELF == 2
		.use_symbol_type =
		    BCC_SYM_ALL_TYPES | (1 << STT_PPC64_ELFV2_SYM_LEP),
#else
		.use_symbol_type = BCC_SYM_ALL_TYPES,
#endif
	};

	if (bin_file == NULL) {
		ebpf_warning("bin_file == NULL, failed.\n");
		return NULL;
	}

	struct symbol_uprobe *uprobe_sym =
	    calloc(1, sizeof(struct symbol_uprobe));
	if (uprobe_sym == NULL) {
		ebpf_warning("uprobe_sym = calloc() failed.\n");
		return NULL;
	}

	uprobe_sym->type = sym->type;
	uprobe_sym->isret = sym->is_probe_ret;
	uprobe_sym->name = strdup(sym->symbol);
	uprobe_sym->pid = pid;
	uprobe_sym->probe_func = strdup(sym->probe_func);
	/*
	 * 判断是可执行目标文件还是库文件。
	 */
	if (strchr(bin_file, '/'))
		uprobe_sym->binary_path = strdup(bin_file);
	else
		/*
		 * 从”/proc/<pid>/maps“和"/etc/ld.so.cache"查找
		 * "lib<libname>.so"绝对路径
		 */
		uprobe_sym->binary_path = bcc_procutils_which_so(bin_file, pid);

	if (uprobe_sym->binary_path == NULL)
		goto invalid;

	uprobe_sym->name = strdup(sym->symbol);
	uprobe_sym->entry = addr;

	if (uprobe_sym->name && uprobe_sym->entry == 0x0)
		if (bcc_elf_foreach_sym
		    (uprobe_sym->binary_path, find_sym, &default_option,
		     uprobe_sym) < 0)
			goto invalid;

	if (uprobe_sym->entry == 0x0)
		goto invalid;

	/* 
	 * 对于可执行的二进制文件(ET_EXEC), 把virtual address转换成物理地址。
	 * 对应共享库二进制文件(ET_DYN), 不需要进行转换。
	 * https://refspecs.linuxbase.org/elf/gabi4+/ch5.pheader.html
	 */
	if (bcc_elf_get_type(uprobe_sym->binary_path) == ET_EXEC) {
		struct load_addr_t addr = {
			.target_addr = uprobe_sym->entry,
			.binary_addr = 0x0,
		};

		if (strstr(uprobe_sym->name, "go.itab.*")) {
			addr.binary_addr = addr.target_addr;
			uprobe_sym->entry = addr.binary_addr;
			return 0;
		}

		if (bcc_elf_foreach_load_section
		    (uprobe_sym->binary_path, &find_load, &addr) < 0) {
			goto invalid;
		}
		if (!addr.binary_addr) {
			goto invalid;
		}
		uprobe_sym->entry = addr.binary_addr;
	}

	if (uprobe_sym->isret && uprobe_sym->type == GO_UPROBE) {
		resolve_func_ret_addr(uprobe_sym);
	}

	return uprobe_sym;

invalid:
	free_uprobe_symbol(uprobe_sym, NULL);
	return NULL;
}

char *get_elf_path_by_pid(int pid)
{
	int ret;
	char bin_path[PATH_MAX], *path;
	char proc_pid_exe[32];
	memset(bin_path, 0, sizeof(bin_path));
	memset(proc_pid_exe, 0, sizeof(proc_pid_exe));

	if (snprintf(proc_pid_exe, sizeof(proc_pid_exe), "/proc/%d/exe", pid)
	    >= sizeof(proc_pid_exe)) {
		ebpf_warning("snprintf /proc/%d/exe failed", pid);
		return NULL;
	}
	ret = readlink(proc_pid_exe, bin_path, sizeof(bin_path));
	if (ret < 0) {
		return NULL;
	}

	path = calloc(1, PATH_MAX);
	if (path == NULL)
		return NULL;
	if (snprintf(path, PATH_MAX, "/proc/%d/root%s", pid, bin_path)
	    >= PATH_MAX) {
		ebpf_warning("snprintf /proc/%d/root%s failed", pid, bin_path);
		free(path);
		return NULL;
	}

	if (access(path, F_OK) != 0) {
		memset(path, 0, PATH_MAX);
		memcpy(path, bin_path, sizeof(bin_path));
	}

	return path;

}

// bddisasm 库要求定义的函数
void *nd_memset(void *s, int c, ND_SIZET n)
{
	return memset(s, c, n);
}
