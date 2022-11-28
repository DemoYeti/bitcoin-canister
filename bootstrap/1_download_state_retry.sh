#!/usr/bin/env bash
#
# Script for downloading the bitcoin state.
set -euo pipefail

BITCOIN_D=$1/bin/bitcoind

# Create a bitcoin.conf file that downloads blocks up to the given height.
CONF_FILE=$(mktemp)
cat << EOF > "$CONF_FILE"
# Reduce storage requirements by only storing most recent N MiB of block.
prune=5000

# Dummy credentials that are required by bitcoin-cli.
rpcuser=ic-btc-integration
rpcpassword=QPQiNaph19FqUsCrBRN0FII7lyM26B51fAMeBQzCb-E=
rpcauth=ic-btc-integration:cdf2741387f3a12438f69092f0fdad8e\$62081498c98bee09a0dce2b30671123fa561932992ce377585e8e08bb0c11dfa
EOF

$BITCOIN_D -conf="$CONF_FILE" -datadir="$(pwd)/data"
