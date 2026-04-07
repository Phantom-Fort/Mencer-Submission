use anchor_lang::prelude::*;
use anchor_lang::system_program;

declare_id!("REPLACE_WITH_YOUR_PROGRAM_ID");

// ─── Constants ───────────────────────────────────────────────────────────────

const CAMPAIGN_SEED:     &[u8] = b"campaign";
const VAULT_SEED:        &[u8] = b"vault";
const CONTRIBUTION_SEED: &[u8] = b"contribution";

/// Campaign space:
/// discriminator(8) + creator(32) + campaign_id(32) + goal(8) + raised(8)
/// + refunded(8) + deadline(8) + claimed(1) + bump(1) = 106
const CAMPAIGN_SPACE: usize = 8 + 32 + 32 + 8 + 8 + 8 + 8 + 1 + 1;

/// Contribution space:
/// discriminator(8) + donor(32) + campaign(32) + amount(8) + refunded(1) + bump(1) = 82
const CONTRIBUTION_SPACE: usize = 8 + 32 + 32 + 8 + 1 + 1;

// ─── Program ─────────────────────────────────────────────────────────────────

#[program]
pub mod crowdfund {
    use super::*;

    /// Creator sets up a new fundraising campaign.
    /// `campaign_id` is a unique 32-byte identifier so one creator can run
    /// multiple campaigns (e.g. pass `Pubkey::new_unique().to_bytes()`).
    pub fn create_campaign(
        ctx: Context<CreateCampaign>,
        campaign_id: [u8; 32],
        goal: u64,
        deadline: i64,
    ) -> Result<()> {
        let clock = Clock::get()?;

        require!(deadline > clock.unix_timestamp, ErrorCode::DeadlineInPast);
        require!(goal > 0,                        ErrorCode::InvalidGoal);

        let campaign         = &mut ctx.accounts.campaign;
        campaign.creator     = ctx.accounts.creator.key();
        campaign.campaign_id = campaign_id;
        campaign.goal        = goal;
        campaign.raised      = 0;
        campaign.refunded    = 0;
        campaign.deadline    = deadline;
        campaign.claimed     = false;
        campaign.bump        = ctx.bumps.campaign;

        emit!(CampaignCreated {
            creator: campaign.creator,
            campaign_id,
            goal,
            deadline,
        });

        msg!("Campaign created: goal={}, deadline={}", goal, deadline);
        Ok(())
    }

