use anchor_lang::prelude::*;
use anchor_lang::system_program;

declare_id!("GzUGzu5NB1RMoYVw27nkGrZuVrgZ1YAanToagVVpgw3P");

// ─── Constants ───────────────────────────────────────────────────────────────

const CAMPAIGN_SEED: &[u8] = b"campaign";
const VAULT_SEED: &[u8]    = b"vault";

/// Anchor discriminator (8) + Pubkey (32) + u64 (8) + u64 (8) + i64 (8) + bool (1) + u8 bump (1)
const CAMPAIGN_SPACE: usize = 8 + 32 + 8 + 8 + 8 + 1 + 1;

// ─── Program ─────────────────────────────────────────────────────────────────

#[program]
pub mod crowdfund {
    use super::*;

    /// Creator sets up a new fundraising campaign.
    pub fn create_campaign(
        ctx: Context<CreateCampaign>,
        goal: u64,
        deadline: i64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        require!(deadline > clock.unix_timestamp, ErrorCode::DeadlineInPast);
        require!(goal > 0, ErrorCode::InvalidGoal);

        let campaign      = &mut ctx.accounts.campaign;
        campaign.creator  = ctx.accounts.creator.key();
        campaign.goal     = goal;
        campaign.raised   = 0;
        campaign.deadline = deadline;
        campaign.claimed  = false;
        campaign.bump     = ctx.bumps.campaign;

        msg!("Campaign created: goal={}, deadline={}", goal, deadline);
        Ok(())
    }

    /// Donor sends SOL to the campaign vault PDA.
    pub fn contribute(ctx: Context<Contribute>, amount: u64) -> Result<()> {
        require!(amount > 0, ErrorCode::InvalidAmount);

        let clock = Clock::get()?;
        require!(
            clock.unix_timestamp < ctx.accounts.campaign.deadline,
            ErrorCode::CampaignEnded
        );

        // Transfer SOL from donor → vault PDA via System Program CPI
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

        let campaign    = &mut ctx.accounts.campaign;
        campaign.raised += amount;

        msg!(
            "Contributed: {} lamports, total={}",
            amount,
            campaign.raised
        );
        Ok(())
    }

    /// Creator withdraws all funds if goal is reached after deadline.
    pub fn withdraw(ctx: Context<Withdraw>) -> Result<()> {
        let campaign = &mut ctx.accounts.campaign;
        let clock    = Clock::get()?;

        require!(clock.unix_timestamp >= campaign.deadline, ErrorCode::DeadlineNotReached);
        require!(campaign.raised >= campaign.goal,          ErrorCode::GoalNotReached);
        require!(!campaign.claimed,                         ErrorCode::AlreadyClaimed);

        let amount        = ctx.accounts.vault.lamports();
        campaign.claimed  = true;

        // Sign the transfer out of the vault PDA using stored bump
        let campaign_key  = campaign.key();
        let vault_seeds   = &[
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

        msg!("Withdrawn: {} lamports", amount);
        Ok(())
    }

    /// Donor reclaims their contribution if goal was NOT reached after deadline.
    pub fn refund(ctx: Context<Refund>, contributed: u64) -> Result<()> {
        let campaign = &ctx.accounts.campaign;
        let clock    = Clock::get()?;

        require!(clock.unix_timestamp >= campaign.deadline, ErrorCode::DeadlineNotReached);
        require!(campaign.raised < campaign.goal,           ErrorCode::GoalWasReached);
        require!(contributed > 0,                           ErrorCode::InvalidAmount);

        // Ensure vault has enough lamports for the refund
        let vault_lamports = ctx.accounts.vault.lamports();
        require!(vault_lamports >= contributed, ErrorCode::InsufficientVaultFunds);

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
            contributed,
        )?;

        msg!("Refunded: {} lamports", contributed);
        Ok(())
    }
}

// ─── Account Contexts ─────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct CreateCampaign<'info> {
    /// The campaign state account — PDA seeded by creator pubkey
    #[account(
        init,
        payer  = creator,
        space  = CAMPAIGN_SPACE,
        seeds  = [CAMPAIGN_SEED, creator.key().as_ref()],
        bump
    )]
    pub campaign: Account<'info, Campaign>,

    /// The vault that will hold donated SOL — separate PDA
    /// CHECK: This is a bare system account PDA used only as a lamport sink.
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
        seeds = [CAMPAIGN_SEED, campaign.creator.as_ref()],
        bump  = campaign.bump
    )]
    pub campaign: Account<'info, Campaign>,

    /// CHECK: vault PDA — receives lamports from donor
    #[account(
        mut,
        seeds = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    #[account(mut)]
    pub donor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(
        mut,
        seeds    = [CAMPAIGN_SEED, creator.key().as_ref()],
        bump     = campaign.bump,
        has_one  = creator @ ErrorCode::Unauthorized
    )]
    pub campaign: Account<'info, Campaign>,

    /// CHECK: vault PDA — sends lamports to creator
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
        seeds = [CAMPAIGN_SEED, campaign.creator.as_ref()],
        bump  = campaign.bump
    )]
    pub campaign: Account<'info, Campaign>,

    /// CHECK: vault PDA — sends lamports back to donor
    #[account(
        mut,
        seeds = [VAULT_SEED, campaign.key().as_ref()],
        bump
    )]
    pub vault: SystemAccount<'info>,

    #[account(mut)]
    pub donor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

// ─── State ────────────────────────────────────────────────────────────────────

#[account]
pub struct Campaign {
    pub creator:  Pubkey,
    pub goal:     u64,
    pub raised:   u64,
    pub deadline: i64,
    pub claimed:  bool,
    pub bump:     u8,     // stored so PDA re-derivation is cheap
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
    #[msg("Campaign has already ended")]
    CampaignEnded,
    #[msg("Deadline has not been reached yet")]
    DeadlineNotReached,
    #[msg("Campaign goal has not been reached")]
    GoalNotReached,
    #[msg("Campaign goal was reached — refunds not available")]
    GoalWasReached,
    #[msg("Funds have already been claimed")]
    AlreadyClaimed,
    #[msg("You are not the campaign creator")]
    Unauthorized,
    #[msg("Vault has insufficient funds for refund")]
    InsufficientVaultFunds,
}
