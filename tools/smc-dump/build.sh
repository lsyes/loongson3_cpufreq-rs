#!/bin/sh
# Build the SMC dump diagnostic against the prepared kernel tree.
cd "$(dirname "$(readlink -f "$0")")" || exit 1
. ../rfl-env.sh
KV=$(make -s -C "$KDIR" kernelversion)
exec make -C "$KDIR" M="$PWD" \
	RUSTC="$RUSTC" BINDGEN="$BINDGEN" RUST_LIB_SRC="$RUST_LIB_SRC" \
	LOCALVERSION="${KVER#"$KV"}" modules
