//! ProofShare (formerly the Scrypt Collaborator Compact). Sister program to ProofDrop.
//!
//! One program for every Scrypt collaborative project. Each project has a treasury,
//! a roster, a contribution ledger, member votes and a prize settlement, with the
//! Collaborator Standing Offer and its Governance Addendum A (ruled 27 Oct 2026)
//! enforced by `rules`.
//!
//! Off the top of every settlement, the organizer's fee (fixed when the project opens; 1% under
//! the standing offer) is paid to the organizer's own wallet: Thomas Lanham personally, not the house.
//!
//! Every decision is a proposal the members vote on. No single wallet, the house's
//! included, can add a member, spend, accept a contribution or settle a prize alone.
//!
//! Life of a project
//!   0 CreateProject  the house opens a project: its id, contribution kinds and weights,
//!                    qualifying bar, vote threshold, split, and founding roster.
//!   1 Propose        any member proposes one of:
//!                      ADD_MEMBER    bring in a new contributor
//!                      EXPENSE       pay a recipient out of operating money
//!                      CONTRIBUTION  log accepted work against a member (that member is recused)
//!                      SETTLE        settle the prize that has been deposited
//!   2 Vote           each eligible member votes once. Unanimous below six members;
//!                    simple majority at six or more, ties broken by the house's vote
//!                    (a tie with the house recused fails).
//!   3 Execute        anyone executes a PASSED proposal; the program does exactly what passed.
//!   4 Deposit        anyone deposits a prize; operating money comes only from the house,
//!                    and expenses are paid only out of it.
//!   5 Claim          anyone triggers a member's payout to that member's own wallet.
//!   6 Sweep          after every claim, the remainder (dust, unspent operating money)
//!                    returns to the house wallet.
//!
//! What this makes true on chain:
//!   - nothing happens to the roster, the money or the ledger without a vote passing;
//!   - nobody votes on their own contribution;
//!   - expenses come only out of money the house fronted, so repaying it is honest;
//!   - the split, the qualifying bar and the weights are fixed when the project opens;
//!   - every member is counted at settlement, or settlement fails;
//!   - each member is paid to their own wallet, once.
//!
//! This program records shares of a prize and votes on a project's spending, roster and
//! ledger. It mints no token, and nothing in it is transferable.
//!
//! STATUS: devnet prototype. Not audited. Not deployed. Do not hold real prize money in it
//! until it has been reviewed (see README).

pub mod rules;

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};

use rules::{Outcome, MAX_KINDS};

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

pub const PROJECT_TAG: u8 = 11;
pub const MEMBER_TAG: u8 = 12;
pub const PROPOSAL_TAG: u8 = 13;
pub const VOTE_TAG: u8 = 14;

pub const PROJECT_LEN: usize = 512;
pub const MEMBER_LEN: usize = 192;
pub const PROPOSAL_LEN: usize = 192;
pub const VOTE_LEN: usize = 8;

pub const MAX_ID_LEN: usize = 32;
pub const MAX_FOUNDERS: usize = 16;

pub const KIND_ADD_MEMBER: u8 = 1;
pub const KIND_EXPENSE: u8 = 2;
pub const KIND_CONTRIBUTION: u8 = 3;
pub const KIND_SETTLE: u8 = 4;

pub const STATUS_PENDING: u8 = 0;
pub const STATUS_PASSED: u8 = 1;
pub const STATUS_FAILED: u8 = 2;
pub const STATUS_EXECUTED: u8 = 3;

pub const PROJECT_OPEN: u8 = 0;
pub const PROJECT_SETTLED: u8 = 1;
pub const PROJECT_SWEPT: u8 = 2;

// ---------------------------------------------------------------- errors

#[repr(u32)]
pub enum CompactError {
    BadTag = 1,
    NotHouse = 2,
    NotMember = 3,
    BadPda = 4,
    ProjectClosed = 5,
    ProjectNotSettled = 6,
    BadParams = 7,
    NotPassed = 8,
    AlreadyDecided = 9,
    NotEligibleToVote = 10,
    InsufficientOperatingFunds = 11,
    InsufficientTreasury = 12,
    RosterIncomplete = 13,
    AlreadyClaimed = 14,
    ClaimsOutstanding = 15,
    WrongRecipient = 16,
    Overflow = 17,
}

