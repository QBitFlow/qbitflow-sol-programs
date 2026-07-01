#!/bin/bash
set -euo pipefail

LEDGER=/root/test-ledger
RPC_URL=http://127.0.0.1:8899

# Start validator with logs visible
solana-test-validator \
	--reset \
	--ledger "$LEDGER" \
	--rpc-port 8899 \
	--faucet-port 9900 \
	--dynamic-port-range 8000-8030 \
	--log > /tmp/validator.log 2>&1 &
VALIDATOR_PID=$!

# Tail logs in background so you see what's happening
# tail -F /tmp/validator.log &

echo "Waiting for Solana validator..."
for i in {1..60}; do
	if solana cluster-version --url "$RPC_URL" > /dev/null 2>&1; then
		echo "Solana validator is ready"
		break
	fi
	if ! kill -0 $VALIDATOR_PID 2>/dev/null; then
		echo "ERROR: validator died. Last logs:"
		tail -n 100 /tmp/validator.log
		exit 1
	fi
	sleep 2
done

# Verify it actually came up
solana cluster-version --url "$RPC_URL" || {
	echo "ERROR: validator never became ready"
	tail -n 200 /tmp/validator.log
	exit 1
}

solana config set --keypair /root/.config/solana/id-test.json
solana config set --url "$RPC_URL"

solana airdrop 100 --url "$RPC_URL"
anchor program deploy --provider.cluster localnet

node scripts/deploy-local.js .env.development EunN8jvRH8LCYfErWb9f26KiTKCdM82fXwEfGU8rkiAF

echo "Solana programs deployed successfully"

wait $VALIDATOR_PID