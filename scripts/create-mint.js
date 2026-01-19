const { createMint } = require("@solana/spl-token");
const base58 = require("bs58");

const fs = require("fs");
const ENV_PATH = "../../.env.development";

if (fs.existsSync(ENV_PATH)) {
	require("dotenv").config({ path: ENV_PATH });
} else {
	console.error(
		".env.development file not found. Please create it with the necessary environment variables.",
	);
	process.exit(1);
}

const { Keypair, Connection } = require("@solana/web3.js");

// Token configurations (similar to your USDC/WETH)
const TOKENS = {
	// 4Z5YhW7ygQ4bxpvR8mnwiSJ71HAWCHzfFFPcj2TMjDW8
	USDC: {
		name: "USD Coin",
		symbol: "USDC",
		decimals: 6, // Real USDC uses 6 decimals
	},
	// 8D6gS6KC2TnfPg2DhZCsLiAAGRCZ9R5CGEToD1Dcm1nH
	WSOL: {
		name: "Wrapped SOL",
		symbol: "WSOL",
		decimals: 9, // Common for Solana tokens
	},
};

const create = async (keypair, tokenConfig) => {
	const connection = new Connection(process.env.SOLANA_NETWORK_URL_TEST);

	// Create mint account
	const mint = await createMint(
		connection,
		keypair, // Payer of the transaction
		keypair.publicKey, // Mint authority
		keypair.publicKey, // Freeze authority (optional)
		tokenConfig.decimals, // Decimals
	);

	return mint;
};

const main = () => {
	const authorityPrivateKey =
		process.env.SOLANA_CONTRACT_OWNER_WALLET_PRIVATE_KEY_TEST;
	if (!authorityPrivateKey) {
		console.error(
			"Please set the SOLANA_CONTRACT_OWNER_WALLET_PRIVATE_KEY_TEST environment variable.",
		);
		process.exit(1);
	}

	const authoritySecretKey = Uint8Array.from(
		base58.decode(authorityPrivateKey),
	);
	const USER_WALLET = Keypair.fromSecretKey(authoritySecretKey);
	console.log("Using authority wallet:", USER_WALLET.publicKey.toBase58());

	// Create mints for each token
	for (const tokenKey in TOKENS) {
		const tokenConfig = TOKENS[tokenKey];
		create(USER_WALLET, tokenConfig)
			.then((mintAddress) => {
				console.log(
					`Created mint for ${tokenConfig.symbol} at address: ${mintAddress.toBase58()}`,
				);
			})
			.catch((err) => {
				console.error(
					`Failed to create mint for ${tokenConfig.symbol}: ${err}`,
				);
			});
	}
};

main();