impl From<CompactError> for ProgramError {
    fn from(e: CompactError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

// ---------------------------------------------------------------- state

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Project {
    pub tag: u8,
    pub bump: u8,
    pub house: Pubkey,
    /// The organizer's personal wallet, paid the organizer's fee off the top.
    pub organizer: Pubkey,
    pub organizer_fee_bps: u64,
    pub id: String,
    /// Weight per contribution kind. Kind 0 = accepted contribution (counts for qualifying).
    pub weights: [u64; MAX_KINDS],
    pub min_qualify: u32,
    pub majority_from: u32,
    pub equal_split_bps: u64,
    pub member_count: u32,
    pub proposal_count: u32,
    pub operating_deposits: u64,
    pub prize_deposits: u64,
    pub expenses_paid: u64,
    pub status: u8,
    // written at settlement
    pub effective_min: u32,
    pub qualifiers: u32,
    pub total_points: u64,
    pub organizer_fee: u64,
    pub organizer_paid: bool,
    pub equal_share: u64,
    pub reimbursement: u64,
    pub bounty_pool: u64,
    pub claims_outstanding: u32,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Member {
    pub tag: u8,
    pub bump: u8,
    pub project: Pubkey,
    pub wallet: Pubkey,
    /// Join order: 0 is the house.
    pub seq: u32,
    pub is_house: bool,
    /// Units logged per contribution kind.
    pub units: [u64; MAX_KINDS],
    pub claimed: bool,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Proposal {
    pub tag: u8,
    pub bump: u8,
    pub project: Pubkey,
    pub index: u32,
    pub kind: u8,
    /// New member's wallet, the expense recipient, or the contributing member's wallet.
    pub target: Pubkey,
    /// Expense lamports, or contribution units.
    pub amount: u64,
    /// For CONTRIBUTION: which contribution kind the units count toward.
    pub contrib_kind: u8,
    /// sha256 of the off-chain description (the envelope, the invoice, the reason).
    pub memo: [u8; 32],
    /// Members with seq < roster_cutoff may vote: the roster as it stood when proposed.
    pub roster_cutoff: u32,
    /// How many of them are eligible (the roster, less a recused contributor).
    pub electorate: u32,
    pub yes: u32,
    pub no: u32,
    /// 0 not yet, 1 yes, 2 no.
    pub house_vote: u8,
    pub status: u8,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct VoteRecord {
    pub tag: u8,
    pub yes: bool,
}

// ---------------------------------------------------------------- instructions

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum Instruction {
    /// Accounts: [house (signer, payer), project PDA, system program,
    ///            member PDA for the house, then a member PDA for each founder]
    CreateProject {
        id: String,
        organizer: Pubkey,
        organizer_fee_bps: u64,
        weights: [u64; MAX_KINDS],
        min_qualify: u32,
        majority_from: u32,
        equal_split_bps: u64,
        founders: Vec<Pubkey>,
    },
    /// Accounts: [proposer (signer, payer), proposer member PDA, project, proposal PDA, system program,
    ///            CONTRIBUTION only: the contributing member's PDA]
    Propose { kind: u8, target: Pubkey, amount: u64, contrib_kind: u8, memo: [u8; 32] },
    /// Accounts: [voter (signer, payer), voter member PDA, project, proposal, vote record PDA, system program]
    Vote { yes: bool },
    /// Accounts: [executor (signer, payer), project, proposal, system program, then:
    ///            ADD_MEMBER: new member PDA | EXPENSE: recipient (writable) |
    ///            CONTRIBUTION: the member PDA | SETTLE: every member PDA of the project]
    Execute,
    /// Accounts: [depositor (signer), project, system program]
    Deposit { amount: u64, prize: bool },
    /// Accounts: [project, member PDA, member wallet (writable)]
    Claim,
    /// Accounts: [project, house wallet (writable)]
    Sweep,
    /// Accounts: [project, organizer wallet (writable)]
    ClaimOrganizer,
}

// ---------------------------------------------------------------- PDAs

pub fn project_pda(program_id: &Pubkey, house: &Pubkey, id: &str) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"project", house.as_ref(), id.as_bytes()], program_id)
}
pub fn member_pda(program_id: &Pubkey, project: &Pubkey, wallet: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"member", project.as_ref(), wallet.as_ref()], program_id)
}
pub fn proposal_pda(program_id: &Pubkey, project: &Pubkey, index: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"proposal", project.as_ref(), &index.to_le_bytes()], program_id)
}
pub fn vote_pda(program_id: &Pubkey, proposal: &Pubkey, wallet: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vote", proposal.as_ref(), wallet.as_ref()], program_id)
}

