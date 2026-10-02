#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Export the toolchain environment needed to build the Rust module.
#
#   . tools/rfl-env.sh
#   make
#
# The variables are produced by tools/setup-rfl.sh, which stores them in
# $RFL_CACHE/env.sh (default cache: ~/.cache/loongson3-cpufreq-rfl).  Anything
# already set in the environment wins, so a hand-tuned toolchain can be used by
# exporting RUSTC/BINDGEN/KDIR first.

RFL_CACHE=${RFL_CACHE:-$HOME/.cache/loongson3-cpufreq-rfl}

if [ -r "$RFL_CACHE/env.sh" ]; then
	# shellcheck disable=SC1090
	. "$RFL_CACHE/env.sh"
else
	cat >&2 <<EOF
tools/rfl-env.sh: no prepared toolchain found in $RFL_CACHE.

A Rust module needs the compiled Rust abstractions of the kernel and the exact
rustc the kernel was built with, which a linux-headers package does not provide.
Run

    tools/setup-rfl.sh

once (it downloads everything into $RFL_CACHE, without root), then source this
file again.  Set RFL_CACHE to use a different directory.
EOF
	return 1 2>/dev/null || exit 1
fi

export KVER KDIR RUSTC RUSTDOC RUST_LIB_SRC BINDGEN LIBCLANG_PATH LD_LIBRARY_PATH

echo "Rust-for-Linux environment ready:"
echo "  KVER        = $KVER"
echo "  KDIR        = $KDIR"
echo "  RUSTC       = $RUSTC ($("$RUSTC" --version 2>/dev/null || echo 'not runnable'))"
echo "  BINDGEN     = $BINDGEN ($("$BINDGEN" --version 2>/dev/null || echo 'not runnable'))"
