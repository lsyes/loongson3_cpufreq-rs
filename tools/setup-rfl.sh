#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Prepare everything needed to build a Rust-for-Linux out-of-tree module for the
# running kernel, without installing anything system-wide and without root.
#
#   tools/setup-rfl.sh [--cache DIR] [--jobs N] [--kver VERSION]
#
# Why this is needed at all
# -------------------------
# A Rust module is compiled against the *compiled* Rust abstractions of the
# kernel (`rust/libkernel.rmeta`, `rust/libcore.rmeta`, ...), which a
# `linux-headers-*` package does not ship -- distributions only build the C
# parts for out-of-tree modules.  On top of that, every Rust symbol is mangled
# with a hash of the compiler and the compilation flags, so the module has to be
# built with the very same rustc the kernel was built with: a different rustc
# -- even a different build of the same release -- produces different hashes and
# the module fails to load with unresolved symbols.
#
# What it does
# ------------
#   1. reads the rustc version the kernel was built with out of /boot/config-*;
#   2. downloads (never installs) the matching rustc, rust-src, bindgen and
#      libclang packages into $CACHE/debs and unpacks them into $CACHE/root;
#   3. downloads and unpacks the matching linux-source package into $CACHE/linux
#      and configures it with the running kernel's config;
#   4. builds the kernel's Rust support (`make prepare modules_prepare`);
#   5. copies the running kernel's Module.symvers into the tree so that modpost
#      can resolve the symbols the module imports;
#   6. writes $CACHE/env.sh, which tools/rfl-env.sh sources.
#
# Re-running the script is cheap: every step is skipped if it already happened.

set -eu

CACHE=${RFL_CACHE:-$HOME/.cache/loongson3-cpufreq-rfl}
JOBS=$(nproc 2>/dev/null || echo 4)
KVER=$(uname -r)

usage() {
	cat <<EOF
Usage: $0 [options]

  --cache DIR    where to keep the toolchain and the kernel tree
                 (default: \$RFL_CACHE or ~/.cache/loongson3-cpufreq-rfl)
  --jobs N       parallel build jobs (default: \$(nproc))
  --kver VERSION kernel release to prepare for (default: \$(uname -r))
  -h, --help     show this help
EOF
}

while [ $# -gt 0 ]; do
	case $1 in
	--cache)
		CACHE=$2
		shift 2
		;;
	--jobs)
		JOBS=$2
		shift 2
		;;
	--kver)
		KVER=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		echo "error: unknown option '$1'" >&2
		usage >&2
		exit 1
		;;
	esac
done

ROOT=$CACHE/root
DEBS=$CACHE/debs
TREE=$CACHE/linux
CONF=/boot/config-$KVER

step() { printf '\n==> %s\n' "$*"; }
die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

for tool in apt-get apt-cache dpkg-deb tar make gcc; do
	command -v "$tool" >/dev/null 2>&1 || die "'$tool' is required but not installed"
done

[ -r "$CONF" ] || die "$CONF not found; is this a distribution kernel?"

mkdir -p "$CACHE" "$ROOT" "$DEBS"

# ---------------------------------------------------------------------------
# 1. Find out which kernel source and which rustc we have to match.
# ---------------------------------------------------------------------------
step "Inspecting $CONF"

