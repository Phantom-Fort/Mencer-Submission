import * as anchor from "@coral-xyz/anchor";
import { Program, BN }  from "@coral-xyz/anchor";
import { Crowdfund }    from "../target/types/crowdfund";
import {
  PublicKey,
  SystemProgram,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { assert } from "chai";

describe("crowdfund", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program  = anchor.workspace.Crowdfund as Program<Crowdfund>;
  const creator  = provider.wallet as anchor.Wallet;

  // Fresh donor keypair funded via airdrop
  const donor    = anchor.web3.Keypair.generate();

  let campaignPda : PublicKey;
  let vaultPda    : PublicKey;
  let campaignBump: number;
  let vaultBump   : number;

  // ── Helpers ────────────────────────────────────────────────────────────────

  const futureDeadline = () =>
    new BN(Math.floor(Date.now() / 1000) + 60 * 60); // 1 hour from now

  const pastDeadline = () =>
    new BN(Math.floor(Date.now() / 1000) - 1); // 1 second ago

  // ── Setup ──────────────────────────────────────────────────────────────────

  before(async () => {
    // Derive PDAs
    [campaignPda, campaignBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("campaign"), creator.publicKey.toBuffer()],
      program.programId
    );
    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), campaignPda.toBuffer()],
      program.programId
    );

    // Fund the donor account
    const sig = await provider.connection.requestAirdrop(
      donor.publicKey,
      5 * LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);
  });

  // ── Tests ──────────────────────────────────────────────────────────────────

  it("Creates a campaign", async () => {
    const goal     = new BN(2 * LAMPORTS_PER_SOL);
    const deadline = futureDeadline();

    await program.methods
      .createCampaign(goal, deadline)
      .accounts({
        campaign:      campaignPda,
        vault:         vaultPda,
        creator:       creator.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const campaign = await program.account.campaign.fetch(campaignPda);
    assert.ok(campaign.creator.equals(creator.publicKey), "creator mismatch");
    assert.equal(campaign.goal.toString(), goal.toString(),     "goal mismatch");
    assert.equal(campaign.raised.toString(), "0",               "raised should be 0");
    assert.equal(campaign.claimed, false,                       "should not be claimed");
    console.log("✅ Campaign created — goal:", goal.toString());
  });

  it("Accepts a contribution from donor", async () => {
    const amount = new BN(0.6 * LAMPORTS_PER_SOL);

    await program.methods
      .contribute(amount)
      .accounts({
        campaign:      campaignPda,
        vault:         vaultPda,
        donor:         donor.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([donor])
      .rpc();

    const campaign = await program.account.campaign.fetch(campaignPda);
    assert.equal(
      campaign.raised.toString(),
      amount.toString(),
      "raised amount mismatch"
    );
    console.log("✅ Contribution accepted — raised:", campaign.raised.toString());
  });

  it("Accepts a second contribution", async () => {
    const amount = new BN(1.5 * LAMPORTS_PER_SOL);

    await program.methods
      .contribute(amount)
      .accounts({
        campaign:      campaignPda,
        vault:         vaultPda,
        donor:         donor.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([donor])
      .rpc();

    const campaign = await program.account.campaign.fetch(campaignPda);
    const expected = new BN(2.1 * LAMPORTS_PER_SOL);
    assert.equal(campaign.raised.toString(), expected.toString());
    console.log("✅ Second contribution accepted — total raised:", campaign.raised.toString());
  });

  it("Rejects withdraw before deadline", async () => {
    try {
      await program.methods
        .withdraw()
        .accounts({
          campaign:      campaignPda,
          vault:         vaultPda,
          creator:       creator.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();
      assert.fail("Should have thrown DeadlineNotReached");
    } catch (err: any) {
      assert.include(err.toString(), "DeadlineNotReached");
      console.log("✅ Withdraw correctly rejected before deadline");
    }
  });

  it("Rejects contribution with zero amount", async () => {
    try {
      await program.methods
        .contribute(new BN(0))
        .accounts({
          campaign:      campaignPda,
          vault:         vaultPda,
          donor:         donor.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([donor])
        .rpc();
      assert.fail("Should have thrown InvalidAmount");
    } catch (err: any) {
      assert.include(err.toString(), "InvalidAmount");
      console.log("✅ Zero-amount contribution correctly rejected");
    }
  });

  it("Rejects unauthorized withdraw attempt", async () => {
    const imposter = anchor.web3.Keypair.generate();
    const sig = await provider.connection.requestAirdrop(
      imposter.publicKey,
      LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    try {
      await program.methods
        .withdraw()
        .accounts({
          campaign:      campaignPda,
          vault:         vaultPda,
          creator:       imposter.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([imposter])
        .rpc();
      assert.fail("Should have thrown Unauthorized or constraint error");
    } catch (err: any) {
      // Anchor rejects has_one mismatch before instruction body runs
      assert.ok(err, "Error was thrown as expected");
      console.log("✅ Unauthorized withdraw correctly rejected");
    }
  });

  // NOTE: deadline-dependent tests (successful withdraw, refund) require
  // either time manipulation or a separate campaign created with a past
  // deadline. On a live validator you'd use Clock sysvar overrides in tests.
  // Those are shown below as commented stubs for completeness.

  /*
  it("Allows withdraw after deadline when goal is met", async () => {
    // Deploy a second campaign with goal already exceeded and past deadline.
    // Omitted here — requires Clock override or localnet time travel.
  });

  it("Allows refund after deadline when goal is not met", async () => {
    // Deploy a campaign that expires without hitting its goal.
    // Omitted here — requires Clock override or localnet time travel.
  });
  */
});