// ---------------------------------------------------------------- entry

pub fn process_instruction(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let ix = Instruction::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;
    match ix {
        Instruction::CreateProject { id, organizer, organizer_fee_bps, weights, min_qualify, majority_from, equal_split_bps, founders } => {
            create_project(program_id, accounts, id, organizer, organizer_fee_bps, weights, min_qualify, majority_from, equal_split_bps, founders)
        }
        Instruction::Propose { kind, target, amount, contrib_kind, memo } => {
            propose(program_id, accounts, kind, target, amount, contrib_kind, memo)
        }
        Instruction::Vote { yes } => vote(program_id, accounts, yes),
        Instruction::Execute => execute(program_id, accounts),
        Instruction::Deposit { amount, prize } => deposit(program_id, accounts, amount, prize),
        Instruction::Claim => claim(program_id, accounts),
        Instruction::Sweep => sweep(program_id, accounts),
        Instruction::ClaimOrganizer => claim_organizer(program_id, accounts),
    }
}

// ---------------------------------------------------------------- helpers

fn load<T: BorshDeserialize>(acc: &AccountInfo, program_id: &Pubkey, tag: u8) -> Result<T, ProgramError> {
    if acc.owner != program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    let data = acc.try_borrow_data()?;
    if data.first() != Some(&tag) {
        return Err(CompactError::BadTag.into());
    }
    T::deserialize(&mut &data[..]).map_err(|_| ProgramError::InvalidAccountData)
}

fn store<T: BorshSerialize>(acc: &AccountInfo, value: &T) -> ProgramResult {
    let mut data = acc.try_borrow_mut_data()?;
    let mut slice: &mut [u8] = &mut data;
    value.serialize(&mut slice).map_err(|_| ProgramError::AccountDataTooSmall)
}

fn create_pda<'a>(
    payer: &AccountInfo<'a>,
    target: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    program_id: &Pubkey,
    len: usize,
    seeds: &[&[u8]],
) -> ProgramResult {
    if !target.data_is_empty() || target.lamports() > 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let rent = Rent::get()?.minimum_balance(len);
    invoke_signed(
        &system_instruction::create_account(payer.key, target.key, rent, len as u64, program_id),
        &[payer.clone(), target.clone(), system.clone()],
        &[seeds],
    )
}

fn require_signer(acc: &AccountInfo) -> ProgramResult {
    if !acc.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}

fn require_system(acc: &AccountInfo) -> ProgramResult {
    if acc.key != &system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    Ok(())
}

fn load_project(acc: &AccountInfo, program_id: &Pubkey) -> Result<Project, ProgramError> {
    let p: Project = load(acc, program_id, PROJECT_TAG)?;
    let (pda, _) = project_pda(program_id, &p.house, &p.id);
    if &pda != acc.key {
        return Err(CompactError::BadPda.into());
    }
    Ok(p)
}

fn load_member(acc: &AccountInfo, program_id: &Pubkey, project: &Pubkey) -> Result<Member, ProgramError> {
    let m: Member = load(acc, program_id, MEMBER_TAG)?;
    let (pda, _) = member_pda(program_id, project, &m.wallet);
    if &pda != acc.key || &m.project != project {
        return Err(CompactError::BadPda.into());
    }
    Ok(m)
}

/// Lamports the project PDA holds above its rent-exempt floor.
fn treasury_available(project_acc: &AccountInfo) -> Result<u64, ProgramError> {
    let floor = Rent::get()?.minimum_balance(project_acc.data_len());
    Ok(project_acc.lamports().saturating_sub(floor))
}

/// Move lamports out of a program-owned account.
fn pay_out(from: &AccountInfo, to: &AccountInfo, amount: u64) -> ProgramResult {
    if amount == 0 {
        return Ok(());
    }
    **from.try_borrow_mut_lamports()? = from.lamports().checked_sub(amount).ok_or(CompactError::InsufficientTreasury)?;
    **to.try_borrow_mut_lamports()? = to.lamports().checked_add(amount).ok_or(CompactError::Overflow)?;
    Ok(())
}

