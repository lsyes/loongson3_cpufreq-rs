# SPDX-License-Identifier: GPL-2.0-only
#
# Build file for the Loongson-3 CPUFreq driver, Rust edition.
#
# The file is dual purpose, which is why it is guarded by KERNELRELEASE:
#
#   * during a kbuild/DKMS build, kbuild parses this file itself (it invokes
#     `make -C <kernel_build_dir> M=<build_dir>`) and only the `obj-m`
#     assignment below is used;
#
#   * when invoked directly (`make`), it forwards to the kbuild of the target
#     kernel, so the module can also be built by hand.
#
# Unlike a C module, a Rust module cannot be built against a bare
# linux-headers package: the kernel build directory has to contain the compiled
# Rust abstractions (`rust/libkernel.rmeta` and friends) *and* the exact
# rustc/bindgen toolchain the kernel itself was built with, because Rust
# mangled symbol names embed a hash of both.  `tools/setup-rfl.sh` prepares
# such a directory and `tools/rfl-env.sh` exports the matching environment; see
# README.md.

ifneq ($(KERNELRELEASE),)

obj-m := loongson3_cpufreq.o

else

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build

# Toolchain; normally provided by tools/rfl-env.sh.
RUSTC ?= rustc
BINDGEN ?= bindgen

# A module carries the release of the kernel it was built against as its
# vermagic, and kbuild derives that from $(KERNELVERSION)$(LOCALVERSION).
# Distribution kernels append a local version (Debian: "+deb14-loong64"), so
# derive whatever makes the two agree.
KERNELVERSION := $(shell $(MAKE) -s -C $(KDIR) kernelversion 2>/dev/null)
LOCALVERSION ?= $(patsubst $(KERNELVERSION)%,%,$(KVER))

# The release the kernel build directory was prepared for.  It is baked into
# include/generated/utsrelease.h, which only a `prepare`-style target rewrites,
# so it has to match the running kernel or the module is rejected on load.
TREE_RELEASE := $(shell cat $(KDIR)/include/config/kernel.release 2>/dev/null)

KMAKE_ARGS := LOCALVERSION='$(LOCALVERSION)' RUSTC='$(RUSTC)' BINDGEN='$(BINDGEN)'
ifneq ($(RUST_LIB_SRC),)
KMAKE_ARGS += RUST_LIB_SRC='$(RUST_LIB_SRC)'
endif

.PHONY: all modules clean install help check-env

all: modules

check-env:
	@test -n '$(KERNELVERSION)' || { \
		echo "*** '$(KDIR)' is not a prepared kernel build directory."; \
		echo "*** Run tools/setup-rfl.sh first; see README.md."; exit 1; }
	@test '$(TREE_RELEASE)' = '$(KVER)' || { \
		echo "*** '$(KDIR)' was prepared for release '$(TREE_RELEASE)', but the"; \
		echo "*** target is '$(KVER)': the module would be rejected on load."; \
		echo "*** Re-run tools/setup-rfl.sh (it picks the running kernel up"; \
		echo "*** automatically), or pass KVER=/KDIR= explicitly."; exit 1; }
	@test -f '$(KDIR)/rust/libkernel.rmeta' || { \
		echo "*** '$(KDIR)' has no compiled Rust support."; \
		echo "*** linux-headers alone cannot build a Rust module; run"; \
		echo "*** tools/setup-rfl.sh to prepare a kernel source tree."; exit 1; }
	@command -v '$(RUSTC)' >/dev/null 2>&1 || { \
		echo "*** rustc '$(RUSTC)' not found; run tools/setup-rfl.sh."; exit 1; }
	@command -v '$(BINDGEN)' >/dev/null 2>&1 || { \
		echo "*** bindgen '$(BINDGEN)' not found; run tools/setup-rfl.sh."; exit 1; }

modules: check-env
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(KMAKE_ARGS) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean

# DESTDIR is honoured so that staged installs work; leave it empty to install
# into the running system.
install: check-env
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(KMAKE_ARGS) \
		INSTALL_MOD_PATH=$(DESTDIR) modules_install
ifndef DESTDIR
	depmod -a $(KVER) 2>/dev/null || true
endif

help:
	@echo "Targets:"
	@echo "  all (default)  build loongson3_cpufreq.ko against KDIR"
	@echo "  modules        same as all"
	@echo "  clean          remove build artefacts"
	@echo "  install        install the module into /lib/modules/\$$(KVER)"
	@echo "  check-env      verify that KDIR and the Rust toolchain are usable"
	@echo ""
	@echo "Variables:"
	@echo "  KVER           kernel release to build for (default: \$$(uname -r))"
	@echo "  KDIR           kernel build directory (default: /lib/modules/\$$(KVER)/build)"
	@echo "  RUSTC          rustc to use (must be the one the kernel was built with)"
	@echo "  BINDGEN        bindgen to use"
	@echo "  RUST_LIB_SRC   path to the 'library' directory of a rust-src checkout"
	@echo "  LOCALVERSION   kernel local version (default: derived from KDIR and KVER)"
	@echo ""
	@echo "See tools/rfl-env.sh to set up the environment in one go."

endif
