#!/bin/sh
# Builds the `idmx` command-line tool and puts it on your PATH.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p idmx-cli
DEST="${1:-/usr/local/bin}"
if [ -w "$DEST" ]; then cp target/release/idmx "$DEST/idmx"; else sudo cp target/release/idmx "$DEST/idmx"; fi
echo "Installed: $DEST/idmx — try: idmx nets"