fn new_member(project: &Pubkey, wallet: &Pubkey, seq: u32, is_house: bool, bump: u8) -> Member {
    Member { tag: MEMBER_TAG, bump, project: *project, wallet: *wallet, seq, is_house, units: [0; MAX_KINDS], claimed: false }
}

// ---------------------------------------------------------------- 0 CreateProject

#[allow(clippy::too_many_arguments)]
fn create_project(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    id: String,
    organizer: Pubkey,
    organizer_fee_bps: u64,
    weights: [u64; MAX_KINDS],
    min_qualify: u32,
    majority_from: u32,
    equal_split_bps: u64,
    founders: Vec<Pubkey>,
) -> ProgramResult {
    let it = &mut accounts.iter();
    let house = next_account_info(it)?;
    let project_acc = next_account_info(it)?;
    let system = next_account_info(it)?;
    require_signer(house)?;
    require_system(system)?;

    if id.is_empty() || id.len() > MAX_ID_LEN || founders.len() > MAX_FOUNDERS {
        return Err(CompactError::BadParams.into());
    }
    // the organizer's fee (ruled 27 Oct 2026): fixed at opening, visible to every member,
    // never more than 1%, and never nothing: every Scrypt contract carries the tithe
    if min_qualify == 0 || majority_from < 2 || equal_split_bps > rules::BPS || organizer_fee_bps == 0 || organizer_fee_bps > 100 {
        return Err(CompactError::BadParams.into());
    }
    for (i, f) in founders.iter().enumerate() {
        if f == house.key || founders[..i].contains(f) {
            return Err(CompactError::BadParams.into());
        }
    }

    let (pda, bump) = project_pda(program_id, house.key, &id);
    if &pda != project_acc.key {
        return Err(CompactError::BadPda.into());
    }
    create_pda(house, project_acc, system, program_id, PROJECT_LEN, &[b"project", house.key.as_ref(), id.as_bytes(), &[bump]])?;

    // Roster: the house first, then each founder.
    let roster: Vec<Pubkey> = std::iter::once(*house.key).chain(founders.iter().copied()).collect();
    for (seq, wallet) in roster.iter().enumerate() {
        let member_acc = next_account_info(it)?;
        let (mpda, mbump) = member_pda(program_id, project_acc.key, wallet);
        if &mpda != member_acc.key {
            return Err(CompactError::BadPda.into());
        }
        create_pda(house, member_acc, system, program_id, MEMBER_LEN, &[b"member", project_acc.key.as_ref(), wallet.as_ref(), &[mbump]])?;
        store(member_acc, &new_member(project_acc.key, wallet, seq as u32, seq == 0, mbump))?;
    }

    let project = Project {
        tag: PROJECT_TAG,
        bump,
        house: *house.key,
        organizer,
        organizer_fee_bps,
        id,
        weights,
        min_qualify,
        majority_from,
        equal_split_bps,
        member_count: roster.len() as u32,
        proposal_count: 0,
        operating_deposits: 0,
        prize_deposits: 0,
        expenses_paid: 0,
        status: PROJECT_OPEN,
        effective_min: 0,
        qualifiers: 0,
        total_points: 0,
        organizer_fee: 0,
        organizer_paid: false,
        equal_share: 0,
        reimbursement: 0,
        bounty_pool: 0,
        claims_outstanding: 0,
    };
    store(project_acc, &project)?;
    msg!("compact: project {} opened with {} members", project.id, project.member_count);
    Ok(())
}

// ---------------------------------------------------------------- 1 Propose