    /// Donor sends SOL to the campaign vault PDA.
    /// A per-donor `Contribution` PDA is initialised on first contribution
    /// and updated on subsequent ones — this is the trusted record for refunds.
    pub fn contribute(ctx: Context<Contribute>, amount: u64) -> Result<()> {
        require!(amount > 0, ErrorCode::InvalidAmount);

        let clock = Clock::get()?;
        require!(
            clock.unix_timestamp < ctx.accounts.campaign.deadline,
            ErrorCode::CampaignEnded
        );

        // Transfer SOL from donor → vault PDA
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.donor.to_account_info(),
                    to:   ctx.accounts.vault.to_account_info(),
                },
            ),
            amount,
        )?;

        // Update on-chain contribution record (trusted source of truth for refunds)
        let contribution      = &mut ctx.accounts.contribution;
        contribution.donor    = ctx.accounts.donor.key();
        contribution.campaign = ctx.accounts.campaign.key();
        contribution.amount   = contribution.amount
            .checked_add(amount)
            .ok_or(ErrorCode::Overflow)?;
        contribution.refunded = false;
        contribution.bump     = ctx.bumps.contribution;

        // Update campaign running total
        let campaign    = &mut ctx.accounts.campaign;
        campaign.raised = campaign.raised
            .checked_add(amount)
            .ok_or(ErrorCode::Overflow)?;

        emit!(ContributionMade {
            donor:    ctx.accounts.donor.key(),
            campaign: campaign.key(),
            amount,
            total:    campaign.raised,
        });

        msg!("Contributed: {} lamports, total={}", amount, campaign.raised);
        Ok(())
    }

    /// Creator claims all vault funds after a successful campaign.
    /// Requires: deadline passed AND goal met AND not already claimed.
    pub fn withdraw(ctx: Context<Withdraw>) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let clock    = Clock::get()?;

        require!(clock.unix_timestamp >= campaign.deadline, ErrorCode::DeadlineNotReached);
        require!(campaign.raised >= campaign.goal,          ErrorCode::GoalNotReached);
        require!(!campaign.claimed,                         ErrorCode::AlreadyClaimed);

        // Checks-Effects-Interactions: write state before any CPI
        campaign.claimed = true;

        let amount       = ctx.accounts.vault.lamports();
        let campaign_key = campaign.key();
        let vault_seeds  = &[
            VAULT_SEED,
            campaign_key.as_ref(),
            &[ctx.bumps.vault],
        ];

        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to:   ctx.accounts.creator.to_account_info(),
                },
                &[vault_seeds],
            ),
            amount,
        )?;

        emit!(FundsWithdrawn {
            creator:  ctx.accounts.creator.key(),
            campaign: campaign_key,
            amount,
        });

        msg!("Withdrawn: {} lamports", amount);
        Ok(())
    }

    /// Donor reclaims their contribution after a failed campaign.
    /// Refund amount is read entirely from the on-chain `Contribution` PDA —
    /// no user-supplied amount — eliminating the vault-drain vulnerability.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        let clock = Clock::get()?;

        require!(
            clock.unix_timestamp >= ctx.accounts.campaign.deadline,
            ErrorCode::DeadlineNotReached
        );
        require!(
            ctx.accounts.campaign.raised < ctx.accounts.campaign.goal,
            ErrorCode::GoalWasReached
        );
        require!(
            !ctx.accounts.contribution.refunded,
            ErrorCode::AlreadyRefunded
        );

        // Trusted amount from on-chain record — no user input involved
        let refund_amount = ctx.accounts.contribution.amount;
        require!(refund_amount > 0, ErrorCode::NothingToRefund);

        let vault_lamports = ctx.accounts.vault.lamports();
        require!(vault_lamports >= refund_amount, ErrorCode::InsufficientVaultFunds);

        // Checks-Effects-Interactions: update state before CPI
        ctx.accounts.contribution.refunded = true;

        let campaign = &mut ctx.accounts.campaign;
        campaign.refunded = campaign.refunded
            .checked_add(refund_amount)
            .ok_or(ErrorCode::Overflow)?;

        let campaign_key = campaign.key();
        let vault_seeds  = &[
            VAULT_SEED,
            campaign_key.as_ref(),
            &[ctx.bumps.vault],
        ];

        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to:   ctx.accounts.donor.to_account_info(),
                },
                &[vault_seeds],
            ),
            refund_amount,
        )?;

        emit!(RefundIssued {
            donor:    ctx.accounts.donor.key(),
            campaign: campaign_key,
            amount:   refund_amount,
        });

        msg!("Refunded: {} lamports", refund_amount);
        Ok(())
    }
}

// ─── Account Contexts ─────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(campaign_id: [u8; 32])]
pub struct CreateCampaign<'info> {
    /// Campaign state PDA — seeded by creator + campaign_id.
    /// Including campaign_id allows one creator to run unlimited campaigns.
    #[account(
        init,
        payer  = creator,
        space  = CAMPAIGN_SPACE,
        seeds  = [CAMPAIGN_SEED, creator.key().as_ref(), campaign_id.as_ref()],
        bump
    )]
    pub campaign: Account<'info, Campaign>,

    /// Vault PDA — holds donated SOL, separate from state to avoid rent conflicts.
    #[account(
        mut,
        seeds  = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    #[account(mut)]
    pub creator: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Contribute<'info> {
    #[account(
        mut,
        seeds = [CAMPAIGN_SEED, campaign.creator.as_ref(), campaign.campaign_id.as_ref()],
        bump  = campaign.bump
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(
        mut,
        seeds = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    /// Per-donor record created on first contribution, updated on subsequent ones.
    /// Seeded by campaign + donor so it is unique per (campaign, donor) pair.
    #[account(
        init_if_needed,
        payer  = donor,
        space  = CONTRIBUTION_SPACE,
        seeds  = [CONTRIBUTION_SEED, campaign.key().as_ref(), donor.key().as_ref()],
        bump
    )]
    pub contribution: Account<'info, Contribution>,

    #[account(mut)]
    pub donor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(
        mut,
        seeds   = [CAMPAIGN_SEED, creator.key().as_ref(), campaign.campaign_id.as_ref()],
        bump    = campaign.bump,
        has_one = creator @ ErrorCode::Unauthorized
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(
        mut,
        seeds = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    #[account(mut)]
    pub creator: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Refund<'info> {
    #[account(
        mut,
        seeds = [CAMPAIGN_SEED, campaign.creator.as_ref(), campaign.campaign_id.as_ref()],
        bump  = campaign.bump
    )]
    pub campaign: Account<'info, Campaign>,

    #[account(
        mut,
        seeds = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    /// The donor's contribution record — ONLY source of refund amount.
    /// `has_one = donor` ensures the signing donor matches this record's donor field.
    /// A donor cannot pass someone else's Contribution PDA.
    #[account(
        mut,
        seeds   = [CONTRIBUTION_SEED, campaign.key().as_ref(), donor.key().as_ref()],
        bump    = contribution.bump,
        has_one = donor @ ErrorCode::Unauthorized,
    )]
    pub contribution: Account<'info, Contribution>,

    #[account(mut)]
    pub donor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

