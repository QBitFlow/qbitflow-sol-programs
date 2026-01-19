const anchor = require("@coral-xyz/anchor");
const {
	PublicKey,
	Keypair,
	LAMPORTS_PER_SOL,
	SystemProgram,
	Connection,
} = require("@solana/web3.js");
const {
	createMint,
	getOrCreateAssociatedTokenAccount,
	mintTo,
	getAccount,
} = require("@solana/spl-token");
const fs = require("fs");
const path = require("path");
const { bs58 } = require("@coral-xyz/anchor/dist/cjs/utils/bytes");

async function airdropSol(connection, publicKey, amount = 5) {
	console.log(`\nAirdropping ${amount} SOL to ${publicKey.toString()}...`);

	const signature = await connection.requestAirdrop(
		publicKey,
		amount * LAMPORTS_PER_SOL,
	);

	// Wait for confirmation
	await connection.confirmTransaction(signature);

	const balance = await connection.getBalance(publicKey);
	console.log(`Balance after airdrop: ${balance / LAMPORTS_PER_SOL} SOL`);

	return signature;
}

// Init the on-chain program by calling the initialize method
// This will set up the authority PDA (and authority owner) to the deployer wallet
async function initProgram(provider, coSignerPubkey) {
	const program = anchor.workspace.QbitflowPaymentSystem;

	const [authorityAccountPda, authorityAccountPdaBump] =
		PublicKey.findProgramAddressSync(
			[Buffer.from("authority")], // Seed for the authority PDA (must match the program's ini 'initialize.rs')
			program.programId,
		);

	await program.methods
		.initialize(new PublicKey(coSignerPubkey))
		.accounts({
			authority: authorityAccountPda,
			signer: provider.wallet.publicKey,
			systemProgram: SystemProgram.programId,
		})
		.signers([provider.wallet.payer])
		.rpc();

	const authAccountPda =
		await program.account.authority.fetch(authorityAccountPda);
	console.log("\nProgram Authority PDA:", authorityAccountPda.toString());
	console.log("\nProgram initialized successfully!");
}

async function main() {
	try {
		let isTest = false;
		let coSignerPubkey;

		// Load environment if provided
		if (process.argv.length === 5) {
			console.log("Loading environment variables from:", process.argv[2]);

			const envFilePath = process.argv[2];
			if (!/^\.env(\..+)?$/.test(path.basename(envFilePath))) {
				throw new Error(
					"Please provide a valid environment file (.env, .env.test, .env.production, etc.)",
				);
			}
			const pathToEnv = `../../${envFilePath}`;
			if (!fs.existsSync(pathToEnv)) {
				throw new Error(
					`The specified environment file does not exist: ${pathToEnv}`,
				);
			}

			require("dotenv").config({ path: pathToEnv });
			console.log("Environment variables loaded.");

			// whether to load test or not
			isTest = process.argv[3] === "test";

			coSignerPubkey = process.argv[4];
			if (!coSignerPubkey) {
				throw new Error(
					"Please provide the co-signer public key as the second argument",
				);
			}
		} else {
			throw new Error(
				"Please provide an environment file as an argument (.env, .env.test, .env.production, etc.)",
			);
		}

		let rpcEndpoint = process.env.SOLANA_NETWORK_URL;
		let wsEndpoint = process.env.SOLANA_WS_URL;
		let walletK = process.env.SOLANA_CONTRACT_OWNER_WALLET_PRIVATE_KEY;

		if (isTest) {
			// Configure the provider to use localhost
			console.log("Loading test environment variables...");

			rpcEndpoint = process.env.SOLANA_NETWORK_URL_TEST;
			wsEndpoint = process.env.SOLANA_WS_URL_TEST;
			walletK = process.env.SOLANA_CONTRACT_OWNER_WALLET_PRIVATE_KEY_TEST;
		}
		console.log("RPC Endpoint:", rpcEndpoint);

		const provider = new anchor.AnchorProvider(
			new Connection(rpcEndpoint, "confirmed"),
			new anchor.Wallet(Keypair.fromSecretKey(bs58.decode(walletK))),
			{ commitment: "confirmed" },
		);
		anchor.setProvider(provider);

		const connection = provider.connection;
		console.log("Connection established to Solana cluster.");
		const wallet = provider.wallet;

		console.log("Solana Local Validator Setup");
		console.log("=============================");
		console.log("Deployer Wallet:", wallet.publicKey.toString());
		console.log("RPC URL:", connection.rpcEndpoint);

		// Check initial balance
		const initialBalance = await connection.getBalance(wallet.publicKey);
		console.log(
			"Initial deployer balance:",
			initialBalance / LAMPORTS_PER_SOL,
			"SOL",
		);

		// Initialize the on-chain program
		await initProgram(provider, coSignerPubkey);

		// Final balance check
		const finalBalance = await connection.getBalance(wallet.publicKey);
		const deploymentCost =
			(initialBalance - finalBalance) / LAMPORTS_PER_SOL;

		console.log("\n=============================");
		console.log("Deployment Summary");
		console.log("=============================");
		console.log(
			"Deployer balance after setup:",
			finalBalance / LAMPORTS_PER_SOL,
			"SOL",
		);
		console.log("Setup cost:", deploymentCost, "SOL");

		console.log("\n✅ Local Solana setup complete!");
		console.log(
			"You can now interact with your deployed tokens and program.",
		);
	} catch (error) {
		console.error("❌ Setup failed:", error);
		process.exit(1);
	}
}

// Run the script
main()
	.then(() => process.exit(0))
	.catch((error) => {
		console.error(error);
		process.exit(1);
	});