#[allow(clippy::too_many_arguments)]
fn propose(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    kind: u8,
    target: Pubkey,
    amount: u64,
    contrib_kind: u8,
    memo: [u8; 32],
) -> ProgramResult {
    let it = &mut accounts.iter();
    let proposer = next_account_info(it)?;
    let proposer_member_acc = next_account_info(it)?;
    let project_acc = next_account_info(it)?;
    let proposal_acc = next_account_info(it)?;
    let system = next_account_info(it)?;
    require_signer(proposer)?;
    require_system(system)?;

    let mut project = load_project(project_acc, program_id)?;
    if project.status != PROJECT_OPEN {
        return Err(CompactError::ProjectClosed.into());
    }
    let m = load_member(proposer_member_acc, program_id, project_acc.key)?;
    if &m.wallet != proposer.key {
        return Err(CompactError::NotMember.into());
    }

    let roster_cutoff = project.member_count;
    let electorate = match kind {
        // a wallet that is already a member fails at Execute: its member PDA already exists
        KIND_ADD_MEMBER if amount == 0 => roster_cutoff,
        KIND_EXPENSE if amount > 0 => roster_cutoff,
        KIND_CONTRIBUTION if amount > 0 && (contrib_kind as usize) < MAX_KINDS => {
            // the contributor must already be a member, and is recused from the vote
            let subject_acc = next_account_info(it)?;
            let subject = load_member(subject_acc, program_id, project_acc.key)?;
            if subject.wallet != target {
                return Err(CompactError::BadParams.into());
            }
            roster_cutoff - 1
        }
        KIND_SETTLE if amount == 0 && project.prize_deposits > 0 => roster_cutoff,
        _ => return Err(CompactError::BadParams.into()),
    };
    if electorate == 0 {
        return Err(CompactError::BadParams.into());
    }

    let index = project.proposal_count;
    let (pda, bump) = proposal_pda(program_id, project_acc.key, index);
    if &pda != proposal_acc.key {
        return Err(CompactError::BadPda.into());
    }
    create_pda(proposer, proposal_acc, system, program_id, PROPOSAL_LEN, &[b"proposal", project_acc.key.as_ref(), &index.to_le_bytes(), &[bump]])?;
    let proposal = Proposal {
        tag: PROPOSAL_TAG,
        bump,
        project: *project_acc.key,
        index,
        kind,
        target,
        amount,
        contrib_kind,
        memo,
        roster_cutoff,
        electorate,
        yes: 0,
        no: 0,
        house_vote: 0,
        status: STATUS_PENDING,
    };
    store(proposal_acc, &proposal)?;
    project.proposal_count = index.checked_add(1).ok_or(CompactError::Overflow)?;
    store(project_acc, &project)?;
    msg!("compact: proposal {} kind {} electorate {}", index, kind, electorate);
    Ok(())
}

// ---------------------------------------------------------------- 2 Vote

fn vote(program_id: &Pubkey, accounts: &[AccountInfo], yes: bool) -> ProgramResult {
    let it = &mut accounts.iter();
    let voter = next_account_info(it)?;
    let voter_member_acc = next_account_info(it)?;
    let project_acc = next_account_info(it)?;
    let proposal_acc = next_account_info(it)?;
    let vote_acc = next_account_info(it)?;
    let system = next_account_info(it)?;
    require_signer(voter)?;
    require_system(system)?;

    let project = load_project(project_acc, program_id)?;
    if project.status != PROJECT_OPEN {
        return Err(CompactError::ProjectClosed.into());
    }
    let m = load_member(voter_member_acc, program_id, project_acc.key)?;
    if &m.wallet != voter.key {
        return Err(CompactError::NotMember.into());
    }
    let mut p: Proposal = load(proposal_acc, program_id, PROPOSAL_TAG)?;
    let (ppda, _) = proposal_pda(program_id, project_acc.key, p.index);
    if &ppda != proposal_acc.key || &p.project != project_acc.key {
        return Err(CompactError::BadPda.into());
    }
    if p.status != STATUS_PENDING {
        return Err(CompactError::AlreadyDecided.into());
    }
    if m.seq >= p.roster_cutoff {
        return Err(CompactError::NotEligibleToVote.into());
    }
    if p.kind == KIND_CONTRIBUTION && &p.target == voter.key {
        // nobody votes on their own contribution
        return Err(CompactError::NotEligibleToVote.into());
    }

    // One vote per member: the vote record PDA can only be created once.
    let (vpda, vbump) = vote_pda(program_id, proposal_acc.key, voter.key);
    if &vpda != vote_acc.key {
        return Err(CompactError::BadPda.into());
    }
    create_pda(voter, vote_acc, system, program_id, VOTE_LEN, &[b"vote", proposal_acc.key.as_ref(), voter.key.as_ref(), &[vbump]])?;
    store(vote_acc, &VoteRecord { tag: VOTE_TAG, yes })?;

    if yes {
        p.yes += 1;
    } else {
        p.no += 1;
    }
    if m.is_house {
        p.house_vote = if yes { 1 } else { 2 };
    }
    let house_vote = match p.house_vote {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    };
    p.status = match rules::tally(p.electorate, p.yes, p.no, house_vote, project.majority_from) {
        Outcome::Pending => STATUS_PENDING,
        Outcome::Passed => STATUS_PASSED,
        Outcome::Failed => STATUS_FAILED,
    };
    store(proposal_acc, &p)?;
    msg!("compact: proposal {} yes {} no {} status {}", p.index, p.yes, p.no, p.status);
    Ok(())
}

