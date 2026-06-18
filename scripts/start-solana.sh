#!/bin/bash
set -e

# Start local validator in background
solana-test-validator \
	--reset \
	--bind-address 0.0.0.0 \
	--rpc-port 8899 &
VALIDATOR_PID=$!

# Wait for validator to be ready
echo "Waiting for Solana validator..."
until solana cluster-version --url http://localhost:8899 > /dev/null 2>&1; do
	sleep 1
done
echo "Solana validator is ready"

# Deploy programs
solana config set --keypair ~/.config/solana/id-test.json
solana config set --url http://localhost:8899



solana airdrop 100 --url http://localhost:8899
anchor deploy --provider.cluster localnet

# Run deploy script
node scripts/deploy-local.js .env.development BRMmBD5m2xBGyBr7b7zeUHbiWtPAZxbpVh1EyAYdqkG

echo "Solana programs deployed successfully"


# Keep container running
wait $VALIDATOR_PID