// ─── State ────────────────────────────────────────────────────────────────────

/// On-chain campaign state.
#[account]
pub struct Campaign {
    /// Wallet that created the campaign and receives funds on success
    pub creator:     Pubkey,
    /// Unique campaign identifier — enables multiple campaigns per creator
    pub campaign_id: [u8; 32],
    /// Target lamports
    pub goal:        u64,
    /// Running total of lamports contributed
    pub raised:      u64,
    /// Running total of lamports refunded (keeps state consistent post-failure)
    pub refunded:    u64,
    /// Unix timestamp when the campaign closes
    pub deadline:    i64,
    /// True once the creator has withdrawn funds
    pub claimed:     bool,
    /// PDA bump stored for cheap re-derivation
    pub bump:        u8,
}

/// Per-donor contribution record — the single source of truth for refund amounts.
/// Seeded by [campaign, donor] so it is unique per (campaign, donor) pair.
#[account]
pub struct Contribution {
    /// The contributor's wallet
    pub donor:    Pubkey,
    /// The campaign this contribution belongs to
    pub campaign: Pubkey,
    /// Cumulative lamports contributed by this donor to this campaign
    pub amount:   u64,
    /// True once the donor has been refunded — prevents double refunds
    pub refunded: bool,
    /// PDA bump
    pub bump:     u8,
}

// ─── Events ───────────────────────────────────────────────────────────────────

/// Emitted when a new campaign is created.
#[event]
pub struct CampaignCreated {
    pub creator:     Pubkey,
    pub campaign_id: [u8; 32],
    pub goal:        u64,
    pub deadline:    i64,
}

/// Emitted on every contribution.
#[event]
pub struct ContributionMade {
    pub donor:    Pubkey,
    pub campaign: Pubkey,
    pub amount:   u64,
    pub total:    u64,
}

/// Emitted when the creator withdraws after a successful campaign.
#[event]
pub struct FundsWithdrawn {
    pub creator:  Pubkey,
    pub campaign: Pubkey,
    pub amount:   u64,
}

/// Emitted when a donor is refunded after a failed campaign.
#[event]
pub struct RefundIssued {
    pub donor:    Pubkey,
    pub campaign: Pubkey,
    pub amount:   u64,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[error_code]
pub enum ErrorCode {
    #[msg("Deadline must be in the future")]
    DeadlineInPast,
    #[msg("Goal must be greater than zero")]
    InvalidGoal,
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Campaign has already ended — no further contributions accepted")]
    CampaignEnded,
    #[msg("Deadline has not been reached yet")]
    DeadlineNotReached,
    #[msg("Campaign goal has not been reached — withdrawal not available")]
    GoalNotReached,
    #[msg("Campaign goal was reached — refunds are not available")]
    GoalWasReached,
    #[msg("Funds have already been claimed by the creator")]
    AlreadyClaimed,
    #[msg("This donor has already received a refund for this campaign")]
    AlreadyRefunded,
    #[msg("No contribution on record to refund")]
    NothingToRefund,
    #[msg("You are not authorised to perform this action")]
    Unauthorized,
    #[msg("Vault has insufficient lamports for this refund")]
    InsufficientVaultFunds,
    #[msg("Arithmetic overflow — value out of range")]
    Overflow,
}