// ---------------------------------------------------------------- 3 Execute

fn execute(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let executor = next_account_info(it)?;
    let project_acc = next_account_info(it)?;
    let proposal_acc = next_account_info(it)?;
    let system = next_account_info(it)?;
    require_signer(executor)?;
    require_system(system)?;

    let mut project = load_project(project_acc, program_id)?;
    if project.status != PROJECT_OPEN {
        return Err(CompactError::ProjectClosed.into());
    }
    let mut p: Proposal = load(proposal_acc, program_id, PROPOSAL_TAG)?;
    let (ppda, _) = proposal_pda(program_id, project_acc.key, p.index);
    if &ppda != proposal_acc.key || &p.project != project_acc.key {
        return Err(CompactError::BadPda.into());
    }
    if p.status != STATUS_PASSED {
        return Err(CompactError::NotPassed.into());
    }

    match p.kind {
        KIND_ADD_MEMBER => {
            let member_acc = next_account_info(it)?;
            let (mpda, mbump) = member_pda(program_id, project_acc.key, &p.target);
            if &mpda != member_acc.key {
                return Err(CompactError::BadPda.into());
            }
            // create_pda refuses an account that already exists, so nobody is added twice
            create_pda(executor, member_acc, system, program_id, MEMBER_LEN, &[b"member", project_acc.key.as_ref(), p.target.as_ref(), &[mbump]])?;
            store(member_acc, &new_member(project_acc.key, &p.target, project.member_count, false, mbump))?;
            project.member_count = project.member_count.checked_add(1).ok_or(CompactError::Overflow)?;
        }
        KIND_EXPENSE => {
            let recipient = next_account_info(it)?;
            if recipient.key != &p.target {
                return Err(CompactError::WrongRecipient.into());
            }
            // Expenses are paid only out of operating money the house fronted.
            let fronted_unspent = project.operating_deposits.saturating_sub(project.expenses_paid);
            if p.amount > fronted_unspent {
                return Err(CompactError::InsufficientOperatingFunds.into());
            }
            if p.amount > treasury_available(project_acc)? {
                return Err(CompactError::InsufficientTreasury.into());
            }
            pay_out(project_acc, recipient, p.amount)?;
            project.expenses_paid = project.expenses_paid.checked_add(p.amount).ok_or(CompactError::Overflow)?;
        }
        KIND_CONTRIBUTION => {
            let member_acc = next_account_info(it)?;
            let mut m = load_member(member_acc, program_id, project_acc.key)?;
            if m.wallet != p.target {
                return Err(CompactError::WrongRecipient.into());
            }
            let k = p.contrib_kind as usize;
            m.units[k] = m.units[k].checked_add(p.amount).ok_or(CompactError::Overflow)?;
            store(member_acc, &m)?;
        }
        KIND_SETTLE => {
            // every account after the first four is a member PDA
            settle_now(program_id, project_acc, &mut project, &accounts[4..])?;
        }
        _ => return Err(CompactError::BadParams.into()),
    }
    p.status = STATUS_EXECUTED;
    store(proposal_acc, &p)?;
    store(project_acc, &project)?;
    msg!("compact: proposal {} executed", p.index);
    Ok(())
}