KREL=${KVER%%[-+]*} # 7.2.8, from e.g. 7.2.8+deb14-loong64
KMAJOR=${KREL%%.*} # 7
KREST=${KREL#*.}
KMINOR=${KREST%%.*} # 2
KSOURCE="linux-source-$KMAJOR.$KMINOR"

WANT_RUSTC=$(sed -n 's/^CONFIG_RUSTC_VERSION_TEXT="rustc \([0-9][0-9.]*\).*/\1/p' "$CONF")
[ -n "$WANT_RUSTC" ] ||
	die "the kernel in $CONF was built without CONFIG_RUST; it cannot load Rust modules"

HAVE_RUSTC=$(LC_ALL=C apt-cache policy rustc 2>/dev/null |
	sed -n 's/^ *Candidate: \([0-9][0-9.]*\).*/\1/p')
[ -n "$HAVE_RUSTC" ] || die "no rustc package is available from apt"

if [ "$HAVE_RUSTC" != "$WANT_RUSTC" ]; then
	die "the kernel was built with rustc $WANT_RUSTC but apt offers $HAVE_RUSTC.
     Different compilers produce different mangled symbols, so a module built
     with $HAVE_RUSTC would not load.  Provide rustc $WANT_RUSTC first."
fi

echo "    kernel release : $KVER"
echo "    kernel source  : $KSOURCE"
echo "    rustc          : $WANT_RUSTC"

# ---------------------------------------------------------------------------
# 2. Download the toolchain packages.  No root needed: --download-only plus a
#    private archive cache.
# ---------------------------------------------------------------------------
if [ ! -x "$ROOT/usr/bin/rustc" ] || [ ! -x "$ROOT/usr/bin/bindgen" ] ||
	[ ! -r "$ROOT/usr/lib/rustlib/src/rust/library/core/src/lib.rs" ]; then
	step "Downloading toolchain packages into $DEBS"
	apt-get install --download-only -y --no-install-recommends \
		-o Debug::NoLocking=1 -o Dir::Cache::archives="$DEBS" \
		rustc rust-src libstd-rust-dev bindgen libclang-dev >/dev/null
fi

if ! ls "$DEBS/${KSOURCE}_"*.deb >/dev/null 2>&1 && [ ! -d "$TREE" ]; then
	step "Downloading $KSOURCE into $DEBS"
	(cd "$DEBS" && apt-get download "$KSOURCE" >/dev/null)
fi

# ---------------------------------------------------------------------------
# 3. Unpack the toolchain into a private prefix.  rustc finds its sysroot
#    relative to its own path, so the prefix is relocatable.
# ---------------------------------------------------------------------------
if [ ! -x "$ROOT/usr/bin/rustc" ]; then
	step "Unpacking toolchain into $ROOT"
	for deb in "$DEBS"/*.deb; do
		dpkg-deb -x "$deb" "$ROOT"
	done
fi

LIBCLANG_PATH=$(ls -d "$ROOT"/usr/lib/llvm-*/lib 2>/dev/null | head -n 1)
LIBCLANG_PATH=${LIBCLANG_PATH:-$ROOT/usr/lib}
RUST_LIBDIR=$(ls -d "$ROOT"/usr/lib/*-linux-gnu 2>/dev/null | head -n 1)
RUST_LIBDIR=${RUST_LIBDIR:-$ROOT/usr/lib}

# Debian installs libLLVM into the multiarch directory while libclang looks for
# it next to itself; make it visible in both places.
if [ "$RUST_LIBDIR" != "$LIBCLANG_PATH" ]; then
	cp -n "$RUST_LIBDIR"/libLLVM*.so* "$LIBCLANG_PATH/" 2>/dev/null || true
fi

RUSTC=$ROOT/usr/bin/rustc
BINDGEN=$ROOT/usr/bin/bindgen
RUSTDOC=$ROOT/usr/bin/rustdoc
RUST_LIB_SRC=$ROOT/usr/lib/rustlib/src/rust/library
LD_LIBRARY_PATH=$RUST_LIBDIR:$LIBCLANG_PATH
export LD_LIBRARY_PATH

[ -x "$RUSTC" ] || die "rustc was not unpacked correctly into $ROOT"
[ -x "$BINDGEN" ] || die "bindgen was not unpacked correctly into $ROOT"
[ -r "$RUST_LIB_SRC/core/src/lib.rs" ] || die "rust-src was not unpacked correctly"

"$RUSTC" --version || die "the unpacked rustc does not run"
"$BINDGEN" --version || die "the unpacked bindgen does not run"

# ---------------------------------------------------------------------------
# 4. Unpack and configure the kernel source tree.
# ---------------------------------------------------------------------------
if [ ! -f "$TREE/Makefile" ]; then
	step "Unpacking $KSOURCE into $TREE"
	rm -rf "$TREE"
	mkdir -p "$TREE"
	tarball=$(ls "$ROOT"/usr/src/${KSOURCE}.tar.* 2>/dev/null | head -n 1)
	[ -n "$tarball" ] || die "$KSOURCE did not contain a source tarball"
	tar -xf "$tarball" -C "$TREE" --strip-components=1
fi

LOCALVERSION=${KVER#"$KREL"}

kbuild() {
	make -C "$TREE" \
		RUSTC="$RUSTC" RUSTDOC="$RUSTDOC" RUSTFMT=true \
		RUST_LIB_SRC="$RUST_LIB_SRC" BINDGEN="$BINDGEN" \
		LOCALVERSION="$LOCALVERSION" "$@"
}

if [ ! -f "$TREE/rust/libkernel.rmeta" ]; then
	step "Configuring the kernel tree with $CONF"
	cp "$CONF" "$TREE/.config"
	kbuild olddefconfig >/dev/null

	grep -q '^CONFIG_RUST=y' "$TREE/.config" ||
		die "CONFIG_RUST got disabled while configuring; check the toolchain versions"

	step "Building the kernel's Rust support (this takes a few minutes)"
	kbuild -j"$JOBS" prepare
	kbuild -j"$JOBS" modules_prepare
else
	step "Reusing the already built kernel tree at $TREE"
	kbuild -j"$JOBS" modules_prepare >/dev/null
fi

# ---------------------------------------------------------------------------
# 5. modpost needs the symbol table of the *running* kernel.
# ---------------------------------------------------------------------------
step "Installing Module.symvers"
HDRS=$(readlink -f "/lib/modules/$KVER/build" 2>/dev/null || true)
if [ -n "$HDRS" ] && [ -r "$HDRS/Module.symvers" ]; then
	cp "$HDRS/Module.symvers" "$TREE/Module.symvers"
elif [ -r "$TREE/Module.symvers" ]; then
	echo "    keeping the existing $TREE/Module.symvers"
else
	die "no Module.symvers found; install linux-headers-$KVER or build vmlinux"
fi

# ---------------------------------------------------------------------------
# 6. Remember the environment.
# ---------------------------------------------------------------------------
cat >"$CACHE/env.sh" <<EOF
# Generated by tools/setup-rfl.sh; use it through tools/rfl-env.sh.
export KVER='$KVER'
export KDIR='$TREE'
export RUSTC='$RUSTC'
export RUSTDOC='$RUSTDOC'
export RUST_LIB_SRC='$RUST_LIB_SRC'
export BINDGEN='$BINDGEN'
export LIBCLANG_PATH='$LIBCLANG_PATH'
export LD_LIBRARY_PATH='$LD_LIBRARY_PATH'
EOF

step "Done"
echo "    kernel tree : $TREE"
echo "    toolchain   : $ROOT"
echo
echo "Now run:"
echo "    . tools/rfl-env.sh && make"
