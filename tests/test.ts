import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
	ASSOCIATED_TOKEN_PROGRAM_ID,
	createAssociatedTokenAccount,
	createMint,
	getAccount,
	mintTo,
	revoke,
	TOKEN_PROGRAM_ID
} from "@solana/spl-token";
import {
	Keypair,
	LAMPORTS_PER_SOL,
	PublicKey,
	SystemProgram
} from "@solana/web3.js";
import BN from "bn.js";
import { expect } from "chai";
import { QbitflowPaymentSystem } from "../target/types/qbitflow_payment_system";
import {
	batchAttestationMessage,
	ed25519Sig,
	getComputeRefundData,
	getComputeRefundInTokens,
	INSTRUCTIONS_SYSVAR,
	PaymentTestParams,
	SettleMode,
	TestAccounts,
	TestHelper,
	TokenAccounts,
} from "./helper";

// TODO: before executing the tests, lower MIN_FREQUENCY in constants.rs to 5 (5 seconds)
// so subscription-execution / referral-deadline tests can advance through real time without
// waiting 7 days.

const coSignerKeyPair = Keypair.generate();

describe("QbitFlow Payment System", async () => {
	const provider = anchor.AnchorProvider.local();
	anchor.setProvider(provider);

	const program = anchor.workspace
		.QbitflowPaymentSystem as Program<QbitflowPaymentSystem>;

	const [authorityAccountPda, authorityAccountPdaBump] =
		PublicKey.findProgramAddressSync(
			[Buffer.from("authority")],
			program.programId,
		);

	let accounts: TestAccounts;
	let tokenAccounts: TokenAccounts;
	let testHelper: TestHelper;

	const INITIAL_TOKEN_SUPPLY = 1_000_000;
	const FEE_BPS = 200; // 2%
	const ORG_FEE_BPS = 100; // 1%
	const FEE_DENOMINATOR_TS = 10000;
	const FREQUENCY = 5; // seconds — relies on MIN_FREQUENCY override (see TODO above)

	// Build the `Fee` arg struct (mirrors the on-chain `Fee` — field names are the camelCase
	// of the Rust struct).
	const mkFee = (
		platformFeeBps: number,
		organizationFeeBps: number = 0,
		referralFeeBps: number = 0,
		referralDeadline: BN = new BN(0),
	) => ({ platformFeeBps, organizationFeeBps, referralFeeBps, referralDeadline });

	const subscriptionPdaFor = (uuid: number[], subscriber: PublicKey) =>
		PublicKey.findProgramAddressSync(
			[
				Buffer.from("subscription"),
				subscriber.toBuffer(),
				Buffer.from(uuid),
			],
			program.programId,
		)[0];

	const permitRegistryPdaFor = (subscriber: PublicKey, mint: PublicKey) =>
		PublicKey.findProgramAddressSync(
			[
				Buffer.from("permit_registry"),
				subscriber.toBuffer(),
				mint.toBuffer(),
			],
			program.programId,
		)[0];

	before(async () => {
		accounts = {
			authority: provider.wallet.payer,
			payer: Keypair.generate(),
			merchant: Keypair.generate(),
			subscriber: Keypair.generate(),
			organizationRecipient: Keypair.generate(),
			referrer: Keypair.generate(),
		};

		// Airdrop SOL
		for (const account of Object.values(accounts)) {
			await provider.connection.requestAirdrop(
				account.publicKey,
				10 * LAMPORTS_PER_SOL,
			);
		}
		await new Promise((resolve) => setTimeout(resolve, 2000));

		const tokenMint = await createMint(
			provider.connection,
			accounts.authority,
			accounts.authority.publicKey,
			null,
			6,
		);

		tokenAccounts = {
			tokenMint,
			payerTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.payer,
				tokenMint,
				accounts.payer.publicKey,
			),
			merchantTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.merchant,
				tokenMint,
				accounts.merchant.publicKey,
			),
			subscriberTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.subscriber,
				tokenMint,
				accounts.subscriber.publicKey,
			),
			authorityTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.authority,
				tokenMint,
				accounts.authority.publicKey,
			),
			organizationTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.organizationRecipient,
				tokenMint,
				accounts.organizationRecipient.publicKey,
			),
			referrerTokenAccount: await createAssociatedTokenAccount(
				provider.connection,
				accounts.referrer,
				tokenMint,
				accounts.referrer.publicKey,
			),
		};

		await mintTo(
			provider.connection,
			accounts.authority,
			tokenMint,
			tokenAccounts.payerTokenAccount,
			accounts.authority,
			INITIAL_TOKEN_SUPPLY * 1_000_000,
		);
		await mintTo(
			provider.connection,
			accounts.authority,
			tokenMint,
			tokenAccounts.subscriberTokenAccount,
			accounts.authority,
			INITIAL_TOKEN_SUPPLY * 1_000_000,
		);

		testHelper = new TestHelper(
			provider,
			program,
			accounts,
			tokenAccounts,
			authorityAccountPda,
		);
	});

	describe("Initialization", () => {
		it("Should initialize the payment system", async () => {
			await program.methods
				.initialize(coSignerKeyPair.publicKey)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					signer: provider.wallet.publicKey,
					systemProgram: SystemProgram.programId,
				})
				.signers([provider.wallet.payer])
				.rpc();

			const authAccountPda =
				await program.account.authority.fetch(authorityAccountPda);
			expect(authAccountPda.owner.toString()).to.equal(
				accounts.authority.publicKey.toString(),
			);
			expect(authAccountPda.bump).to.equal(authorityAccountPdaBump);
			expect(authAccountPda.coSigner.toString()).to.equal(
				coSignerKeyPair.publicKey.toString(),
			);
		});

		it("Should not allow re-initialization", async () => {
			try {
				await program.methods
					.initialize(coSignerKeyPair.publicKey)
					.accounts({
						// @ts-ignore
						authority: authorityAccountPda,
						signer: provider.wallet.publicKey,
						systemProgram: SystemProgram.programId,
					})
					.signers([provider.wallet.payer])
					.rpc();
				expect.fail("Should have thrown an error");
			} catch (error) {
				expect(error.message).to.include("already in use");
			}
		});
	});

	describe("SOL Payments", () => {
		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 100));
		});

		const testCases: Array<{ name: string; params: PaymentTestParams }> = [
			{
				name: "Should process a SOL payment successfully",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
				},
			},
			{
				name: "Should reject zero amount payments",
				params: {
					amount: new BN(0),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					expectError: "ZeroAmount",
				},
			},
			{
				name: "Should reject invalid fee percentages",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: 1501, // just over the 15% MAX_FEE_PLATFORM_BPS cap
					organizationFeeBps: ORG_FEE_BPS,
					expectError: "InvalidFeePercentage",
				},
			},
			{
				name: "Should apply minimum fee when fee is below threshold",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: 25, // below MIN_FEE_FOR_CONTRACT_BPS (50)
					organizationFeeBps: 0,
					organizationFeeIsZero: true,
					useMinimumFee: true,
				},
			},
			{
				name: "Should reject unauthorized fee recipient",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					useUnauthorizedFeeRecipient: true,
					expectError: "Unauthorized",
				},
			},
			{
				name: "Should work if organization fee is zero",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: 0,
					organizationFeeIsZero: true,
				},
			},
			{
				name: "Should work if organization fee recipient is not provided but organization fee is zero",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: 0,
					organizationFeeIsZero: true,
					noOrganizationRecipient: true,
				},
			},
			{
				name: "Should reject if authority owner signer is incorrect",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					wrongAuthorityOwnerSigner: true,
					expectError: "unknown signer",
				},
			},
			{
				name: "Should reject org fee > 0 with no organization recipient",
				params: {
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					noOrganizationRecipient: true,
					expectError: "MissingOrganizationAccount",
				},
			},
		];

		testCases.forEach(({ name, params }) => {
			it(name, async () => {
				await testHelper.testPayment(params, false);
			});
		});
	});

	describe("Token Payments", () => {
		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 100));
		});

		const testCases: Array<{ name: string; params: PaymentTestParams }> = [
			{
				name: "Should process a token payment successfully",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
				},
			},
			{
				name: "Should process a token payment successfully and reimburse authority owner for compute fees",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					priorityFee: true,
				},
			},
			{
				name: "Should reject zero amount token payments",
				params: {
					amount: new BN(0),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					expectError: "ZeroAmount",
				},
			},
			{
				name: "Should reject invalid fee percentages for token payments",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: 1501, // just over the 15% MAX_FEE_PLATFORM_BPS cap
					organizationFeeBps: ORG_FEE_BPS,
					expectError: "InvalidFeePercentage",
				},
			},
			{
				name: "Should apply minimum fee for token payments when fee is below threshold",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: 25,
					organizationFeeBps: 0,
					organizationFeeIsZero: true,
					useMinimumFee: true,
				},
			},
			{
				name: "Should reject unauthorized fee recipient for token payments",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					useUnauthorizedFeeRecipient: true,
					expectError: "ConstraintTokenOwner",
				},
			},
			{
				name: "Should work for token payments if organization fee is zero",
				params: {
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: 0,
					organizationFeeIsZero: true,
				},
			},
		];

		testCases.forEach(({ name, params }) => {
			it(name, async () => {
				await testHelper.testPayment(params, true);
			});
		});
	});

	describe("Subscriptions", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const noReferralArgs = () => ({
			referralFeeBps: 0,
			referralDeadline: new BN(0),
		});

		const createSub = async (
			args: {
				uuid: number[];
				amount?: BN;
				maxAmount?: BN;
				frequency?: number;
				feeBps?: number;
				organizationFeeBps?: number;
				allowance?: BN;
				mode?: SettleMode;
				sessionSigner?: PublicKey;
				referralFeeBps?: number;
				referralDeadline?: BN;
				referrer?: PublicKey | null;
				referrerTokenAccount?: PublicKey | null;
			} & Record<string, any>,
		) => {
			// Defaults sized to leave room for the compute refund (~2300 token-units per
			// call with the test token price). max_amount - amount must always cover that.
			const amount = args.amount ?? new BN(1000);
			const maxAmount = args.maxAmount ?? new BN(50_000);
			const frequency = args.frequency ?? FREQUENCY;
			const feeBps = args.feeBps ?? FEE_BPS;
			const organizationFeeBps = args.organizationFeeBps ?? 0;
			const allowance = args.allowance ?? new BN(200_000);
			const mode = args.mode ?? SettleMode.Regular;
			const sessionSigner = args.sessionSigner ?? PublicKey.default;
			const referralFeeBps = args.referralFeeBps ?? 0;
			const referralDeadline = args.referralDeadline ?? new BN(0);
			const refundData = getComputeRefundData(2);

			const ctxAccounts = testHelper.getDefaultAccountsForSubscription(
				args.uuid,
				"create",
			);
			(ctxAccounts as any).referrer = args.referrer ?? null;
			(ctxAccounts as any).referrerTokenAccount =
				args.referrerTokenAccount ?? null;

			return program.methods
				.createSubscription(
					args.uuid,
					amount,
					maxAmount,
					frequency,
					{
						platformFeeBps: feeBps,
						organizationFeeBps,
						referralFeeBps,
						referralDeadline,
					},
					allowance,
					refundData,
					mode,
					sessionSigner,
				)
				.accounts(ctxAccounts as any)
				.signers([accounts.payer])
				.rpc();
		};

		const executeSub = async (
			args: {
				uuid: number[];
				amount?: BN;
				frequency?: number;
				feeBps?: number;
				organizationFeeBps?: number;
				mode?: SettleMode;
				sessionSigner?: PublicKey;
				referralFeeBps?: number;
				referralDeadline?: BN;
				referrerTokenAccount?: PublicKey | null;
			},
		) => {
			const amount = args.amount ?? new BN(1000);
			const frequency = args.frequency ?? FREQUENCY;
			const feeBps = args.feeBps ?? FEE_BPS;
			const organizationFeeBps = args.organizationFeeBps ?? 0;
			const mode = args.mode ?? SettleMode.Regular;
			const sessionSigner = args.sessionSigner ?? PublicKey.default;
			const referralFeeBps = args.referralFeeBps ?? 0;
			const referralDeadline = args.referralDeadline ?? new BN(0);
			const refundData = getComputeRefundData(1);

			const ctxAccounts = testHelper.getDefaultAccountsForSubscription(
				args.uuid,
				"execute",
			);
			(ctxAccounts as any).referrerTokenAccount =
				args.referrerTokenAccount ?? null;

			return program.methods
				.executeSubscription(
					amount,
					args.uuid,
					frequency,
					{
						platformFeeBps: feeBps,
						organizationFeeBps,
						referralFeeBps,
						referralDeadline,
					},
					refundData,
					mode,
					sessionSigner,
				)
				.accounts(ctxAccounts as any)
				.signers([accounts.authority])
				.rpc();
		};

		it("Should create a subscription (Regular bills cycle 1 atomically)", async () => {
			const FIRST_AMOUNT = 1000; // createSub default amount, billed at create for Regular
			const firstFee = (FIRST_AMOUNT * FEE_BPS) / 10000; // 20
			const balancesBefore = await testHelper.getBalances(true);
			const tx = await createSub({ uuid, allowance: new BN(1_000_000) });
			expect(tx).to.exist;

			const subscription = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(subscription.subscriber.toString()).to.equal(
				accounts.payer.publicKey.toString(),
			);
			expect(subscription.allowance.toNumber()).to.equal(1_000_000);
			expect(subscription.maxAmountPerCycle.toNumber()).to.equal(50_000);
			expect(subscription.mode).to.equal(SettleMode.Regular);
			expect(subscription.stopped).to.equal(false);
			// Regular bills the first cycle in-create (parity with EVM), so the next bill is a full
			// period out, not `now`.
			expect(subscription.nextPaymentDue.toNumber()).to.be.greaterThan(
				Math.floor(Date.now() / 1000),
			);
			// Referral terms (fee bps / deadline / referrer ATA) are committed only in the
			// subscription_hash digest, NOT stored as account fields.

			const refundData = getComputeRefundData(2);
			const refundValueInTokens = getComputeRefundInTokens(
				refundData.computeCostInLamports,
			).toNumber();
			// used_allowance = first-cycle bill + the create-tx compute refund.
			expect(subscription.usedAllowance.toNumber()).to.equal(
				FIRST_AMOUNT + refundValueInTokens,
			);
			expect(subscription.currentCycleUsed.toNumber()).to.equal(
				FIRST_AMOUNT + refundValueInTokens,
			);

			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.merchant).to.equal(
				balancesBefore.merchant + FIRST_AMOUNT - firstFee,
			);
			expect(balancesAfter.feeRecipient).to.equal(
				balancesBefore.feeRecipient + firstFee + refundValueInTokens,
			);
			expect(balancesAfter.payer).to.equal(
				balancesBefore.payer - FIRST_AMOUNT - refundValueInTokens,
			);
		});

		it("Should reject creating two subscriptions with same uuid (already in use)", async () => {
			await createSub({ uuid });
			try {
				await createSub({ uuid });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/already in use/);
			}
		});

		it("Should execute subscription, moving funds + increasing usage", async () => {
			const allowance = new BN(200_000);
			const refundDataCreation = getComputeRefundData(2);
			const refundValueCreation = getComputeRefundInTokens(
				refundDataCreation.computeCostInLamports,
			).toNumber();
			const createAmount = 1000;
			await createSub({
				uuid,
				amount: new BN(createAmount),
				maxAmount: new BN(50_000),
				allowance,
			});
			// Regular create bills cycle 1 atomically; wait out the period before the next execute.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			// Snapshot AFTER create so the deltas below measure the execute leg only.
			const balancesBefore = await testHelper.getBalances(true);
			const execAmount = 5000;
			const refundData = getComputeRefundData(1);
			const refundValueInTokens = getComputeRefundInTokens(
				refundData.computeCostInLamports,
			).toNumber();
			await executeSub({ uuid, amount: new BN(execAmount) });

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			// used_allowance is cumulative: create bill + create refund + execute + execute refund.
			expect(sub.usedAllowance.toNumber()).to.equal(
				createAmount + refundValueCreation + execAmount + refundValueInTokens,
			);
			expect(sub.allowance.toNumber()).to.equal(allowance.toNumber());

			const expectedFees = 0.02 * execAmount; // 2%
			const expectedOrgFees = 0;
			const remainingAmount = execAmount - expectedFees - expectedOrgFees;

			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.payer).to.equal(
				balancesBefore.payer - execAmount - refundValueInTokens,
			);
			expect(balancesAfter.merchant).to.equal(
				balancesBefore.merchant + remainingAmount,
			);
			expect(balancesAfter.feeRecipient).to.equal(
				balancesBefore.feeRecipient + expectedFees + refundValueInTokens,
			);
		});

		it("Should transfer funds to the organization recipient if org fee > 0", async () => {
			await createSub({ uuid, organizationFeeBps: ORG_FEE_BPS });
			// Regular create bills cycle 1; wait out the period before the next execute.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			// Snapshot AFTER create so the deltas measure the execute leg only.
			const balancesBefore = await testHelper.getBalances(true);
			const execAmount = 5000;
			const refundData = getComputeRefundData(1);
			const refundValueInTokens = getComputeRefundInTokens(
				refundData.computeCostInLamports,
			).toNumber();
			await executeSub({
				uuid,
				amount: new BN(execAmount),
				organizationFeeBps: ORG_FEE_BPS,
			});

			const expectedFees = 0.02 * execAmount;
			const expectedOrgFees = (execAmount - expectedFees) * 0.01;
			const remainingAmount = execAmount - expectedFees - expectedOrgFees;

			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.payer).to.equal(
				balancesBefore.payer - execAmount - refundValueInTokens,
			);
			expect(balancesAfter.merchant).to.equal(
				balancesBefore.merchant + remainingAmount,
			);
			expect(balancesAfter.organizationRecipient).to.equal(
				balancesBefore.organizationRecipient + expectedOrgFees,
			);
		});

		// Regression for bug 1e: execute_subscription must RESET current_cycle_used to this
		// period's spend (parity with EVM `_settle`), not accumulate it across periods. The old
		// code accumulated, which would slowly choke the per-cycle refund headroom.
		it("execute resets current_cycle_used each period (no cross-period accumulation)", async () => {
			await createSub({
				uuid,
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(500_000),
			});
			const refundPerExec = getComputeRefundInTokens(
				getComputeRefundData(1).computeCostInLamports,
			).toNumber();

			// Regular create billed cycle 1; wait out the period before the first execute.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			await executeSub({ uuid, amount: new BN(3000) });
			// Roll into the next period.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			await executeSub({ uuid, amount: new BN(7000) });

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			// Only the LAST period's spend (7000 + its refund) — not 3000 + 7000 + both refunds.
			expect(sub.currentCycleUsed.toNumber()).to.equal(7000 + refundPerExec);
		});

		it("Should reject with hash mismatch if wrong frequency on execute", async () => {
			await createSub({ uuid });
			// Wait out the first period so execute passes the period gate and reaches the hash check.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await executeSub({ uuid, frequency: FREQUENCY * 2 });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}
		});

		it("Should cancel a Regular subscription while the next payment is in the future", async () => {
			// Regular create bills cycle 1 and sets next_payment_due = now + frequency, so the sub
			// can be cancelled immediately (before the next bill is due).
			await createSub({ uuid });

			await program.methods
				.cancelSubscription(uuid)
				.accounts(
					testHelper.getDefaultAccountsForSubscription(uuid, "cancel") as any,
				)
				.signers([accounts.payer])
				.rpc();

			try {
				await program.account.subscription.fetch(
					subscriptionPdaFor(uuid, accounts.payer.publicKey),
				);
				expect.fail("Subscription should have been closed");
			} catch (err: any) {
				expect(err.message).to.match(/Account does not exist/);
			}
		});

		it("Should reject cancel once the next payment is due (CannotCancelActiveSubscription)", async () => {
			await createSub({ uuid });
			// Let the period elapse so next_payment_due is now in the past — the subscriber can no
			// longer dodge the due bill via cancel; they must let it settle or be force-cancelled.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await program.methods
					.cancelSubscription(uuid)
					.accounts(
						testHelper.getDefaultAccountsForSubscription(uuid, "cancel") as any,
					)
					.signers([accounts.payer])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/CannotCancelActiveSubscription/);
			}
		});

		it("Force cancel should work for authority owner", async () => {
			await createSub({ uuid });

			await program.methods
				.forceCancelSubscription(uuid)
				.accounts(
					testHelper.getDefaultAccountsForSubscription(
						uuid,
						"force-cancel",
					) as any,
				)
				.signers([])
				.rpc();

			try {
				await program.account.subscription.fetch(
					subscriptionPdaFor(uuid, accounts.payer.publicKey),
				);
				expect.fail("Subscription should have been closed");
			} catch (err: any) {
				expect(err.message).to.match(/Account does not exist/);
			}
		});

		it("Should reject zero-amount execute", async () => {
			await createSub({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await executeSub({ uuid, amount: new BN(0) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/ZeroAmount/);
			}
		});

		it("Should reject amount exceeding max_amount", async () => {
			// Create with max=10000 (room for the create-time refund of ~2300 token-units).
			// Execute amount=20000 then trips the max_amount guard on execute.
			await createSub({ uuid, amount: new BN(1000), maxAmount: new BN(10_000) });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await executeSub({ uuid, amount: new BN(20_000) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/MaxAmountExceeded/);
			}
		});

		it("Should reject when compute_cost_in_lamports == 0 (refund must be real)", async () => {
			await createSub({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			const refundData = getComputeRefundData(1);
			refundData.computeCostInLamports = new BN(0);
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(ctx as any).referrerTokenAccount = null;
			try {
				await program.methods
					.executeSubscription(
						new BN(1000), uuid, FREQUENCY, mkFee(FEE_BPS),
						refundData,
						SettleMode.Regular, PublicKey.default,
					)
					.accounts(ctx as any)
					.signers([accounts.authority])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidComputeRefundParams/);
			}
		});

		it("Should reject when token_price_in_lamports == 0 (refund must be real)", async () => {
			await createSub({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			const refundData = getComputeRefundData(1);
			refundData.tokenPriceInLamports = new BN(0);
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(ctx as any).referrerTokenAccount = null;
			try {
				await program.methods
					.executeSubscription(
						new BN(1000), uuid, FREQUENCY, mkFee(FEE_BPS),
						refundData,
						SettleMode.Regular, PublicKey.default,
					)
					.accounts(ctx as any)
					.signers([accounts.authority])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidComputeRefundParams/);
			}
		});

		// ---- create-time validation ----

		it("Should reject create with frequency below MIN_FREQUENCY", async () => {
			try {
				await createSub({ uuid, frequency: 4 }); // MIN_FREQUENCY is 5 in test builds
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidFrequency/);
			}
		});

		it("Should reject create with zero amount", async () => {
			try {
				await createSub({ uuid, amount: new BN(0) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/ZeroAmount/);
			}
		});

		it("Should reject create when max_amount <= amount", async () => {
			try {
				await createSub({ uuid, amount: new BN(1000), maxAmount: new BN(1000) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidAmount/);
			}
		});

		it("Should reject create when allowance <= max_amount", async () => {
			try {
				await createSub({
					uuid,
					amount: new BN(1000),
					maxAmount: new BN(10_000),
					allowance: new BN(9_999),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidAmount/);
			}
		});

		it("Should reject execute exceeding the remaining allowance (InsufficientAllowance)", async () => {
			// allowance just above max; the create-time refund eats into it, so a near-max
			// execute pushes used_allowance past the cap.
			await createSub({
				uuid,
				amount: new BN(1000),
				maxAmount: new BN(10_000),
				allowance: new BN(10_001),
			});
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await executeSub({ uuid, amount: new BN(9000) }); // < max, but > remaining allowance
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InsufficientAllowance/);
			}
		});
	});

	describe("X402 Batch Settlement", () => {
		let uuid: number[];
		// Off-chain session signer the merchant uses to mint x402 receipts. Bound at create.
		const sessionSigner = Keypair.generate();

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const settleAccounts = (uuid: number[]) => {
			return testHelper.getDefaultAccountsForSubscription(uuid, "execute");
		};

		const createX402 = async (
			args: {
				uuid: number[];
				amount?: BN;
				maxAmount?: BN;
				frequency?: number;
				allowance?: BN;
				feeBps?: number;
				orgFeeBps?: number;
				sessionSignerPubkey?: PublicKey;
			} = {} as any,
		) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(args.uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					args.uuid,
					args.amount ?? new BN(1000),
					args.maxAmount ?? new BN(50_000),
					args.frequency ?? FREQUENCY,
					mkFee(args.feeBps ?? FEE_BPS, args.orgFeeBps ?? 0),
					args.allowance ?? new BN(200_000),
					getComputeRefundData(2),
					SettleMode.X402,
					args.sessionSignerPubkey ?? sessionSigner.publicKey,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const settleX402 = async (
			args: {
				uuid: number[];
				amount: BN;
				firstNonce: BN;
				lastNonce: BN;
				merkleRoot?: Buffer;
				sessionSignerPubkey?: PublicKey;
			},
		) => {
			const root = args.merkleRoot ?? Buffer.alloc(32, 0x42);
			const tx = program.methods.settleX402Batch(
				args.uuid,
				args.amount,
				mkFee(FEE_BPS),
				FREQUENCY,
				args.sessionSignerPubkey ?? sessionSigner.publicKey,
				args.firstNonce,
				args.lastNonce,
				Array.from(root),
				getComputeRefundData(1),
			).accounts({
				...settleAccounts(args.uuid),
				referrerTokenAccount: null,
			} as any);

			return tx.signers([accounts.authority]).rpc();
		};

		it("Should persist mode + next_nonce on the subscription account (session_signer lives only in the hash)", async () => {
			await createX402({ uuid });
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.mode).to.equal(SettleMode.X402);
			expect(sub.nextNonce.toNumber()).to.equal(0);
			// session_signer and session_signer_version are NOT on the struct — they're
			// committed only via subscription_hash. Off-chain readers reconstruct from
			// the SubscriptionCreated event.
		});

		it("Should reject execute_subscription on an X402 sub (WrongSettleMode)", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(ctx as any).referrerTokenAccount = null;
			try {
				await program.methods
					.executeSubscription(
						new BN(1000),
						uuid,
						FREQUENCY,
						mkFee(FEE_BPS),
						getComputeRefundData(1),
						SettleMode.X402, // hash would match, but the mode check above runs first
						sessionSigner.publicKey,
					)
					.accounts(ctx as any)
					.signers([accounts.authority])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/WrongSettleMode/);
			}
		});

		it("Should reject settle before the first period elapses (SettleTooEarly)", async () => {
			await createX402({ uuid });
			try {
				await settleX402({ uuid, amount: new BN(5000), firstNonce: new BN(0), lastNonce: new BN(9) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/SettleTooEarly/);
			}
		});

		it("Should settle a batch (no attestation) and advance the nonce window", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			const balancesBefore = await testHelper.getBalances(true);
			const refundData = getComputeRefundData(1);
			const refundValue = getComputeRefundInTokens(refundData.computeCostInLamports).toNumber();

			await settleX402({
				uuid,
				amount: new BN(5000),
				firstNonce: new BN(0),
				lastNonce: new BN(9),
			});

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(10);
			// Period-roll reset: current_cycle_used == this settle's total spend (amount + refund).
			expect(sub.currentCycleUsed.toNumber()).to.equal(5000 + refundValue);

			const expectedFees = 0.02 * 5000;
			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.merchant).to.equal(balancesBefore.merchant + 5000 - expectedFees);
			expect(balancesAfter.feeRecipient).to.equal(
				balancesBefore.feeRecipient + expectedFees + refundValue,
			);
		});

		it("Should accept first_nonce > next_nonce (relayer skipped burned nonces)", async () => {
			// With the loosened nonce check, the relayer can settle a batch that starts
			// past the current next_nonce — useful when individual receipts were lost.
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await settleX402({
				uuid,
				amount: new BN(5000),
				firstNonce: new BN(50), // started past 0
				lastNonce: new BN(99),
			});

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(100); // last_nonce + 1
		});

		it("Should reject when first_nonce < next_nonce (InvalidNonceRange — would rewind)", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			// Settle to advance next_nonce past 0
			await settleX402({
				uuid,
				amount: new BN(5000),
				firstNonce: new BN(0),
				lastNonce: new BN(9),
			});
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			// next_nonce is now 10; attempt to settle starting at 5 (rewind) → reverts.
			try {
				await settleX402({
					uuid,
					amount: new BN(5000),
					firstNonce: new BN(5),
					lastNonce: new BN(15),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidNonceRange/);
			}
		});

		it("Should reject when last_nonce < first_nonce (InvalidNonceRange)", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			try {
				await settleX402({
					uuid,
					amount: new BN(5000),
					firstNonce: new BN(5),
					lastNonce: new BN(2),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidNonceRange/);
			}
		});

		it("Two settles in the same period are blocked by SettleTooEarly", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await settleX402({ uuid, amount: new BN(1000), firstNonce: new BN(0), lastNonce: new BN(0) });
			try {
				await settleX402({ uuid, amount: new BN(1000), firstNonce: new BN(1), lastNonce: new BN(1) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/SettleTooEarly/);
			}
		});

		it("Cancel mid-period → next settle finalizes and closes the subscription", async () => {
			await createX402({ uuid });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await program.methods
				.cancelPayAsYouGoSubscription(uuid)
				.accounts(
					testHelper.getDefaultAccountsForSubscription(uuid, "cancel") as any,
				)
				.signers([accounts.payer])
				.rpc();

			// Subscription is still open but `stopped = true`.
			const subBefore = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(subBefore.stopped).to.equal(true);

			await settleX402({ uuid, amount: new BN(1000), firstNonce: new BN(0), lastNonce: new BN(0) });

			try {
				await program.account.subscription.fetch(
					subscriptionPdaFor(uuid, accounts.payer.publicKey),
				);
				expect.fail("Subscription should have been closed");
			} catch (err: any) {
				expect(err.message).to.match(/Account does not exist/);
			}
		});
	});

	describe("MerchantX402 Settlement", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const createMX402 = async () => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					uuid,
					new BN(1000),
					new BN(50_000),
					FREQUENCY,
					mkFee(FEE_BPS),
					new BN(200_000),
					getComputeRefundData(2),
					SettleMode.MerchantX402,
					PublicKey.default, // sessionSigner committed at create (merchant signs reports off-chain)
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		// MerchantX402 batch settle. Same machinery as settleX402Batch. On-chain authorization is
		// the subscription digest + per-cycle cap + nonce range (no batch-settle attestation).
		const settleMX402 = async (
			args: {
				amount: BN;
				firstNonce: BN;
				lastNonce: BN;
				merkleRoot?: Buffer;
			},
		) => {
			const root = args.merkleRoot ?? Buffer.alloc(32, 0x55);
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			const tx = program.methods.settleMerchantX402Batch(
				uuid,
				args.amount,
				mkFee(FEE_BPS),
				FREQUENCY,
				PublicKey.default, // sessionSigner — must match what was bound at create
				args.firstNonce,
				args.lastNonce,
				Array.from(root),
				getComputeRefundData(1),
			).accounts({
				...ctx,
				referrerTokenAccount: null,
			} as any);

			return tx.signers([accounts.authority]).rpc();
		};

		it("Should settle a merchant batch (no attestation) and advance the nonce window", async () => {
			await createMX402();
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			const balancesBefore = await testHelper.getBalances(true);
			await settleMX402({ amount: new BN(5000), firstNonce: new BN(0), lastNonce: new BN(2) });

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(3); // last_nonce + 1

			const expectedFees = 0.02 * 5000;
			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.merchant).to.equal(balancesBefore.merchant + 5000 - expectedFees);
		});

		it("Should accept firstNonce > next_nonce (relayer skipped burned nonces)", async () => {
			await createMX402();
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await settleMX402({ amount: new BN(5000), firstNonce: new BN(5), lastNonce: new BN(7) });

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(8); // last_nonce + 1
		});

		it("Should reject firstNonce < next_nonce (InvalidNonceRange — would rewind)", async () => {
			await createMX402();
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await settleMX402({ amount: new BN(5000), firstNonce: new BN(3), lastNonce: new BN(3) });
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			// next_nonce is now 4; attempt firstNonce=2 (rewind) → reverts.
			try {
				await settleMX402({ amount: new BN(5000), firstNonce: new BN(2), lastNonce: new BN(2) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidNonceRange/);
			}
		});

		it("Two settles in the same period are blocked by SettleTooEarly", async () => {
			await createMX402();
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			await settleMX402({ amount: new BN(1000), firstNonce: new BN(0), lastNonce: new BN(0) });
			try {
				await settleMX402({ amount: new BN(1000), firstNonce: new BN(1), lastNonce: new BN(1) });
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/SettleTooEarly/);
			}
		});
	});

	describe("Session Signer Rotation", () => {
		let uuid: number[];
		const sessionSigner = Keypair.generate();
		const newSessionSigner = Keypair.generate();

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const createX402 = async () => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					uuid,
					new BN(1000),
					new BN(50_000),
					FREQUENCY,
					mkFee(FEE_BPS),
					new BN(200_000),
					getComputeRefundData(2),
					SettleMode.X402,
					sessionSigner.publicKey,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const rotateAccounts = () => ({
			authority: authorityAccountPda,
			authorityOwner: accounts.authority.publicKey,
			subscriber: accounts.payer.publicKey,
			subscription: subscriptionPdaFor(uuid, accounts.payer.publicKey),
			authoritySigner: accounts.payer.publicKey,
			merchantTokenAccount: tokenAccounts.merchantTokenAccount,
			subscriberTokenAccount: tokenAccounts.payerTokenAccount,
			organizationTokenAccount: tokenAccounts.organizationTokenAccount,
			referrerTokenAccount: null as PublicKey | null,
			// Settle-only — fill in for atomic-settle case
			mint: null as PublicKey | null,
			permitRegistry: null as PublicKey | null,
			feeRecipientTokenAccount: null as PublicKey | null,
			instructionsSysvar: null as PublicKey | null,
			tokenProgram: null as PublicKey | null,
		});

		// Atomic rotate-and-settle helper (X402). Subscriber (payer) attests the batch summary.
		const atomicRotate = (args: {
			oldSigner: PublicKey;
			newSigner: PublicKey;
			amount: BN;
			firstNonce: BN;
			lastNonce: BN;
			attestSigner?: Keypair;
			root?: Buffer;
		}) => {
			const root = args.root ?? Buffer.alloc(32, 0x77);
			const attestSigner = args.attestSigner ?? accounts.payer;
			const msg = batchAttestationMessage(
				uuid, args.firstNonce, args.lastNonce, args.amount, root,
			);
			const accountsForRotate = {
				authority: authorityAccountPda,
				authorityOwner: accounts.authority.publicKey,
				subscriber: accounts.payer.publicKey,
				subscription: subscriptionPdaFor(uuid, accounts.payer.publicKey),
				authoritySigner: accounts.payer.publicKey,
				merchantTokenAccount: tokenAccounts.merchantTokenAccount,
				subscriberTokenAccount: tokenAccounts.payerTokenAccount,
				organizationTokenAccount: tokenAccounts.organizationTokenAccount,
				referrerTokenAccount: null,
				mint: tokenAccounts.tokenMint,
				permitRegistry: permitRegistryPdaFor(accounts.payer.publicKey, tokenAccounts.tokenMint),
				feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
				instructionsSysvar: INSTRUCTIONS_SYSVAR,
				tokenProgram: TOKEN_PROGRAM_ID,
			};
			const settleArgs = {
				amount: args.amount,
				firstNonce: args.firstNonce,
				lastNonce: args.lastNonce,
				merkleRoot: Array.from(root),
				attestationIxIndex: 0,
				computeRefundParams: getComputeRefundData(1),
			};
			return program.methods
				.rotateSessionSigner(
					uuid, FREQUENCY, mkFee(FEE_BPS), SettleMode.X402,
					args.oldSigner, args.newSigner, settleArgs as any,
				)
				.preInstructions([ed25519Sig(attestSigner, msg)])
				.accounts(accountsForRotate as any)
				.signers([accounts.authority, accounts.payer])
				.rpc();
		};

		it("Plain rotate updates the hash; subsequent settle uses NEW signer", async () => {
			await createX402();

			await program.methods
				.rotateSessionSigner(
					uuid,
					FREQUENCY,
					mkFee(FEE_BPS),
					SettleMode.X402,
					sessionSigner.publicKey,
					newSessionSigner.publicKey,
					null, // plain rotation, no atomic settle
				)
				.accounts(rotateAccounts() as any)
				.signers([accounts.authority, accounts.payer])
				.rpc();

			// Try to settle with OLD signer in the hash → mismatch
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			const exec = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(exec as any).referrerTokenAccount = null;
			try {
				const root = Buffer.alloc(32, 0x42);
				await program.methods.settleX402Batch(
					uuid, new BN(1000), mkFee(FEE_BPS), FREQUENCY,
					sessionSigner.publicKey, // OLD signer — hash mismatch
					new BN(0), new BN(0), Array.from(root),
					getComputeRefundData(1),
				).accounts({
					...exec,
					referrerTokenAccount: null,
				} as any).signers([accounts.authority]).rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}

			// Settling with the NEW signer succeeds
			const root = Buffer.alloc(32, 0x42);
			await program.methods.settleX402Batch(
				uuid, new BN(1000), mkFee(FEE_BPS), FREQUENCY,
				newSessionSigner.publicKey, // NEW signer — hash matches
				new BN(0), new BN(0), Array.from(root),
				getComputeRefundData(1),
			).accounts({
				...exec,
				referrerTokenAccount: null,
			} as any).signers([accounts.authority]).rpc();
		});

		it("Rotation with WRONG old signer reverts", async () => {
			await createX402();
			const wrongOldSigner = Keypair.generate();
			try {
				await program.methods
					.rotateSessionSigner(
						uuid,
						FREQUENCY,
						mkFee(FEE_BPS),
						SettleMode.X402,
						wrongOldSigner.publicKey, // wrong
						newSessionSigner.publicKey,
						null,
					)
					.accounts(rotateAccounts() as any)
					.signers([accounts.authority, accounts.payer])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}
		});

		it("Non-subscriber cannot rotate", async () => {
			await createX402();
			const intruder = Keypair.generate();
			const ctx = rotateAccounts() as any;
			ctx.subscriber = intruder.publicKey;
			ctx.authoritySigner = intruder.publicKey;
			try {
				await program.methods
					.rotateSessionSigner(
						uuid,
						FREQUENCY,
						mkFee(FEE_BPS),
						SettleMode.X402,
						sessionSigner.publicKey,
						newSessionSigner.publicKey,
						null,
					)
					.accounts(ctx)
					.signers([accounts.authority, intruder])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				// PDA seeds derivation fails before address constraint when subscriber is swapped.
				expect(err.message).to.match(/ConstraintSeeds|Unauthorized|unknown signer|AccountNotInitialized/);
			}
		});

		it("Atomic rotate-and-settle: settles before period elapses, then rotates", async () => {
			await createX402();
			// Do NOT wait for the period — atomic settle bypasses the period guard.

			const root = Buffer.alloc(32, 0x77);
			const amount = new BN(2000);
			const firstNonce = new BN(0);
			const lastNonce = new BN(4);
			const attestationMessage = batchAttestationMessage(
				uuid, firstNonce, lastNonce, amount, root,
			);

			const accountsForRotate = {
				authority: authorityAccountPda,
				authorityOwner: accounts.authority.publicKey,
				subscriber: accounts.payer.publicKey,
				subscription: subscriptionPdaFor(uuid, accounts.payer.publicKey),
				authoritySigner: accounts.payer.publicKey,
				merchantTokenAccount: tokenAccounts.merchantTokenAccount,
				subscriberTokenAccount: tokenAccounts.payerTokenAccount,
				organizationTokenAccount: tokenAccounts.organizationTokenAccount,
				referrerTokenAccount: null,
				mint: tokenAccounts.tokenMint,
				permitRegistry: permitRegistryPdaFor(accounts.payer.publicKey, tokenAccounts.tokenMint),
				feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
				instructionsSysvar: INSTRUCTIONS_SYSVAR,
				tokenProgram: TOKEN_PROGRAM_ID,
			};

			const settleArgs = {
				amount,
				firstNonce,
				lastNonce,
				merkleRoot: Array.from(root),
				attestationIxIndex: 0,
				computeRefundParams: getComputeRefundData(1),
			};

			await program.methods
				.rotateSessionSigner(
					uuid,
					FREQUENCY,
					mkFee(FEE_BPS),
					SettleMode.X402,
					sessionSigner.publicKey,
					newSessionSigner.publicKey,
					settleArgs as any,
				)
				.preInstructions([ed25519Sig(accounts.payer, attestationMessage)])
				.accounts(accountsForRotate as any)
				.signers([accounts.authority, accounts.payer])
				.rpc();

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(5); // last_nonce + 1
		});

		// Regression for the double-count bug: the atomic settle must add (amount + refund) to
		// current_cycle_used EXACTLY ONCE. The old code added `amount` first, then `amount +
		// refund` again, so the counter ended at 2*amount + refund.
		it("Atomic settle counts current_cycle_used once (no double-count)", async () => {
			await createX402();
			const amount = new BN(2000);
			const refund = getComputeRefundInTokens(
				getComputeRefundData(1).computeCostInLamports,
			).toNumber();
			await atomicRotate({
				oldSigner: sessionSigner.publicKey,
				newSigner: newSessionSigner.publicKey,
				amount,
				firstNonce: new BN(0),
				lastNonce: new BN(4),
			});
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			// Exactly amount + refund — NOT 2*amount + refund.
			expect(sub.currentCycleUsed.toNumber()).to.equal(amount.toNumber() + refund);
		});

		// Two atomic rotate-settles in the SAME period accumulate (the period is never rolled),
		// and the cumulative cap is meaningful because each settle counts only once.
		it("Two atomic settles in one period accumulate current_cycle_used correctly", async () => {
			await createX402();
			const sig1 = Keypair.generate();
			const refund = getComputeRefundInTokens(
				getComputeRefundData(1).computeCostInLamports,
			).toNumber();

			await atomicRotate({
				oldSigner: sessionSigner.publicKey,
				newSigner: newSessionSigner.publicKey,
				amount: new BN(2000),
				firstNonce: new BN(0),
				lastNonce: new BN(4),
			});
			await atomicRotate({
				oldSigner: newSessionSigner.publicKey,
				newSigner: sig1.publicKey,
				amount: new BN(3000),
				firstNonce: new BN(5),
				lastNonce: new BN(9),
			});

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.nextNonce.toNumber()).to.equal(10);
			// 2000 + 3000 plus one refund per settle — no period reset between them.
			expect(sub.currentCycleUsed.toNumber()).to.equal(2000 + 3000 + 2 * refund);
		});

		// The cumulative cap rejects an atomic settle that would push current_cycle_used to/over
		// max_amount. With the double-count fixed, the boundary is exact.
		it("Atomic settle respects the cumulative per-cycle cap (MaxAmountExceeded)", async () => {
			await createX402(); // max_amount = 50_000
			try {
				await atomicRotate({
					oldSigner: sessionSigner.publicKey,
					newSigner: newSessionSigner.publicKey,
					amount: new BN(50_000), // current_cycle_used(0) + 50_000 >= 50_000 → reject
					firstNonce: new BN(0),
					lastNonce: new BN(4),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/MaxAmountExceeded/);
			}
		});
	});

	describe("Increase Allowance", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const createBasicSub = async (allowance: BN, maxAmount: BN = new BN(5000)) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					uuid,
					new BN(500),
					maxAmount,
					FREQUENCY,
					mkFee(FEE_BPS),
					allowance,
					getComputeRefundData(2),
					SettleMode.Regular,
					PublicKey.default,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const extendAllowance = (newAllowance: BN, refundSigs = 2) =>
			program.methods
				.extendAllowance(uuid, newAllowance, getComputeRefundData(refundSigs))
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					authorityOwner: accounts.authority.publicKey,
					feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
					permitRegistry: permitRegistryPdaFor(
						accounts.payer.publicKey,
						tokenAccounts.tokenMint,
					),
					subscription: subscriptionPdaFor(uuid, accounts.payer.publicKey),
					subscriber: accounts.payer.publicKey,
					subscriberTokenAccount: tokenAccounts.payerTokenAccount,
					mint: tokenAccounts.tokenMint,
					tokenProgram: TOKEN_PROGRAM_ID,
				})
				.signers([accounts.payer])
				.rpc();

		it("Should increase allowance and reset usage", async () => {
			await createBasicSub(new BN(10_000));
			const newAllowance = new BN(20_000);
			await extendAllowance(newAllowance);

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.allowance.toNumber()).to.equal(newAllowance.toNumber());
		});

		// Regression for bug 1b: refund_cap was `max - max = 0`, so the compute refund always
		// reverted with MaxAmountExceeded and extend_allowance was unusable. The refund must now
		// actually be pulled to the fee recipient, and used_allowance reset to that refund.
		it("Extend pulls the compute refund (was always reverting before the fix)", async () => {
			// Create bills cycle 1: amount(500) + compute refund(2300) = current_cycle_used 2800.
			// max_amount must leave >= one extend-refund (2300) of headroom (8000 - 2800 = 5200),
			// otherwise the best-effort refund is correctly capped out and skipped.
			await createBasicSub(new BN(10_000), new BN(8_000));
			const before = await testHelper.getBalances(true);
			const refundValue = getComputeRefundInTokens(
				getComputeRefundData(2).computeCostInLamports,
			).toNumber();

			await extendAllowance(new BN(20_000), 2);

			const after = await testHelper.getBalances(true);
			expect(after.feeRecipient).to.equal(before.feeRecipient + refundValue);

			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.allowance.toNumber()).to.equal(20_000);
			// used_allowance is reset to just the refund taken by this call.
			expect(sub.usedAllowance.toNumber()).to.equal(refundValue);
		});

		it("Should reject increasing allowance to zero", async () => {
			await createBasicSub(new BN(10_000));
			try {
				await extendAllowance(new BN(0));
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/ZeroAmount/);
			}
		});

		it("Should reject lowering the allowance (must be strictly higher)", async () => {
			await createBasicSub(new BN(10_000));
			try {
				await extendAllowance(new BN(8_000));
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidAmount/);
			}
		});
	});

	describe("Referral Mechanism — One-Time Payments", () => {
		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
		});

		const REFERRAL_BPS = 5000; // 50% of platform fee

		it("Should split SOL platform fee between owner and referrer", async () => {
			await testHelper.testPayment(
				{
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: REFERRAL_BPS,
				},
				false,
			);
		});

		it("Should split token platform fee between owner and referrer", async () => {
			await testHelper.testPayment(
				{
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: REFERRAL_BPS,
				},
				true,
			);
		});

		it("Should reject referrer == owner (duplicate fee recipient)", async () => {
			await testHelper.testPayment(
				{
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: REFERRAL_BPS,
					overrideReferrerEqualsOwner: true,
					expectError: "DuplicateFeeRecipient",
				},
				false,
			);
		});

		it("Should reject referrer == organization (duplicate fee recipient, token)", async () => {
			// SPL path: referrer and organization resolve to the SAME ATA passed in two
			// `mut` account slots, so Anchor rejects with ConstraintDuplicateMutableAccount
			// before the program's DuplicateFeeRecipient check runs. The duplicate is still
			// blocked — just by the framework. (The native-SOL path above still exercises the
			// program-level DuplicateFeeRecipient check.)
			await testHelper.testPayment(
				{
					amount: new BN(1000 * 1_000_000),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: REFERRAL_BPS,
					overrideReferrerEqualsOrg: true,
					expectError: "ConstraintDuplicateMutableAccount",
				},
				true,
			);
		});

		it("Should reject if referral_fee_bps > MAX (>50%)", async () => {
			await testHelper.testPayment(
				{
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: 6000, // > 5000 cap
					expectError: "InvalidReferralFeePercentage",
				},
				false,
			);
		});

		it("Should reject if referral_fee_bps > 0 but referrer account is missing", async () => {
			await testHelper.testPayment(
				{
					amount: new BN(1 * LAMPORTS_PER_SOL),
					feeBps: FEE_BPS,
					organizationFeeBps: ORG_FEE_BPS,
					referralFeeBps: REFERRAL_BPS,
					noReferrer: true,
					expectError: "MissingReferrerAccount",
				},
				false,
			);
		});
	});

	describe("Referral Mechanism — Subscriptions", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const REFERRAL_BPS = 5000; // 50% of platform fee

		const futureDeadline = (offsetSeconds: number) =>
			new BN(Math.floor(Date.now() / 1000) + offsetSeconds);

		const createSubWithReferrer = (overrides: Record<string, any> = {}) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer =
				overrides.referrer === undefined
					? accounts.referrer.publicKey
					: overrides.referrer;
			(ctx as any).referrerTokenAccount =
				overrides.referrerTokenAccount === undefined
					? tokenAccounts.referrerTokenAccount
					: overrides.referrerTokenAccount;

			return program.methods
				.createSubscription(
					uuid,
					overrides.amount ?? new BN(1000),
					overrides.maxAmount ?? new BN(50_000),
					overrides.frequency ?? FREQUENCY,
					mkFee(
						overrides.feeBps ?? FEE_BPS,
						overrides.organizationFeeBps ?? 0,
						overrides.referralFeeBps ?? REFERRAL_BPS,
						overrides.referralDeadline ?? futureDeadline(3600),
					),
					overrides.allowance ?? new BN(200_000),
					getComputeRefundData(2),
					overrides.mode ?? SettleMode.Regular,
					overrides.sessionSigner ?? PublicKey.default,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const executeSubWithReferrer = (overrides: Record<string, any> = {}) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(ctx as any).referrerTokenAccount =
				overrides.referrerTokenAccount === undefined
					? tokenAccounts.referrerTokenAccount
					: overrides.referrerTokenAccount;

			return program.methods
				.executeSubscription(
					overrides.amount ?? new BN(5000),
					uuid,
					overrides.frequency ?? FREQUENCY,
					mkFee(
						overrides.feeBps ?? FEE_BPS,
						overrides.organizationFeeBps ?? 0,
						overrides.referralFeeBps ?? REFERRAL_BPS,
						overrides.referralDeadline ?? overrides._cachedDeadline,
					),
					getComputeRefundData(1),
					overrides.mode ?? SettleMode.Regular,
					overrides.sessionSigner ?? PublicKey.default,
				)
				.accounts(ctx as any)
				.signers([accounts.authority])
				.rpc();
		};

		it("Should reject create if referral_deadline is in the past", async () => {
			try {
				await createSubWithReferrer({
					referralDeadline: new BN(Math.floor(Date.now() / 1000) - 60),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidReferralDeadline/);
			}
		});

		it("Should reject create if referrer ATA == fee recipient ATA", async () => {
			// referrer_token_account and fee_recipient_token_account resolve to the same ATA,
			// passed in two `mut` slots. Anchor rejects with ConstraintDuplicateMutableAccount
			// before the program's DuplicateFeeRecipient check — the duplicate is still blocked.
			try {
				await createSubWithReferrer({
					referrer: accounts.authority.publicKey,
					referrerTokenAccount: tokenAccounts.authorityTokenAccount,
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/ConstraintDuplicateMutableAccount/);
			}
		});

		it("Should reject create if referral_fee_bps > MAX", async () => {
			try {
				await createSubWithReferrer({
					referralFeeBps: 6000,
					referralDeadline: futureDeadline(3600),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidReferralFeePercentage/);
			}
		});

		it("Should reject create if referral_fee_bps > 0 but no referrer ATA", async () => {
			try {
				await createSubWithReferrer({
					referrer: null,
					referrerTokenAccount: null,
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/MissingReferrerAccount/);
			}
		});

		it("Should pay referrer when within deadline on execute", async () => {
			const deadline = futureDeadline(3600);
			await createSubWithReferrer({ referralDeadline: deadline });
			// Regular create billed cycle 1; wait out the period before the execute.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			const balancesBefore = await testHelper.getBalances(true);
			await executeSubWithReferrer({
				amount: new BN(5000),
				_cachedDeadline: deadline,
			});

			const expectedGrossFees = 0.02 * 5000; // 100
			const expectedReferral = (expectedGrossFees * REFERRAL_BPS) / 10000; // 50
			const expectedNetPlatform = expectedGrossFees - expectedReferral; // 50
			const expectedMerchant = 5000 - expectedGrossFees; // 4900

			const refundData = getComputeRefundData(1);
			const refundValue = getComputeRefundInTokens(
				refundData.computeCostInLamports,
			).toNumber();

			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.referrer).to.equal(
				balancesBefore.referrer + expectedReferral,
			);
			expect(balancesAfter.feeRecipient).to.equal(
				balancesBefore.feeRecipient + expectedNetPlatform + refundValue,
			);
			expect(balancesAfter.merchant).to.equal(
				balancesBefore.merchant + expectedMerchant,
			);
		});

		it("Should NOT pay referrer once deadline has passed (platform owner gets full gross fee)", async () => {
			// Short deadline so we can race past it. Validator clock can lag wall-clock by
			// 1-2 seconds on localnet; sleep generously to ensure on-chain `now > deadline`.
			const deadline = futureDeadline(3);
			await createSubWithReferrer({ referralDeadline: deadline });
			await new Promise((r) => setTimeout(r, 6000));

			const balancesBefore = await testHelper.getBalances(true);
			await executeSubWithReferrer({
				amount: new BN(5000),
				_cachedDeadline: deadline,
			});

			const expectedGrossFees = 0.02 * 5000; // 100
			const refundData = getComputeRefundData(1);
			const refundValue = getComputeRefundInTokens(
				refundData.computeCostInLamports,
			).toNumber();

			const balancesAfter = await testHelper.getBalances(true);
			expect(balancesAfter.referrer).to.equal(balancesBefore.referrer);
			expect(balancesAfter.feeRecipient).to.equal(
				balancesBefore.feeRecipient + expectedGrossFees + refundValue,
			);
		});

		it("Execute hash mismatch if referral_fee_bps differs from create-time", async () => {
			const deadline = futureDeadline(3600);
			await createSubWithReferrer({ referralDeadline: deadline });
			// Regular create billed cycle 1; wait out the period so the execute clears the
			// next_payment_due guard and actually reaches the subscription-hash check.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			try {
				await executeSubWithReferrer({
					amount: new BN(5000),
					_cachedDeadline: deadline,
					referralFeeBps: 1000, // create used REFERRAL_BPS (5000)
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}
		});

		it("Execute hash mismatch if referrer ATA differs from create-time", async () => {
			const deadline = futureDeadline(3600);
			await createSubWithReferrer({ referralDeadline: deadline });
			// Regular create billed cycle 1; wait out the period so the execute reaches the hash check.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			try {
				// A valid ATA that differs from the create-time referrer ATA and collides with
				// no other `mut` account in execute (subscriberTokenAccount is unused there) —
				// so the hash check fires, not Anchor's duplicate-mutable-account guard.
				await executeSubWithReferrer({
					amount: new BN(5000),
					_cachedDeadline: deadline,
					referrerTokenAccount: tokenAccounts.subscriberTokenAccount,
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}
		});

		it("Subscription created without referrer cannot be executed with referrer args", async () => {
			// Create a no-referrer sub
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			await program.methods
				.createSubscription(
					uuid,
					new BN(500),
					new BN(5000),
					FREQUENCY,
					mkFee(FEE_BPS),
					new BN(20000),
					getComputeRefundData(2),
					SettleMode.Regular,
					PublicKey.default,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();

			// Regular create billed cycle 1; wait out the period so the execute clears the
			// next_payment_due guard and reaches the hash check.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

			// Try to execute with referral args → hash mismatch.
			// Use amount < max (4000 < 5000) so the max-amount guard doesn't fire first.
			try {
				await executeSubWithReferrer({
					amount: new BN(4000),
					_cachedDeadline: futureDeadline(3600),
				});
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/InvalidSubscriptionParameters/);
			}
		});
	});

	describe("Settle-Mode Guards", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const createWithMode = (mode: SettleMode, sessionSigner = PublicKey.default) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					uuid, new BN(1000), new BN(50_000), FREQUENCY, mkFee(FEE_BPS),
					new BN(200_000), getComputeRefundData(2), mode, sessionSigner,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const settleBatch = (which: "x402" | "merchant", mode: SettleMode) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(ctx as any).referrerTokenAccount = null;
			const method =
				which === "x402"
					? program.methods.settleX402Batch
					: program.methods.settleMerchantX402Batch;
			return method(
				uuid, new BN(1000), mkFee(FEE_BPS), FREQUENCY, PublicKey.default,
				new BN(0), new BN(0), Array.from(Buffer.alloc(32, 0x11)),
				getComputeRefundData(1),
			)
				.accounts(ctx as any)
				.signers([accounts.authority])
				.rpc();
		};

		it("cancel_subscription rejects a PAYG sub (WrongSettleMode)", async () => {
			await createWithMode(SettleMode.X402);
			try {
				await program.methods
					.cancelSubscription(uuid)
					.accounts(
						testHelper.getDefaultAccountsForSubscription(uuid, "cancel") as any,
					)
					.signers([accounts.payer])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/WrongSettleMode/);
			}
		});

		it("cancel_pay_as_you_go rejects a Regular sub (WrongSettleMode)", async () => {
			await createWithMode(SettleMode.Regular);
			try {
				await program.methods
					.cancelPayAsYouGoSubscription(uuid)
					.accounts(
						testHelper.getDefaultAccountsForSubscription(uuid, "cancel") as any,
					)
					.signers([accounts.payer])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/WrongSettleMode/);
			}
		});

		it("settle_x402_batch rejects a Regular sub (WrongSettleMode)", async () => {
			await createWithMode(SettleMode.Regular);
			try {
				await settleBatch("x402", SettleMode.Regular);
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/WrongSettleMode/);
			}
		});

		it("settle_merchant_x402_batch rejects an X402 sub (WrongSettleMode)", async () => {
			await createWithMode(SettleMode.X402);
			try {
				await settleBatch("merchant", SettleMode.X402);
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/WrongSettleMode/);
			}
		});
	});

	describe("Delegate Recovery (set_delegate)", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const createRegular = (allowance: BN) => {
			const ctx = testHelper.getDefaultAccountsForSubscription(uuid, "create");
			(ctx as any).referrer = null;
			(ctx as any).referrerTokenAccount = null;
			return program.methods
				.createSubscription(
					uuid, new BN(1000), new BN(50_000), FREQUENCY, mkFee(FEE_BPS),
					allowance, getComputeRefundData(2), SettleMode.Regular, PublicKey.default,
				)
				.accounts(ctx as any)
				.signers([accounts.payer])
				.rpc();
		};

		const setDelegateAccounts = () => ({
			authority: authorityAccountPda,
			authorityOwner: accounts.authority.publicKey,
			permitRegistry: permitRegistryPdaFor(
				accounts.payer.publicKey,
				tokenAccounts.tokenMint,
			),
			subscriber: accounts.payer.publicKey,
			subscriberTokenAccount: tokenAccounts.payerTokenAccount,
			mint: tokenAccounts.tokenMint,
			tokenProgram: TOKEN_PROGRAM_ID,
		});

		it("Restores the program PDA as delegate after the user revokes it", async () => {
			await createRegular(new BN(500_000));

			// User clears the delegate on their own token account.
			await revoke(
				provider.connection,
				accounts.payer,
				tokenAccounts.payerTokenAccount,
				accounts.payer,
			);
			let acct = await getAccount(
				provider.connection,
				tokenAccounts.payerTokenAccount,
			);
			expect(acct.delegate).to.equal(null);

			// set_delegate re-approves the program PDA for the registry's effective allowance.
			await program.methods
				.setDelegate()
				.accounts(setDelegateAccounts() as any)
				.signers([accounts.payer])
				.rpc();

			acct = await getAccount(
				provider.connection,
				tokenAccounts.payerTokenAccount,
			);
			expect(acct.delegate?.toString()).to.equal(authorityAccountPda.toString());
			expect(Number(acct.delegatedAmount)).to.be.greaterThan(0);
		});

		it("Is a no-op when the delegate is already the program PDA", async () => {
			await createRegular(new BN(500_000));
			// Delegate is already set by create. set_delegate should early-return Ok.
			const tx = await program.methods
				.setDelegate()
				.accounts(setDelegateAccounts() as any)
				.signers([accounts.payer])
				.rpc();
			expect(tx).to.exist;
		});
	});

	// NOTE: There is intentionally no "X402 One-Shot Payment" suite. The canonical x402 SVM
	// "exact" scheme can't carry QBitFlow's on-chain fee split / gas reimbursement / request_hash
	// binding (arbitrary program invocations are prohibited in a conformant tx), so the Solana
	// program deliberately omits a `process_x402_payment_exact` equivalent.

	describe("Update Max Amount Per Cycle", () => {
		let uuid: number[];

		beforeEach(async () => {
			await new Promise((resolve) => setTimeout(resolve, 75));
			uuid = testHelper.generateUuid();
		});

		const updateMaxAccounts = () => ({
			authority: authorityAccountPda,
			authorityOwner: accounts.authority.publicKey,
			subscription: subscriptionPdaFor(uuid, accounts.payer.publicKey),
			permitRegistry: permitRegistryPdaFor(
				accounts.payer.publicKey,
				tokenAccounts.tokenMint,
			),
			subscriber: accounts.payer.publicKey,
			subscriberTokenAccount: tokenAccounts.payerTokenAccount,
			mint: tokenAccounts.tokenMint,
			feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
			tokenProgram: TOKEN_PROGRAM_ID,
		});

		const createSub = async (maxAmount: BN) =>
			program.methods
				.createSubscription(
					uuid,
					new BN(1000), // last_payment_amount baseline
					maxAmount,
					FREQUENCY,
					mkFee(FEE_BPS),
					new BN(1_000_000),
					getComputeRefundData(2),
					SettleMode.Regular,
					PublicKey.default,
				)
				.accounts({
					...testHelper.getDefaultAccountsForSubscription(uuid, "create"),
					referrer: null,
					referrerTokenAccount: null,
				} as any)
				.signers([accounts.payer])
				.rpc();

		const updateMax = (newMax: BN, refundSigs = 1) =>
			program.methods
				.updateMaxAmountPerCycle(uuid, newMax, getComputeRefundData(refundSigs))
				.accounts(updateMaxAccounts() as any)
				.signers([accounts.payer])
				.rpc();

		// Run one period-gated Regular billing so current_cycle_used reflects a known
		// `execAmount + refund` (create also bills cycle 1, but the period-gated execute below
		// RESETS current_cycle_used to its own spend). Returns it so decrease-floor tests are
		// deterministic.
		const createAndBillOnce = async (maxAmount: BN, execAmount: BN) => {
			await createSub(maxAmount);
			// create billed cycle 1; wait out the period before the execute.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			const execCtx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(execCtx as any).referrerTokenAccount = null;
			await program.methods
				.executeSubscription(
					execAmount, uuid, FREQUENCY, mkFee(FEE_BPS),
					getComputeRefundData(1), SettleMode.Regular, PublicKey.default,
				)
				.accounts(execCtx as any)
				.signers([accounts.authority])
				.rpc();
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			return sub.currentCycleUsed.toNumber();
		};

		it("Should allow an increase up to 3x the current cap", async () => {
			await createSub(new BN(50_000));
			await updateMax(new BN(150_000)); // exactly 3x
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.maxAmountPerCycle.toNumber()).to.equal(150_000);
		});

		it("Should reject an increase beyond 3x the current cap", async () => {
			await createSub(new BN(50_000));
			try {
				await updateMax(new BN(150_001));
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/MaxIncreaseExceeded/);
			}
		});

		it("Should allow a decrease floored above the current cycle usage", async () => {
			const used = await createAndBillOnce(new BN(50_000), new BN(5000));
			expect(used).to.be.greaterThan(0);
			await updateMax(new BN(used + 5000)); // decrease, still > current_cycle_used
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.maxAmountPerCycle.toNumber()).to.equal(used + 5000);
		});

		it("Should reject a decrease at/below the current cycle usage", async () => {
			const used = await createAndBillOnce(new BN(50_000), new BN(5000));
			try {
				await updateMax(new BN(used)); // == current_cycle_used → not strictly above
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/MaxAmountInvalid/);
			}
		});

		it("Should reject a no-op (new == current)", async () => {
			await createSub(new BN(50_000));
			try {
				await updateMax(new BN(50_000));
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/NoChange/);
			}
		});

		// Regression for the digest-brick bug: max_amount is NOT in the subscription_hash, so a
		// cap change must NOT invalidate the digest. Before the fix, update_max changed the cap
		// without rewriting the digest, so the next settle failed InvalidSubscriptionParameters.
		it("Cap update does not brick settlement (digest is independent of max_amount)", async () => {
			await createSub(new BN(50_000)); // bills cycle 1; next_payment_due = now + frequency
			await updateMax(new BN(120_000)); // increase
			// Wait out the period so the settle below passes the period gate.
			await new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));
			// Settle must still succeed — proves the digest still matches after the cap change.
			const execCtx = testHelper.getDefaultAccountsForSubscription(uuid, "execute");
			(execCtx as any).referrerTokenAccount = null;
			await program.methods
				.executeSubscription(
					new BN(5000), uuid, FREQUENCY, mkFee(FEE_BPS),
					getComputeRefundData(1), SettleMode.Regular, PublicKey.default,
				)
				.accounts(execCtx as any)
				.signers([accounts.authority])
				.rpc();
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.maxAmountPerCycle.toNumber()).to.equal(120_000);
		});

		// Regression for the refund_cap bug + best-effort parity: a decrease to just above the
		// current usage leaves NO headroom for the compute refund. The refund must be skipped
		// (best-effort) rather than reverting — otherwise the user could be locked into a high
		// cap. Before the fix, refund_cap was always 0 and this path always reverted.
		it("Decrease with no refund headroom succeeds (refund is best-effort)", async () => {
			const used = await createAndBillOnce(new BN(50_000), new BN(5000));
			const before = await testHelper.getBalances(true);
			// new cap = used + 1: strictly above current usage (passes the floor) but only 1 unit
			// of headroom, far below the ~575-unit refund → refund is skipped, update still lands.
			await updateMax(new BN(used + 1));
			const sub = await program.account.subscription.fetch(
				subscriptionPdaFor(uuid, accounts.payer.publicKey),
			);
			expect(sub.maxAmountPerCycle.toNumber()).to.equal(used + 1);
			// No refund was pulled (headroom too small), so the fee recipient balance is unchanged.
			const after = await testHelper.getBalances(true);
			expect(after.feeRecipient).to.equal(before.feeRecipient);
		});
	});

	// The permit registry is a per-(subscriber, mint) PDA that tracks the SUM of allowances
	// across ALL of a subscriber's subscriptions on that mint, plus the cumulative amount
	// actually pulled. There is a SINGLE shared SPL delegate approval on the subscriber's token
	// account; every create/extend re-approves it to `total_allowance - total_used`. Without the
	// registry, each subscription would approve only its OWN allowance, clobbering the shared
	// delegate and silently breaking sibling subscriptions on the same mint. These tests verify
	// the registry keeps the sum coherent across create / execute / extend / cancel.
	describe("Permit Registry (multi-subscription accounting)", () => {
		// Compute refunds for the test token price (see helper.getComputeRefundData/InTokens).
		// Create signs with 2 keys (owner + subscriber); execute signs with 1 (owner only).
		const CREATE_REFUND = getComputeRefundInTokens(
			getComputeRefundData(2).computeCostInLamports,
		).toNumber();
		const EXEC_REFUND = getComputeRefundInTokens(
			getComputeRefundData(1).computeCostInLamports,
		).toNumber();

		// Fresh, isolated subscriber per test → a clean (subscriber, mint) registry that starts
		// empty, so absolute total_allowance / total_used assertions are deterministic. The
		// subscriber needs no SOL: the authority owner (provider wallet) pays all tx fees + rent.
		let regSubscriber: Keypair;
		let regSubAta: PublicKey;

		const regPda = () =>
			permitRegistryPdaFor(regSubscriber.publicKey, tokenAccounts.tokenMint);
		const subPda = (uuid: number[]) =>
			subscriptionPdaFor(uuid, regSubscriber.publicKey);
		const fetchReg = () => program.account.permitRegistry.fetch(regPda());
		const sleepPeriod = () =>
			new Promise((r) => setTimeout(r, FREQUENCY * 1000 + 500));

		// chai-as-promised isn't wired up in this suite, so assert "account is gone" by hand.
		const expectAccountGone = async (p: Promise<any>) => {
			let threw = false;
			try {
				await p;
			} catch {
				threw = true;
			}
			expect(threw).to.equal(true);
		};

		const getDelegateInfo = async () => {
			const acct = await getAccount(provider.connection, regSubAta);
			return {
				delegate: acct.delegate ? acct.delegate.toString() : null,
				amount: Number(acct.delegatedAmount),
			};
		};

		beforeEach(async () => {
			regSubscriber = Keypair.generate();
			regSubAta = await createAssociatedTokenAccount(
				provider.connection,
				accounts.authority, // fee payer for the ATA creation
				tokenAccounts.tokenMint,
				regSubscriber.publicKey,
			);
			await mintTo(
				provider.connection,
				accounts.authority,
				tokenAccounts.tokenMint,
				regSubAta,
				accounts.authority,
				1_000_000_000,
			);
		});

		const createSub = (
			uuid: number[],
			args: { amount: BN; maxAmount: BN; allowance: BN },
		) =>
			program.methods
				.createSubscription(
					uuid,
					args.amount,
					args.maxAmount,
					FREQUENCY,
					mkFee(FEE_BPS),
					args.allowance,
					getComputeRefundData(2),
					SettleMode.Regular,
					PublicKey.default,
				)
				.accounts({
					// @ts-ignore — Anchor's generated type wants the resolved PDA; we pass it explicitly.
					authority: authorityAccountPda,
					authorityOwner: accounts.authority.publicKey,
					permitRegistry: regPda(),
					subscription: subPda(uuid),
					subscriber: regSubscriber.publicKey,
					subscriberTokenAccount: regSubAta,
					merchant: accounts.merchant.publicKey,
					merchantTokenAccount: tokenAccounts.merchantTokenAccount,
					feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
					organization: accounts.organizationRecipient.publicKey,
					organizationTokenAccount: tokenAccounts.organizationTokenAccount,
					referrer: null,
					referrerTokenAccount: null,
					mint: tokenAccounts.tokenMint,
					tokenProgram: TOKEN_PROGRAM_ID,
					systemProgram: SystemProgram.programId,
					associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
				} as any)
				.signers([regSubscriber])
				.rpc();

		const executeSub = (uuid: number[], amount: BN) =>
			program.methods
				.executeSubscription(
					amount,
					uuid,
					FREQUENCY,
					mkFee(FEE_BPS),
					getComputeRefundData(1),
					SettleMode.Regular,
					PublicKey.default,
				)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					authorityOwner: accounts.authority.publicKey,
					permitRegistry: regPda(),
					subscription: subPda(uuid),
					subscriber: regSubscriber.publicKey,
					subscriberTokenAccount: regSubAta,
					merchantTokenAccount: tokenAccounts.merchantTokenAccount,
					mint: tokenAccounts.tokenMint,
					feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
					organizationTokenAccount: tokenAccounts.organizationTokenAccount,
					referrerTokenAccount: null,
					tokenProgram: TOKEN_PROGRAM_ID,
					systemProgram: SystemProgram.programId,
					associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
				} as any)
				.rpc();

		const cancelSub = (uuid: number[]) =>
			program.methods
				.cancelSubscription(uuid)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					owner: accounts.authority.publicKey,
					permitRegistry: regPda(),
					subscription: subPda(uuid),
					subscriber: regSubscriber.publicKey,
					mint: tokenAccounts.tokenMint,
				} as any)
				.signers([regSubscriber])
				.rpc();

		const forceCancelSub = (uuid: number[]) =>
			program.methods
				.forceCancelSubscription(uuid)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					authorityOwner: accounts.authority.publicKey,
					subscriber: regSubscriber.publicKey,
					permitRegistry: regPda(),
					subscription: subPda(uuid),
					mint: tokenAccounts.tokenMint,
				} as any)
				.rpc();

		const extendAllow = (uuid: number[], newAllowance: BN) =>
			program.methods
				.extendAllowance(uuid, newAllowance, getComputeRefundData(2))
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					authorityOwner: accounts.authority.publicKey,
					feeRecipientTokenAccount: tokenAccounts.authorityTokenAccount,
					subscription: subPda(uuid),
					permitRegistry: regPda(),
					subscriber: regSubscriber.publicKey,
					subscriberTokenAccount: regSubAta,
					mint: tokenAccounts.tokenMint,
					tokenProgram: TOKEN_PROGRAM_ID,
				} as any)
				.signers([regSubscriber])
				.rpc();

		it("Creating a second subscription for the same mint sums total_allowance", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();

			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			let reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(100_000);
			expect(reg.totalUsed.toNumber()).to.equal(1000 + CREATE_REFUND);

			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});
			reg = await fetchReg();
			// total_allowance is the SUM of both subscriptions' allowances.
			expect(reg.totalAllowance.toNumber()).to.equal(300_000);
			// total_used is the SUM of both create-time bills (amount + create refund each).
			expect(reg.totalUsed.toNumber()).to.equal(
				1000 + CREATE_REFUND + 2000 + CREATE_REFUND,
			);
		});

		it("The shared SPL delegate tracks the registry SUM, not the last subscription (no clobber)", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			const reg = await fetchReg();
			const effective = reg.totalAllowance.toNumber() - reg.totalUsed.toNumber();
			const del = await getDelegateInfo();

			// The single delegate approval equals the registry-wide effective allowance.
			expect(del.delegate).to.equal(authorityAccountPda.toString());
			expect(del.amount).to.equal(effective);

			// Crucially, it is LARGER than subscription B's own remaining allowance — proving
			// creating B re-approved the delegate to the COMBINED sum rather than overwriting it
			// with just B's allowance (which would have stranded A).
			const subB = await program.account.subscription.fetch(subPda(b));
			const bRemaining =
				subB.allowance.toNumber() - subB.usedAllowance.toNumber();
			expect(del.amount).to.be.greaterThan(bRemaining);
		});

		it("Executing one subscription advances total_used and leaves the sibling untouched", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			await sleepPeriod();
			await executeSub(a, new BN(5000));

			const reg = await fetchReg();
			// total_allowance unchanged by an execute; total_used grows by amount + execute refund.
			expect(reg.totalAllowance.toNumber()).to.equal(300_000);
			expect(reg.totalUsed.toNumber()).to.equal(
				1000 + CREATE_REFUND + 2000 + CREATE_REFUND + 5000 + EXEC_REFUND,
			);

			// Subscription A reflects its own cumulative spend.
			const subA = await program.account.subscription.fetch(subPda(a));
			expect(subA.usedAllowance.toNumber()).to.equal(
				1000 + CREATE_REFUND + 5000 + EXEC_REFUND,
			);
			// Subscription B is completely untouched by A's execute.
			const subB = await program.account.subscription.fetch(subPda(b));
			expect(subB.usedAllowance.toNumber()).to.equal(2000 + CREATE_REFUND);
			expect(subB.allowance.toNumber()).to.equal(200_000);

			// Delegate invariant still holds: approval == total_allowance - total_used.
			const del = await getDelegateInfo();
			expect(del.amount).to.equal(
				reg.totalAllowance.toNumber() - reg.totalUsed.toNumber(),
			);
		});

		it("Extending one subscription's allowance updates the registry total for the mint", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			// Extend B from 200_000 → 350_000. extend_allowance revokes B's old slice from the
			// registry (allowance + used), sets the new allowance, re-adds it, and pulls a
			// best-effort compute refund.
			await extendAllow(b, new BN(350_000));

			const reg = await fetchReg();
			// total_allowance = A (100_000) + new B (350_000).
			expect(reg.totalAllowance.toNumber()).to.equal(450_000);
			// total_used = A's create bill (unchanged) + B's reset usage (just the extend refund).
			expect(reg.totalUsed.toNumber()).to.equal(
				1000 + CREATE_REFUND + CREATE_REFUND,
			);

			const subB = await program.account.subscription.fetch(subPda(b));
			expect(subB.allowance.toNumber()).to.equal(350_000);
			// extend resets B's used_allowance to just the refund taken by this call.
			expect(subB.usedAllowance.toNumber()).to.equal(CREATE_REFUND);

			// Delegate was re-approved upward to the NEW registry-wide effective allowance.
			// Note: extend re-approves to (total_allowance - total_used) measured BEFORE its
			// best-effort compute refund. That refund is authorized by the subscriber directly
			// (CpiContext::new, not the PDA delegate), so — unlike create/execute, whose refund
			// runs through the PDA and decrements the approval — it does NOT decrement the
			// delegate here. Hence the approval sits one refund above the post-refund effective.
			const del = await getDelegateInfo();
			expect(del.delegate).to.equal(authorityAccountPda.toString());
			expect(del.amount).to.equal(
				reg.totalAllowance.toNumber() - reg.totalUsed.toNumber() + CREATE_REFUND,
			);
		});

		it("Cancelling one subscription removes only its slice, keeps the sibling valid & executable, and does NOT revoke the delegate", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			const delBefore = await getDelegateInfo();
			expect(delBefore.delegate).to.equal(authorityAccountPda.toString());

			// Cancel A while its next payment is still in the future (immediately after create).
			await cancelSub(a);

			// Registry now reflects ONLY B's slice — A's allowance and used are removed.
			let reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(200_000);
			expect(reg.totalUsed.toNumber()).to.equal(2000 + CREATE_REFUND);

			// A is gone; B is still present and unchanged.
			await expectAccountGone(program.account.subscription.fetch(subPda(a)));
			const subB = await program.account.subscription.fetch(subPda(b));
			expect(subB.allowance.toNumber()).to.equal(200_000);

			// The shared SPL delegate is UNTOUCHED by the cancel — same delegate, same amount.
			// Revoking it here would break B (the program could no longer pull B's payments).
			const delAfter = await getDelegateInfo();
			expect(delAfter.delegate).to.equal(authorityAccountPda.toString());
			expect(delAfter.amount).to.equal(delBefore.amount);
			expect(delAfter.amount).to.be.greaterThan(0);

			// And B is still fully executable after A's cancel.
			await sleepPeriod();
			await executeSub(b, new BN(5000));

			reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(200_000);
			expect(reg.totalUsed.toNumber()).to.equal(
				2000 + CREATE_REFUND + 5000 + EXEC_REFUND,
			);
		});

		it("Force-cancel removes the cancelled subscription's FULL used (including billed spend) and the sibling still executes", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			// Bill A once so its used_allowance exceeds the create bill — then force-cancel it.
			await sleepPeriod();
			await executeSub(a, new BN(5000));

			// force_cancel bypasses the next_payment_due check, so no timing race here.
			await forceCancelSub(a);

			// A's entire slice (allowance 100_000 + its full used incl. the 5000 execute) is
			// removed; the registry is left with exactly B's slice.
			let reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(200_000);
			expect(reg.totalUsed.toNumber()).to.equal(2000 + CREATE_REFUND);

			// B is still billable.
			await executeSub(b, new BN(3000));
			reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(200_000);
			expect(reg.totalUsed.toNumber()).to.equal(
				2000 + CREATE_REFUND + 3000 + EXEC_REFUND,
			);
		});

		it("Cancelling the LAST subscription closes the registry account", async () => {
			const a = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});

			// Sanity: registry exists with A's slice.
			const reg = await fetchReg();
			expect(reg.totalAllowance.toNumber()).to.equal(100_000);

			// Cancelling the only subscription drops total_allowance to 0 → registry is closed.
			await cancelSub(a);
			await expectAccountGone(fetchReg());
		});

		it("Creating another subscription after a partial cancel re-grows the registry and re-approves the delegate to the new sum", async () => {
			const a = testHelper.generateUuid();
			const b = testHelper.generateUuid();
			const c = testHelper.generateUuid();
			await createSub(a, {
				amount: new BN(1000),
				maxAmount: new BN(50_000),
				allowance: new BN(100_000),
			});
			await createSub(b, {
				amount: new BN(2000),
				maxAmount: new BN(50_000),
				allowance: new BN(200_000),
			});

			// Cancel A, leaving only B in the registry.
			await cancelSub(a);

			// New subscription C on the same (subscriber, mint) reuses the existing registry.
			await createSub(c, {
				amount: new BN(1500),
				maxAmount: new BN(50_000),
				allowance: new BN(75_000),
			});

			const reg = await fetchReg();
			// total_allowance = B (200_000) + C (75_000); A's slice is gone.
			expect(reg.totalAllowance.toNumber()).to.equal(275_000);
			expect(reg.totalUsed.toNumber()).to.equal(
				2000 + CREATE_REFUND + 1500 + CREATE_REFUND,
			);

			// Delegate re-approved to the new registry-wide effective allowance.
			const del = await getDelegateInfo();
			expect(del.delegate).to.equal(authorityAccountPda.toString());
			expect(del.amount).to.equal(
				reg.totalAllowance.toNumber() - reg.totalUsed.toNumber(),
			);
		});
	});

	describe("Update Owner", () => {
		let originalOwner: Keypair;
		let newOwner: Keypair;

		before(() => {
			originalOwner = accounts.authority;
			newOwner = Keypair.generate();
		});

		it("Should reject if cosigner isn't correct", async () => {
			const keyp = Keypair.generate();
			try {
				await program.methods
					.updateOwner(newOwner.publicKey)
					.accounts({
						// @ts-ignore
						authority: authorityAccountPda,
						owner: originalOwner.publicKey,
						coSigner: keyp.publicKey,
					})
					.signers([originalOwner, keyp])
					.rpc();
				expect.fail("Expected constraint failure");
			} catch (err: any) {
				expect(err.message).to.match(/Unauthorized|Constraint/);
			}
		});

		it("Should update owner when signed by current owner and the correct cosigner", async () => {
			await program.methods
				.updateOwner(newOwner.publicKey)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					owner: originalOwner.publicKey,
					coSigner: coSignerKeyPair.publicKey,
				})
				.signers([originalOwner, coSignerKeyPair])
				.rpc();

			const authorityAcc =
				await program.account.authority.fetch(authorityAccountPda);
			expect(authorityAcc.owner.toString()).to.equal(
				newOwner.publicKey.toString(),
			);
			expect(authorityAcc.coSigner.toString()).to.equal(
				coSignerKeyPair.publicKey.toString(),
			);
		});

		it("Should reject co-signer rotation without the current co-signer", async () => {
			const newCoSigner = Keypair.generate();
			const wrong = Keypair.generate();
			try {
				await program.methods
					.updateCoSigner(newCoSigner.publicKey)
					.accounts({
						// @ts-ignore
						authority: authorityAccountPda,
						owner: newOwner.publicKey, // current owner after the rotation above
						coSigner: wrong.publicKey,
					})
					.signers([newOwner, wrong])
					.rpc();
				expect.fail("Should have thrown");
			} catch (err: any) {
				expect(err.message).to.match(/Unauthorized|Constraint/);
			}
		});

		it("Should rotate the co-signer (current owner + current co-signer sign)", async () => {
			const newCoSigner = Keypair.generate();
			await program.methods
				.updateCoSigner(newCoSigner.publicKey)
				.accounts({
					// @ts-ignore
					authority: authorityAccountPda,
					owner: newOwner.publicKey, // current owner after the owner rotation above
					coSigner: coSignerKeyPair.publicKey, // current co-signer
				})
				.signers([newOwner, coSignerKeyPair])
				.rpc();

			const authorityAcc =
				await program.account.authority.fetch(authorityAccountPda);
			expect(authorityAcc.coSigner.toString()).to.equal(
				newCoSigner.publicKey.toString(),
			);
		});
	});
});