/// Settle the deposited prize. Every member account must be presented exactly once.
fn settle_now(program_id: &Pubkey, project_acc: &AccountInfo, project: &mut Project, rest: &[AccountInfo]) -> ProgramResult {
    if project.prize_deposits == 0 {
        return Err(CompactError::BadParams.into());
    }
    let mut members: Vec<Member> = Vec::with_capacity(project.member_count as usize);
    let mut seen = vec![false; project.member_count as usize];
    for acc in rest {
        let m = load_member(acc, program_id, project_acc.key)?;
        let s = m.seq as usize;
        if s >= seen.len() || seen[s] {
            return Err(CompactError::RosterIncomplete.into());
        }
        seen[s] = true;
        members.push(m);
    }
    if seen.iter().any(|x| !x) {
        return Err(CompactError::RosterIncomplete.into());
    }

    let max_collab = members.iter().filter(|m| !m.is_house).map(|m| m.units[0]).max().unwrap_or(0);
    let max_collab = u32::try_from(max_collab).unwrap_or(u32::MAX);
    let eff_min = rules::effective_min(project.min_qualify, max_collab);
    let qualifiers = members
        .iter()
        .filter(|m| rules::qualifies(m.is_house, u32::try_from(m.units[0]).unwrap_or(u32::MAX), eff_min))
        .count() as u32;
    let total_points = members.iter().fold(0u64, |acc, m| acc.saturating_add(rules::points(&m.units, &project.weights)));

    let s = rules::settle(project.prize_deposits, project.organizer_fee_bps, project.expenses_paid, qualifiers, project.equal_split_bps);

    // The treasury must actually hold what settlement promises to pay.
    let promised = s
        .equal_share
        .checked_mul(qualifiers as u64)
        .and_then(|x| x.checked_add(s.reimbursement))
        .and_then(|x| x.checked_add(s.bounty_pool))
        .and_then(|x| x.checked_add(s.organizer_fee))
        .ok_or(CompactError::Overflow)?;
    if promised > treasury_available(project_acc)? {
        return Err(CompactError::InsufficientTreasury.into());
    }

    project.status = PROJECT_SETTLED;
    project.effective_min = eff_min;
    project.qualifiers = qualifiers;
    project.total_points = total_points;
    project.organizer_fee = s.organizer_fee;
    project.organizer_paid = s.organizer_fee == 0;
    project.equal_share = s.equal_share;
    project.reimbursement = s.reimbursement;
    project.bounty_pool = s.bounty_pool;
    project.claims_outstanding = project.member_count;
    msg!(
        "compact: settled prize {} | qualifiers {} (bar {}) | equal {} each | reimburse {} | bounty {} over {} points",
        project.prize_deposits, qualifiers, eff_min, s.equal_share, s.reimbursement, s.bounty_pool, total_points
    );
    Ok(())
}

// ---------------------------------------------------------------- 4 Deposit

fn deposit(program_id: &Pubkey, accounts: &[AccountInfo], amount: u64, prize: bool) -> ProgramResult {
    let it = &mut accounts.iter();
    let depositor = next_account_info(it)?;
    let project_acc = next_account_info(it)?;
    let system = next_account_info(it)?;
    require_signer(depositor)?;
    require_system(system)?;

    let mut project = load_project(project_acc, program_id)?;
    if project.status != PROJECT_OPEN {
        return Err(CompactError::ProjectClosed.into());
    }
    if amount == 0 {
        return Err(CompactError::BadParams.into());
    }
    if !prize && &project.house != depositor.key {
        // operating money is what expenses are reimbursed from; only the house fronts it
        return Err(CompactError::NotHouse.into());
    }
    invoke(
        &system_instruction::transfer(depositor.key, project_acc.key, amount),
        &[depositor.clone(), project_acc.clone(), system.clone()],
    )?;
    if prize {
        project.prize_deposits = project.prize_deposits.checked_add(amount).ok_or(CompactError::Overflow)?;
    } else {
        project.operating_deposits = project.operating_deposits.checked_add(amount).ok_or(CompactError::Overflow)?;
    }
    store(project_acc, &project)?;
    Ok(())
}

// ---------------------------------------------------------------- 5 Claim

fn claim(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let project_acc = next_account_info(it)?;
    let member_acc = next_account_info(it)?;
    let wallet = next_account_info(it)?;

    let mut project = load_project(project_acc, program_id)?;
    if project.status != PROJECT_SETTLED {
        return Err(CompactError::ProjectNotSettled.into());
    }
    let mut m = load_member(member_acc, program_id, project_acc.key)?;
    if &m.wallet != wallet.key {
        return Err(CompactError::WrongRecipient.into());
    }
    if m.claimed {
        return Err(CompactError::AlreadyClaimed.into());
    }

    let accepted = u32::try_from(m.units[0]).unwrap_or(u32::MAX);
    let mut amount = 0u64;
    if rules::qualifies(m.is_house, accepted, project.effective_min) {
        amount = amount.checked_add(project.equal_share).ok_or(CompactError::Overflow)?;
    }
    let pts = rules::points(&m.units, &project.weights);
    amount = amount
        .checked_add(rules::bounty_share(project.bounty_pool, pts, project.total_points))
        .ok_or(CompactError::Overflow)?;
    if m.is_house {
        amount = amount.checked_add(project.reimbursement).ok_or(CompactError::Overflow)?;
    }

    pay_out(project_acc, wallet, amount)?;
    m.claimed = true;
    store(member_acc, &m)?;
    project.claims_outstanding -= 1;
    store(project_acc, &project)?;
    msg!("compact: member {} claimed {}", m.seq, amount);
    Ok(())
}

