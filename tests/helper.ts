import BN from "bn.js";
import {
	PublicKey,
	Keypair,
	SystemProgram,
	ComputeBudgetProgram,
	Ed25519Program,
	TransactionInstruction,
	SYSVAR_INSTRUCTIONS_PUBKEY,
} from "@solana/web3.js";
import { createHash } from "crypto";

import * as anchor from "@coral-xyz/anchor";

import { Program } from "@coral-xyz/anchor";
import { QbitflowPaymentSystem } from "../target/types/qbitflow_payment_system";

import {
	TOKEN_PROGRAM_ID,
	ASSOCIATED_TOKEN_PROGRAM_ID,
	getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

// SettleMode discriminant — must match the Rust enum in state.rs.
export enum SettleMode {
	Regular = 0,
	X402 = 1,
	MerchantX402 = 2,
}

// Domain tags — must match constants.rs byte-for-byte.
export const BATCH_ATTESTATION_HASH_DOMAIN = Buffer.from("qbf:x402-att:v1", "utf-8");

// sha256 matches the on-chain `anchor_lang::solana_program::hash::hashv` primitive used
// for the attestation / report message digests.
const sha256 = (...parts: Buffer[]): Buffer => {
	const h = createHash("sha256");
	for (const p of parts) h.update(p);
	return h.digest();
};

const u64Le = (n: BN | number | bigint): Buffer => {
	const bn = BN.isBN(n) ? n : new BN(n.toString());
	return bn.toArrayLike(Buffer, "le", 8);
};

/**
 * Build the message a subscriber signs to attest to an x402 batch:
 *   sha256( DOMAIN || uuid || first_nonce || last_nonce || total_amount || merkle_root ).
 * Matches the on-chain `hashv(&[BATCH_ATTESTATION_HASH_DOMAIN, ...])` in settle.rs.
 */
export const batchAttestationMessage = (
	uuid: number[],
	firstNonce: BN,
	lastNonce: BN,
	totalAmount: BN,
	merkleRoot: Buffer,
): Buffer => {
	if (merkleRoot.length !== 32) throw new Error("merkleRoot must be 32 bytes");
	return sha256(
		BATCH_ATTESTATION_HASH_DOMAIN,
		Buffer.from(uuid),
		u64Le(firstNonce),
		u64Le(lastNonce),
		u64Le(totalAmount),
		merkleRoot,
	);
};

/**
 * Build an Ed25519Program precompile instruction that signs `message` with `signer`.
 * The runtime verifies the signature before our handler runs; the handler then matches
 * the precompile's embedded pubkey + message against what it expects.
 */
export const ed25519Sig = (
	signer: Keypair,
	message: Buffer,
): TransactionInstruction => {
	return Ed25519Program.createInstructionWithPrivateKey({
		privateKey: signer.secretKey,
		message,
	});
};

export const INSTRUCTIONS_SYSVAR = SYSVAR_INSTRUCTIONS_PUBKEY;

// Test helper types
interface TestAccounts {
	authority: Keypair;
	payer: Keypair;
	merchant: Keypair;
	subscriber: Keypair;
	organizationRecipient: Keypair;
	referrer: Keypair;
}

interface TokenAccounts {
	authorityTokenAccount: PublicKey;
	tokenMint: PublicKey;
	payerTokenAccount: PublicKey;
	merchantTokenAccount: PublicKey;
	subscriberTokenAccount: PublicKey;
	organizationTokenAccount: PublicKey;
	referrerTokenAccount: PublicKey;
}

interface PaymentTestParams {
	authority?: PublicKey; // Optional authority override for testing
	amount: BN;
	feeBps: number;
	organizationFeeBps: number;
	referralFeeBps?: number; // 0 / omitted ⇒ no referrer
	expectError?: string;
	useMinimumFee?: boolean;
	useUnauthorizedFeeRecipient?: boolean;
	organizationFeeIsZero?: boolean;
	noOrganizationRecipient?: boolean;
	noReferrer?: boolean; // even if referralFeeBps > 0, omit the account (negative test)
	overrideReferrerEqualsOwner?: boolean; // for duplicate-recipient negative test
	overrideReferrerEqualsOrg?: boolean;
	displayTxDetails?: boolean;
	wrongAuthorityOwnerSigner?: boolean;
	priorityFee?: boolean;
}

const SOL_USD = 230;
const TOKEN_USD = 1; // 1 USD per token (USDC)
const TOKEN_PRICE_IN_LAMPORTS = new BN((SOL_USD / TOKEN_USD) * 1e6);

// MIN_FEE_FOR_CONTRACT_BPS in constants.rs (0.50%) — handler bumps fee_bps up to this
// floor before applying.
const MIN_FEE_FOR_CONTRACT_BPS = 50;
const FEE_DENOMINATOR = 10000;

export const getComputeRefundData = (
	numSignatures: number,
	priorityFee: boolean = false,
) => {
	const baseFee = new BN(5000 * numSignatures); // base fee for transaction, based on the number of signatures
	const computeUnits = 200_000; // Estimated compute units for the transaction (here is the default max)

	let totalFee: BN;
	if (priorityFee) {
		// 100 micro-lamports per compute unit. Use integer math to avoid the FP-tail
		// (100 * 1e-6 * 200_000 = 20.000000000000004 in IEEE 754) that turns into a
		// 1-lamport off-by-one in the token-refund conversion.
		const priorityFeeLamports = new BN(100).mul(new BN(computeUnits)).div(new BN(1_000_000));
		totalFee = baseFee.add(priorityFeeLamports);
	} else {
		totalFee = baseFee;
	}

	return {
		tokenPriceInLamports: TOKEN_PRICE_IN_LAMPORTS, // in lamports (see currency.GetTokenPriceInMainCurrency), USDC has 6 decimals
		computeCostInLamports: totalFee, // 0.000005 SOL per compute unit
	};
};

export const getComputeRefundInTokens = (totalFeeInLamports: BN) => {
	// Ceiling division — must match the on-chain compute_refund() rounding semantics
	// (programs/qbitflow_payment_system/src/instructions/compute_refund.rs).
	const product = totalFeeInLamports.mul(TOKEN_PRICE_IN_LAMPORTS);
	return product.add(new BN(999_999_999)).div(new BN(1_000_000_000));
};

// Test helper class
class TestHelper {
	constructor(
		private provider: anchor.AnchorProvider,
		private program: Program<QbitflowPaymentSystem>,
		private accounts: TestAccounts,
		private tokenAccounts: TokenAccounts,
		private authorityAccountPda: PublicKey,
	) {}

	getDefaultAccountsForPayment(tokenAccounts: boolean) {
		if (tokenAccounts) {
			return {
				authority: this.authorityAccountPda,
				authorityOwner: this.accounts.authority.publicKey,
				payer: this.accounts.payer.publicKey,
				merchant: this.accounts.merchant.publicKey,
				organization: this.accounts.organizationRecipient.publicKey,
				mint: this.tokenAccounts.tokenMint,
				tokenProgram: TOKEN_PROGRAM_ID,
				systemProgram: SystemProgram.programId,
				associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
			};
		}

		// SOL accounts
		return {
			authority: this.authorityAccountPda,
			payer: this.accounts.payer.publicKey,
			authorityOwner: this.accounts.authority.publicKey,
			feeRecipient: this.accounts.authority.publicKey,
			merchant: this.accounts.merchant.publicKey,
			organizationFeeRecipient: null as PublicKey | null,
			referrer: null as PublicKey | null,
			systemProgram: SystemProgram.programId,
		};
	}

	getDefaultAccountsForSubscription(
		uuid: number[],
		type: "create" | "execute" | "cancel" | "force-cancel",
	) {
		const [permitRegistry] = PublicKey.findProgramAddressSync(
			[
				Buffer.from("permit_registry"),
				this.accounts.payer.publicKey.toBuffer(),
				this.tokenAccounts.tokenMint.toBuffer(),
			],
			this.program.programId,
		);

		const [subscriptionPda] = PublicKey.findProgramAddressSync(
			[
				Buffer.from("subscription"),
				this.accounts.payer.publicKey.toBuffer(),
				Buffer.from(uuid),
			],
			this.program.programId,
		);
		switch (type) {
			case "create":
				return {
					authority: this.authorityAccountPda,
					authorityOwner: this.accounts.authority.publicKey,
					permitRegistry,
					subscription: subscriptionPda,
					subscriber: this.accounts.payer.publicKey,
					subscriberTokenAccount: this.tokenAccounts.payerTokenAccount,
					merchant: this.accounts.merchant.publicKey,
					merchantTokenAccount: this.tokenAccounts.merchantTokenAccount,
					feeRecipientTokenAccount: this.tokenAccounts.authorityTokenAccount,
					organization: this.accounts.organizationRecipient.publicKey,
					organizationTokenAccount: this.tokenAccounts.organizationTokenAccount,
					referrer: null as PublicKey | null,
					referrerTokenAccount: null as PublicKey | null,
					mint: this.tokenAccounts.tokenMint,
					tokenProgram: TOKEN_PROGRAM_ID,
					systemProgram: SystemProgram.programId,
					associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
				};
			case "execute":
				return {
					authority: this.authorityAccountPda,
					authorityOwner: this.accounts.authority.publicKey,
					permitRegistry,
					subscription: subscriptionPda,
					subscriber: this.accounts.payer.publicKey,
					subscriberTokenAccount: this.tokenAccounts.payerTokenAccount,
					merchantTokenAccount: this.tokenAccounts.merchantTokenAccount,
					mint: this.tokenAccounts.tokenMint,
					feeRecipientTokenAccount: this.tokenAccounts.authorityTokenAccount,
					organizationTokenAccount: this.tokenAccounts.organizationTokenAccount,
					referrerTokenAccount: null as PublicKey | null,
					tokenProgram: TOKEN_PROGRAM_ID,
					systemProgram: SystemProgram.programId,
					associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
				};
			case "cancel":
				return {
					authority: this.authorityAccountPda,
					owner: this.accounts.authority.publicKey,
					permitRegistry,
					subscription: subscriptionPda,
					subscriber: this.accounts.payer.publicKey,
					mint: this.tokenAccounts.tokenMint,
				};
			case "force-cancel":
				return {
					authority: this.authorityAccountPda,
					authorityOwner: this.accounts.authority.publicKey,
					subscriber: this.accounts.payer.publicKey,
					permitRegistry,
					subscription: subscriptionPda,
					mint: this.tokenAccounts.tokenMint,
				};
		}
	}

	generateUuid(): number[] {
		return Array.from({ length: 16 }, () =>
			Math.floor(Math.random() * 256),
		);
	}

	async getBalances(isToken: boolean = false) {
		if (isToken) {
			return {
				payer: Number(
					(
						await getAccount(
							this.provider.connection,
							this.tokenAccounts.payerTokenAccount,
						)
					).amount,
				),
				merchant: Number(
					(
						await getAccount(
							this.provider.connection,
							this.tokenAccounts.merchantTokenAccount,
						)
					).amount,
				),
				feeRecipient: Number(
					(
						await getAccount(
							this.provider.connection,
							this.tokenAccounts.authorityTokenAccount,
						)
					).amount,
				),
				organizationRecipient: Number(
					(
						await getAccount(
							this.provider.connection,
							this.tokenAccounts.organizationTokenAccount,
						)
					).amount,
				),
				referrer: Number(
					(
						await getAccount(
							this.provider.connection,
							this.tokenAccounts.referrerTokenAccount,
						)
					).amount,
				),
			};
		}
		return {
			payer: await this.provider.connection.getBalance(
				this.accounts.payer.publicKey,
			),
			merchant: await this.provider.connection.getBalance(
				this.accounts.merchant.publicKey,
			),
			feeRecipient: await this.provider.connection.getBalance(
				this.accounts.authority.publicKey,
			),
			organizationRecipient: await this.provider.connection.getBalance(
				this.accounts.organizationRecipient.publicKey,
			),
			referrer: await this.provider.connection.getBalance(
				this.accounts.referrer.publicKey,
			),
		};
	}

	calculateExpectedAmounts(params: PaymentTestParams) {
		const effectiveFeeBps =
			params.useMinimumFee && params.feeBps < MIN_FEE_FOR_CONTRACT_BPS
				? MIN_FEE_FOR_CONTRACT_BPS
				: params.feeBps;

		const grossFeeAmount = Math.floor(
			(params.amount.toNumber() * effectiveFeeBps) / FEE_DENOMINATOR,
		);
		const referralFeeAmount =
			params.referralFeeBps && params.referralFeeBps > 0
				? Math.floor(
						(grossFeeAmount * params.referralFeeBps) / FEE_DENOMINATOR,
					)
				: 0;
		const feeAmount = grossFeeAmount - referralFeeAmount;
		const orgFeeAmount = params.organizationFeeIsZero
			? 0
			: Math.floor(
					((params.amount.toNumber() - grossFeeAmount) *
						params.organizationFeeBps) /
						FEE_DENOMINATOR,
				);
		const remainingAmount =
			params.amount.toNumber() - grossFeeAmount - orgFeeAmount;

		return { feeAmount, orgFeeAmount, referralFeeAmount, remainingAmount };
	}

	async processSOLPayment(params: PaymentTestParams) {
		const uuid = this.generateUuid();

		// Pick the referrer pubkey for the negative-tests; default to the dedicated referrer
		let referrerKey: PublicKey | null = null;
		if ((params.referralFeeBps ?? 0) > 0) {
			if (params.overrideReferrerEqualsOwner) {
				referrerKey = this.accounts.authority.publicKey;
			} else if (params.overrideReferrerEqualsOrg) {
				referrerKey = this.accounts.organizationRecipient.publicKey;
			} else if (!params.noReferrer) {
				referrerKey = this.accounts.referrer.publicKey;
			}
		}

		const accounts: any = {
			...this.getDefaultAccountsForPayment(false),
			authority: params.authority || this.authorityAccountPda,
			authorityOwner: params.wrongAuthorityOwnerSigner
				? this.accounts.payer.publicKey
				: this.accounts.authority.publicKey,
			feeRecipient: params.useUnauthorizedFeeRecipient
				? this.accounts.payer.publicKey
				: this.accounts.authority.publicKey,
			organizationFeeRecipient: params.noOrganizationRecipient
				? null
				: this.accounts.organizationRecipient.publicKey,
			referrer: referrerKey,
		};

		const tx = await this.program.methods
			.processPayment(params.amount, uuid, {
				platformFeeBps: params.feeBps,
				organizationFeeBps: params.organizationFeeBps,
				referralFeeBps: params.referralFeeBps ?? 0,
				// Future deadline so the referral cut is paid when referralFeeBps > 0
				// (the deadline gate lives in calculate_referral_fee).
				referralDeadline: new BN(Math.floor(Date.now() / 1000) + 3600),
			})
			.accounts(accounts)
			.transaction();

		tx.recentBlockhash = (
			await this.provider.connection.getLatestBlockhash()
		).blockhash;
		tx.feePayer = this.accounts.payer.publicKey;

		return await this.provider.sendAndConfirm(
			tx,
			[
				params.wrongAuthorityOwnerSigner
					? this.accounts.merchant
					: this.accounts.authority,
				this.accounts.payer,
			],
			{
				skipPreflight: false,
				commitment: "confirmed",
			},
		);
	}

	async processTokenPayment(params: PaymentTestParams) {
		const uuid = this.generateUuid();

		let referrerKey: PublicKey | null = null;
		let referrerAta: PublicKey | null = null;
		if ((params.referralFeeBps ?? 0) > 0) {
			if (params.overrideReferrerEqualsOwner) {
				referrerKey = this.accounts.authority.publicKey;
				referrerAta = this.tokenAccounts.authorityTokenAccount;
			} else if (params.overrideReferrerEqualsOrg) {
				referrerKey = this.accounts.organizationRecipient.publicKey;
				referrerAta = this.tokenAccounts.organizationTokenAccount;
			} else if (!params.noReferrer) {
				referrerKey = this.accounts.referrer.publicKey;
				referrerAta = this.tokenAccounts.referrerTokenAccount;
			}
		}

		const accounts: any = {
			...this.getDefaultAccountsForPayment(true),
			authorityOwner: params.wrongAuthorityOwnerSigner
				? this.accounts.payer.publicKey
				: this.accounts.authority.publicKey,
			feeRecipientTokenAccount: params.useUnauthorizedFeeRecipient
				? this.tokenAccounts.payerTokenAccount
				: this.tokenAccounts.authorityTokenAccount,
			organizationTokenAccount: params.noOrganizationRecipient
				? null
				: this.tokenAccounts.organizationTokenAccount,
			referrer: referrerKey,
			referrerTokenAccount: referrerAta,
		};

		const tx = await this.program.methods
			.processTokenPayment(
				params.amount,
				uuid,
				{
					platformFeeBps: params.feeBps,
					organizationFeeBps: params.organizationFeeBps,
					referralFeeBps: params.referralFeeBps ?? 0,
					// Future deadline so the referral cut is paid when referralFeeBps > 0.
					referralDeadline: new BN(Math.floor(Date.now() / 1000) + 3600),
				},
				getComputeRefundData(2, params.priorityFee),
			)
			.accounts(accounts)
			.transaction();

		const modifyComputeUnits = ComputeBudgetProgram.setComputeUnitLimit({
			units: 200_000,
		});
		const addPriorityFee = ComputeBudgetProgram.setComputeUnitPrice({
			microLamports: 100,
		});

		tx.add(modifyComputeUnits);
		if (params.priorityFee) {
			tx.add(addPriorityFee);
		}

		tx.recentBlockhash = (
			await this.provider.connection.getLatestBlockhash()
		).blockhash;
		tx.feePayer = this.accounts.authority.publicKey;

		return await this.provider.sendAndConfirm(
			tx,
			[
				params.wrongAuthorityOwnerSigner
					? this.accounts.merchant
					: this.accounts.authority,
				this.accounts.payer,
			],
			{
				skipPreflight: false,
				commitment: "confirmed",
			},
		);
	}

	async testPayment(params: PaymentTestParams, isToken: boolean = false) {
		const balancesBefore = await this.getBalances(isToken);
		let tx: string | undefined;
		let error: any;

		try {
			tx = isToken
				? await this.processTokenPayment(params)
				: await this.processSOLPayment(params);
		} catch (err) {
			if (!params.expectError) console.log("Caught error:", err);
			error = err;
		}

		if (params.expectError) {
			expect(error).to.exist;
			expect(error.message).to.include(params.expectError);
			return;
		}

		expect(error).to.not.exist;
		expect(tx).to.exist;

		const balancesAfter = await this.getBalances(isToken);
		const expectedAmounts = this.calculateExpectedAmounts(params);

		let gasFees = 0;
		if (tx) {
			const txDetails = await this.provider.connection.getTransaction(tx, {
				commitment: "confirmed",
			});
			if (params.displayTxDetails) {
				console.log("Transaction details:", txDetails);
			}
			gasFees = txDetails?.meta?.fee || 0;
		}

		expect(balancesAfter.merchant).to.equal(
			balancesBefore.merchant + expectedAmounts.remainingAmount,
		);
		expect(balancesAfter.organizationRecipient).to.equal(
			balancesBefore.organizationRecipient + expectedAmounts.orgFeeAmount,
		);
		expect(balancesAfter.referrer).to.equal(
			balancesBefore.referrer + expectedAmounts.referralFeeAmount,
		);

		const expectedFeeRecipientBalance =
			balancesBefore.feeRecipient + expectedAmounts.feeAmount;

		if (!isToken) {
			expect(balancesAfter.feeRecipient).to.equal(
				expectedFeeRecipientBalance,
			);
			expect(balancesAfter.payer).to.equal(
				balancesBefore.payer - params.amount.toNumber() - gasFees,
			);
		} else {
			const computeData = getComputeRefundData(2, params.priorityFee);
			const computeRefundValueInTokens = getComputeRefundInTokens(
				computeData.computeCostInLamports,
			);
			expect(balancesAfter.feeRecipient).to.equal(
				expectedFeeRecipientBalance +
					computeRefundValueInTokens.toNumber(),
			);
			expect(balancesAfter.payer).to.equal(
				balancesBefore.payer -
					params.amount.toNumber() -
					computeRefundValueInTokens.toNumber(),
			);
		}
	}
}

export { TestHelper, TestAccounts, TokenAccounts, PaymentTestParams };
