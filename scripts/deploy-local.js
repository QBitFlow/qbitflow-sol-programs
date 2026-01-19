const anchor = require("@coral-xyz/anchor");
const {
	PublicKey,
	Keypair,
	LAMPORTS_PER_SOL,
	SystemProgram,
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

// Token configurations (similar to your USDC/WETH)
const TOKENS = {
	USDC: {
		name: "Mock USDC",
		symbol: "USDC",
		decimals: 6, // Real USDC uses 6 decimals
		mintAmount: 1000, // 1000 USDC
	},
	WSOL: {
		name: "Wrapped SOL",
		symbol: "WSOL",
		decimals: 9, // Common for Solana tokens
		mintAmount: 50, // 50 WSOL
	},
};

// Create a new wallet for the user

// Check if ../tests/accounts/user.json exists
let USER_WALLET;
if (fs.existsSync(path.join(__dirname, "../tests/accounts/user.json"))) {
	const userKeypairData = JSON.parse(
		fs.readFileSync(path.join(__dirname, "../tests/accounts/user.json")),
	);
	USER_WALLET = Keypair.fromSecretKey(new Uint8Array(userKeypairData));
} else {
	USER_WALLET = Keypair.generate();
	fs.writeFileSync(
		path.join(__dirname, "../tests/accounts/user.json"),
		JSON.stringify(Array.from(USER_WALLET.secretKey)),
	);
	console.log(
		"Generated new user wallet and saved to ../tests/accounts/user.json",
	);
}

// Check if ../tests/accounts/merchant.json exists
let MERCHANT_WALLET;
if (fs.existsSync(path.join(__dirname, "../tests/accounts/merchant.json"))) {
	const merchantKeypairData = JSON.parse(
		fs.readFileSync(
			path.join(__dirname, "../tests/accounts/merchant.json"),
		),
	);
	MERCHANT_WALLET = Keypair.fromSecretKey(
		new Uint8Array(merchantKeypairData),
	);
} else {
	MERCHANT_WALLET = Keypair.generate();
	fs.writeFileSync(
		path.join(__dirname, "../tests/accounts/merchant.json"),
		JSON.stringify(Array.from(MERCHANT_WALLET.secretKey)),
	);
	console.log(
		"Generated new merchant wallet and saved to ../tests/accounts/merchant.json",
	);
}

const displayKeyPair = (keypair, name) => {
	console.log(`${name} Public Key:`, keypair.publicKey.toString());
	console.log(
		`${name} Private Key:`,
		bs58.encode(Uint8Array.from(keypair.secretKey)),
	);
};

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

async function createAndMintToken(connection, payer, tokenConfig, recipient) {
	console.log(`\nCreating and minting ${tokenConfig.name}...`);

	// Create mint account
	const mint = await createMint(
		connection,
		payer, // Payer of the transaction
		payer.publicKey, // Mint authority
		payer.publicKey, // Freeze authority (optional)
		tokenConfig.decimals, // Decimals
	);

	console.log(`${tokenConfig.symbol} mint created: ${mint.toString()}`);

	// Get or create associated token account for recipient
	const recipientTokenAccount = await getOrCreateAssociatedTokenAccount(
		connection,
		payer, // Payer
		mint, // Mint
		recipient, // Owner
	);

	// Mint tokens to recipient
	const mintAmount =
		tokenConfig.mintAmount * Math.pow(10, tokenConfig.decimals);
	await mintTo(
		connection,
		payer, // Payer
		mint, // Mint
		recipientTokenAccount.address, // Destination
		payer.publicKey, // Mint authority
		mintAmount, // Amount
	);

	// Check balance
	const accountInfo = await getAccount(
		connection,
		recipientTokenAccount.address,
	);
	const balance =
		Number(accountInfo.amount) / Math.pow(10, tokenConfig.decimals);

	console.log(`${tokenConfig.symbol} minted to ${recipient.toString()}`);
	console.log(
		`${tokenConfig.symbol} balance: ${balance} ${tokenConfig.symbol}`,
	);
	console.log(
		`${tokenConfig.symbol} token account: ${recipientTokenAccount.address.toString()}`,
	);

	return {
		mint: mint.toString(),
		tokenAccount: recipientTokenAccount.address.toString(),
	};
}

// Init the on-chain program by calling the initialize method
// This will set up the authority PDA (and authority owner) to the deployer wallet
async function initProgram(provider, coSignerPubkey) {
	const program = anchor.workspace.QbitflowPaymentSystem;

	console.log("\nInitializing on-chain program...");
	console.log("Co-signer Public Key:", coSignerPubkey);

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
		let coSignerPubkey;
		// Load environment if provided
		if (process.argv.length === 4) {
			const envFilePath = process.argv[2];
			if (!/^\.env(\..+)?$/.test(path.basename(envFilePath))) {
				throw new Error(
					"Please provide a valid environment file (.env, .env.test, .env.production, etc.)",
				);
			}
			const pathToEnv = `../../${envFilePath}`;
			require("dotenv").config({ path: pathToEnv });

			coSignerPubkey = process.argv[3];
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

		// Configure the provider to use localhost
		const provider = anchor.AnchorProvider.local("http://127.0.0.1:8899");
		anchor.setProvider(provider);

		const connection = provider.connection;
		const wallet = provider.wallet;

		console.log("Solana Local Validator Setup");
		console.log("=============================");
		console.log("RPC URL:", connection.rpcEndpoint);

		// Check initial balance
		const initialBalance = await connection.getBalance(wallet.publicKey);
		console.log(
			"Initial deployer balance:",
			initialBalance / LAMPORTS_PER_SOL,
			"SOL",
		);

		// Airdrop SOL to deployer if needed
		if (initialBalance < 2 * LAMPORTS_PER_SOL) {
			await airdropSol(connection, wallet.publicKey, 10);
		}

		// Airdrop SOL to user wallet
		await airdropSol(connection, USER_WALLET.publicKey, 5);

		// Airdrop SOL to merchant wallet
		await airdropSol(connection, MERCHANT_WALLET.publicKey, 1);

		// Initialize the on-chain program
		await initProgram(provider, coSignerPubkey);

		// Create and mint tokens
		const tokenAddresses = {};
		const userTokenAccounts = {};

		for (const [symbol, config] of Object.entries(TOKENS)) {
			const result = await createAndMintToken(
				connection,
				wallet.payer,
				config,
				USER_WALLET.publicKey,
			);

			tokenAddresses[symbol] = result.mint;
			userTokenAccounts[`${symbol}_TOKEN_ACCOUNT`] = result.tokenAccount;
		}

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

		// Prepare addresses object
		const addresses = {
			DEPLOYER_WALLET: wallet.publicKey.toString(),
			USER_WALLET: USER_WALLET.publicKey.toString(),
			RPC_URL: connection.rpcEndpoint,
			...tokenAddresses,
			...userTokenAccounts,
		};

		// Print all addresses
		console.log("\nDeployed Addresses:");
		console.log("==================");
		Object.entries(addresses).forEach(([key, value]) => {
			console.log(`${key}: ${value}`);
		});

		// Save addresses to file
		const outputPath = path.join(__dirname, "../deployed-addresses.json");
		fs.writeFileSync(outputPath, JSON.stringify(addresses, null, 2));
		console.log(`\nAddresses saved to: ${outputPath}`);

		// Display keypairs
		displayKeyPair(wallet.payer, "Deployer Wallet");
		displayKeyPair(USER_WALLET, "User Wallet");
		displayKeyPair(MERCHANT_WALLET, "Merchant Wallet");

		// Load and display your program ID if available
		// try {
		// 	const programKeypairPath = path.join(__dirname, "../target/deploy");
		// 	const files = fs.readdirSync(programKeypairPath);
		// 	const keypairFile = files.find((f) => f.endsWith("-keypair.json"));

		// 	if (keypairFile) {
		// 		const keypairData = JSON.parse(
		// 			fs.readFileSync(path.join(programKeypairPath, keypairFile)),
		// 		);
		// 		const programKeypair = Keypair.fromSecretKey(
		// 			new Uint8Array(keypairData),
		// 		);
		// 		console.log(
		// 			`\nYour Program ID: ${programKeypair.publicKey.toString()}`,
		// 		);
		// 		addresses.PROGRAM_ID = programKeypair.publicKey.toString();
		// 	}
		// } catch (error) {
		// 	console.log(
		// 		"\nCouldn't load program ID (this is normal if program isn't deployed yet)",
		// 	);
		// }

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