// ---------------------------------------------------------------- 6 Sweep

fn sweep(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let project_acc = next_account_info(it)?;
    let house = next_account_info(it)?;

    let mut project = load_project(project_acc, program_id)?;
    if &project.house != house.key {
        return Err(CompactError::WrongRecipient.into());
    }
    if project.status != PROJECT_SETTLED {
        return Err(CompactError::ProjectNotSettled.into());
    }
    if project.claims_outstanding != 0 || !project.organizer_paid {
        return Err(CompactError::ClaimsOutstanding.into());
    }
    let rest = treasury_available(project_acc)?;
    pay_out(project_acc, house, rest)?;
    project.status = PROJECT_SWEPT;
    store(project_acc, &project)?;
    msg!("compact: swept {} to the house", rest);
    Ok(())
}

// ---------------------------------------------------------------- 7 ClaimOrganizer

fn claim_organizer(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let project_acc = next_account_info(it)?;
    let organizer = next_account_info(it)?;

    let mut project = load_project(project_acc, program_id)?;
    if &project.organizer != organizer.key {
        return Err(CompactError::WrongRecipient.into());
    }
    if project.status != PROJECT_SETTLED {
        return Err(CompactError::ProjectNotSettled.into());
    }
    if project.organizer_paid {
        return Err(CompactError::AlreadyClaimed.into());
    }
    pay_out(project_acc, organizer, project.organizer_fee)?;
    project.organizer_paid = true;
    store(project_acc, &project)?;
    msg!("compact: organizer's fee {} paid", project.organizer_fee);
    Ok(())
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn accounts_fit_their_allocations() {
        let p = Project {
            tag: PROJECT_TAG, bump: 255, house: Pubkey::new_unique(), organizer: Pubkey::new_unique(),
            organizer_fee_bps: 100, id: "x".repeat(MAX_ID_LEN),
            weights: [u64::MAX; MAX_KINDS], min_qualify: 3, majority_from: 6, equal_split_bps: 7_500,
            member_count: u32::MAX, proposal_count: u32::MAX, operating_deposits: u64::MAX,
            prize_deposits: u64::MAX, expenses_paid: u64::MAX, status: 2, effective_min: 3,
            qualifiers: 10, total_points: u64::MAX, organizer_fee: u64::MAX, organizer_paid: true, equal_share: u64::MAX, reimbursement: u64::MAX,
            bounty_pool: u64::MAX, claims_outstanding: 10,
        };
        assert!(borsh::to_vec(&p).unwrap().len() <= PROJECT_LEN);
        let m = new_member(&Pubkey::new_unique(), &Pubkey::new_unique(), 9, false, 255);
        assert!(borsh::to_vec(&m).unwrap().len() <= MEMBER_LEN);
        let pr = Proposal {
            tag: PROPOSAL_TAG, bump: 1, project: Pubkey::new_unique(), index: 1, kind: 2,
            target: Pubkey::new_unique(), amount: 1, contrib_kind: 0, memo: [7; 32], roster_cutoff: 4,
            electorate: 4, yes: 1, no: 0,
            house_vote: 1, status: 0,
        };
        assert!(borsh::to_vec(&pr).unwrap().len() <= PROPOSAL_LEN);
        assert!(borsh::to_vec(&VoteRecord { tag: VOTE_TAG, yes: true }).unwrap().len() <= VOTE_LEN);
    }

    #[test]
    fn instructions_round_trip() {
        let ix = Instruction::CreateProject {
            id: "vesuvius".into(),
            organizer: Pubkey::new_unique(),
            organizer_fee_bps: 100,
            weights: [5, 1, 0, 0, 0, 0, 0, 0],
            min_qualify: 3,
            majority_from: 6,
            equal_split_bps: 7_500,
            founders: vec![Pubkey::new_unique(), Pubkey::new_unique(), Pubkey::new_unique()],
        };
        let bytes = borsh::to_vec(&ix).unwrap();
        assert!(matches!(Instruction::try_from_slice(&bytes).unwrap(), Instruction::CreateProject { .. }));
    }
}
