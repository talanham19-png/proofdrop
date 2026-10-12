//! ProofDrop v2: provably fair puzzle contests with an escrowed purse.
//!
//! The same commitments as the Memo protocol (v1), now enforced by a program:
//!   commit = sha256("<id>|<salt>|<ANSWER>")
//!   entry  = sha256("<id>|<solver base58>|<ANSWER>")
//!
//! Life of a contest
//!   0 CreateContest  house locks commit + purse in the contest PDA (escrow)
//!   1 Submit         one sealed entry per wallet, only while slot <= close_slot
//!   2 Reveal         house reveals salt+answer after close; rejected unless it matches the commit
//!   3 Settle         anyone settles an entry after the reveal: the program recomputes its hash from the
//!                    revealed answer and the entry's own signer, marks it settled, and keeps the earliest
//!                    correct entry as leader. Each entry settles exactly once.
//!   4 Payout         once EVERY entry is settled, anyone triggers payment: the purse goes to the earliest
//!                    correct entry, or back to the house only if every entry was proven wrong.
//!
//! Rules this makes true on chain:
//!   - the house cannot change the answer (reveal must match the commit);
//!   - the house wallet cannot enter its own contest;
//!   - the earliest correct entry wins, whoever settles it, and no correct entry can be skipped;
//!   - the purse returns to the house only if every entry is wrong;
//!   - if the house never reveals, the purse can never leave escrow, so withholding saves it nothing.

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    hash::hashv,
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

pub const CONTEST_TAG: u8 = 1;
pub const ENTRY_TAG: u8 = 2;
pub const CONTEST_LEN: usize = 256;
pub const ENTRY_LEN: usize = 112;
pub const NONE_SLOT: u64 = u64::MAX;

// Contest layout (fixed offsets)
const C_TAG: usize = 0; //            u8
const C_BUMP: usize = 1; //           u8
const C_HOUSE: usize = 2; //          32
const C_ID_LEN: usize = 34; //        u8
const C_ID: usize = 35; //            16
const C_COMMIT: usize = 51; //        32
const C_CLOSE: usize = 83; //         u64  last slot that accepts entries
const C_CLAIM_SLOTS: usize = 91; //   u64  reserved (v2.0 claim window; unused since v2.1)
const C_PURSE: usize = 99; //         u64  lamports in escrow
const C_REVEALED: usize = 107; //     u8
const C_REVEAL_SLOT: usize = 108; //  u64
const C_ANS_LEN: usize = 116; //      u8
const C_ANS: usize = 117; //          32
const C_BEST: usize = 149; //         32   best solver so far
const C_BEST_SLOT: usize = 181; //    u64  NONE_SLOT when nobody
const C_ENTRIES: usize = 189; //      u32
const C_PAID: usize = 193; //         u8   0 open, 1 paid to winner, 2 refunded to house
const C_BEST_IDX: usize = 194; //     u32  submission index of the leader (slot tie-break)
const C_SETTLED: usize = 198; //      u32  entries settled so far (v2.1)

// Entry layout
const E_TAG: usize = 0;
const E_BUMP: usize = 1;
const E_CONTEST: usize = 2; //   32
const E_SOLVER: usize = 34; //   32
const E_HASH: usize = 66; //     32
const E_SLOT: usize = 98; //     u64
const E_IDX: usize = 106; //     u32  submission order within the contest
const E_SETTLED: usize = 110; // u8   1 once settled (v2.1)
const E_CORRECT: usize = 111; // u8   1 if settled correct (v2.1)

#[derive(Debug)]
#[repr(u32)]
pub enum PdError {
    BadInstruction = 1,
    BadAccount,
    TooLate,
    TooEarly,
    AlreadyRevealed,
    NotRevealed,
    RevealMismatch,
    WrongAnswer,
    NotEarlier,
    ClaimWindowOpen,
    ClaimWindowClosed,
    AlreadyPaid,
    WrongWinner,
    NotHouse,
    NotAllSettled,
    HouseCannotEnter,
    AlreadySettled,
}
impl From<PdError> for ProgramError {
    fn from(e: PdError) -> Self { ProgramError::Custom(e as u32) }
}

fn rd_u64(d: &[u8], o: usize) -> u64 { u64::from_le_bytes(d[o..o + 8].try_into().unwrap()) }
fn wr_u64(d: &mut [u8], o: usize, v: u64) { d[o..o + 8].copy_from_slice(&v.to_le_bytes()) }
fn rd_u32(d: &[u8], o: usize) -> u32 { u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) }
fn rd_pk(d: &[u8], o: usize) -> Pubkey { Pubkey::new_from_array(d[o..o + 32].try_into().unwrap()) }

/// Same normalisation as the browser: keep A-Z and 0-9, upper-case. Input from the house is
/// expected already normalised; we enforce it so the stored answer is canonical.
fn is_normalised(a: &[u8]) -> bool { !a.is_empty() && a.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) }

pub fn contest_pda(program_id: &Pubkey, house: &Pubkey, id: &[u8]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"contest", house.as_ref(), id], program_id)
}
pub fn entry_pda(program_id: &Pubkey, contest: &Pubkey, solver: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"entry", contest.as_ref(), solver.as_ref()], program_id)
}

pub fn process_instruction(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (&tag, rest) = data.split_first().ok_or(PdError::BadInstruction)?;
    match tag {
        0 => create_contest(program_id, accounts, rest),
        1 => submit(program_id, accounts, rest),
        2 => reveal(program_id, accounts, rest),
        3 => settle(program_id, accounts),
        4 => payout(program_id, accounts),
        _ => Err(PdError::BadInstruction.into()),
    }
}

fn load_contest<'a>(program_id: &Pubkey, acc: &AccountInfo<'a>) -> Result<(), ProgramError> {
    if acc.owner != program_id || acc.data_len() != CONTEST_LEN || acc.try_borrow_data()?[C_TAG] != CONTEST_TAG {
        return Err(PdError::BadAccount.into());
    }
    Ok(())
}

/// data: id_len u8 | id bytes (<=16) | commit[32] | close_slot u64 | claim_slots u64 | purse u64
fn create_contest(program_id: &Pubkey, accounts: &[AccountInfo], d: &[u8]) -> ProgramResult {
    let it = &mut accounts.iter();
    let house = next_account_info(it)?;
    let contest = next_account_info(it)?;
    let sys = next_account_info(it)?;
    if !house.is_signer || *sys.key != system_program::id() { return Err(PdError::BadAccount.into()); }

    let id_len = *d.first().ok_or(PdError::BadInstruction)? as usize;
    if id_len == 0 || id_len > 16 || d.len() != 1 + id_len + 32 + 24 { return Err(PdError::BadInstruction.into()); }
    let id = &d[1..1 + id_len];
    let mut p = 1 + id_len;
    let commit: [u8; 32] = d[p..p + 32].try_into().unwrap(); p += 32;
    let close_slot = rd_u64(d, p); p += 8;
    let claim_slots = rd_u64(d, p); p += 8;
    let purse = rd_u64(d, p);

    let (pda, bump) = contest_pda(program_id, house.key, id);
    if pda != *contest.key { return Err(PdError::BadAccount.into()); }
    if close_slot <= Clock::get()?.slot { return Err(PdError::TooLate.into()); }

    let rent = Rent::get()?.minimum_balance(CONTEST_LEN);
    let lamports = rent.checked_add(purse).ok_or(ProgramError::ArithmeticOverflow)?;
    invoke_signed(
        &system_instruction::create_account(house.key, contest.key, lamports, CONTEST_LEN as u64, program_id),
        &[house.clone(), contest.clone(), sys.clone()],
        &[&[b"contest", house.key.as_ref(), id, &[bump]]],
    )?;

    let mut c = contest.try_borrow_mut_data()?;
    c[C_TAG] = CONTEST_TAG;
    c[C_BUMP] = bump;
    c[C_HOUSE..C_HOUSE + 32].copy_from_slice(house.key.as_ref());
    c[C_ID_LEN] = id_len as u8;
    c[C_ID..C_ID + id_len].copy_from_slice(id);
    c[C_COMMIT..C_COMMIT + 32].copy_from_slice(&commit);
    wr_u64(&mut c, C_CLOSE, close_slot);
    wr_u64(&mut c, C_CLAIM_SLOTS, claim_slots);
    wr_u64(&mut c, C_PURSE, purse);
    wr_u64(&mut c, C_BEST_SLOT, NONE_SLOT);
    msg!("ProofDrop: contest created, purse {} lamports, entries close at slot {}", purse, close_slot);
    Ok(())
}

/// data: hash[32] = sha256("<id>|<solver base58>|<ANSWER>")
fn submit(program_id: &Pubkey, accounts: &[AccountInfo], d: &[u8]) -> ProgramResult {
    let it = &mut accounts.iter();
    let solver = next_account_info(it)?;
    let contest = next_account_info(it)?;
    let entry = next_account_info(it)?;
    let sys = next_account_info(it)?;
    if !solver.is_signer || *sys.key != system_program::id() { return Err(PdError::BadAccount.into()); }
    if d.len() != 32 { return Err(PdError::BadInstruction.into()); }
    load_contest(program_id, contest)?;
    if rd_pk(&contest.try_borrow_data()?, C_HOUSE) == *solver.key { return Err(PdError::HouseCannotEnter.into()); }

    let slot = Clock::get()?.slot;
    let idx;
    {
        let mut c = contest.try_borrow_mut_data()?;
        if c[C_REVEALED] != 0 { return Err(PdError::AlreadyRevealed.into()); }
        if slot > rd_u64(&c, C_CLOSE) { return Err(PdError::TooLate.into()); }
        idx = rd_u32(&c, C_ENTRIES);
        c[C_ENTRIES..C_ENTRIES + 4].copy_from_slice(&(idx + 1).to_le_bytes());
    }
    let (pda, bump) = entry_pda(program_id, contest.key, solver.key);
    if pda != *entry.key { return Err(PdError::BadAccount.into()); }
    // create_account fails if the entry already exists: one sealed entry per wallet.
    let rent = Rent::get()?.minimum_balance(ENTRY_LEN);
    invoke_signed(
        &system_instruction::create_account(solver.key, entry.key, rent, ENTRY_LEN as u64, program_id),
        &[solver.clone(), entry.clone(), sys.clone()],
        &[&[b"entry", contest.key.as_ref(), solver.key.as_ref(), &[bump]]],
    )?;
    let mut e = entry.try_borrow_mut_data()?;
    e[E_TAG] = ENTRY_TAG;
    e[E_BUMP] = bump;
    e[E_CONTEST..E_CONTEST + 32].copy_from_slice(contest.key.as_ref());
    e[E_SOLVER..E_SOLVER + 32].copy_from_slice(solver.key.as_ref());
    e[E_HASH..E_HASH + 32].copy_from_slice(d);
    wr_u64(&mut e, E_SLOT, slot);
    e[E_IDX..E_IDX + 4].copy_from_slice(&idx.to_le_bytes());
    msg!("ProofDrop: sealed entry #{} at slot {}", idx, slot);
    Ok(())
}

/// data: salt_len u8 | salt | ans_len u8 | answer (normalised, <=32)
fn reveal(program_id: &Pubkey, accounts: &[AccountInfo], d: &[u8]) -> ProgramResult {
    let it = &mut accounts.iter();
    let house = next_account_info(it)?;
    let contest = next_account_info(it)?;
    load_contest(program_id, contest)?;
    let mut c = contest.try_borrow_mut_data()?;
    if !house.is_signer || rd_pk(&c, C_HOUSE) != *house.key { return Err(PdError::NotHouse.into()); }
    if c[C_REVEALED] != 0 { return Err(PdError::AlreadyRevealed.into()); }
    let slot = Clock::get()?.slot;
    if slot <= rd_u64(&c, C_CLOSE) { return Err(PdError::TooEarly.into()); }

    let sl = *d.first().ok_or(PdError::BadInstruction)? as usize;
    if d.len() < 2 + sl || sl > 64 { return Err(PdError::BadInstruction.into()); }
    let salt = &d[1..1 + sl];
    let al = d[1 + sl] as usize;
    if al == 0 || al > 32 || d.len() != 2 + sl + al { return Err(PdError::BadInstruction.into()); }
    let ans = &d[2 + sl..2 + sl + al];
    if !is_normalised(ans) { return Err(PdError::BadInstruction.into()); }

    let id_len = c[C_ID_LEN] as usize;
    let id = c[C_ID..C_ID + id_len].to_vec();
    let h = hashv(&[&id, b"|", salt, b"|", ans]);
    if h.to_bytes() != c[C_COMMIT..C_COMMIT + 32] { return Err(PdError::RevealMismatch.into()); }

    c[C_REVEALED] = 1;
    wr_u64(&mut c, C_REVEAL_SLOT, slot);
    c[C_ANS_LEN] = al as u8;
    c[C_ANS..C_ANS + al].copy_from_slice(ans);
    msg!("ProofDrop: reveal matches commitment at slot {}", slot);
    Ok(())
}

/// Anyone may settle any entry after the reveal. Wrong entries are counted, correct ones compete
/// for the lead by slot (then submission order). Every entry settles exactly once.
fn settle(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let contest = next_account_info(it)?;
    let entry = next_account_info(it)?;
    load_contest(program_id, contest)?;
    if entry.owner != program_id || entry.data_len() != ENTRY_LEN || !entry.is_writable { return Err(PdError::BadAccount.into()); }
    let mut e = entry.try_borrow_mut_data()?;
    if e[E_TAG] != ENTRY_TAG || rd_pk(&e, E_CONTEST) != *contest.key { return Err(PdError::BadAccount.into()); }
    if e[E_SETTLED] != 0 { return Err(PdError::AlreadySettled.into()); }

    let mut c = contest.try_borrow_mut_data()?;
    if c[C_REVEALED] == 0 { return Err(PdError::NotRevealed.into()); }
    if c[C_PAID] != 0 { return Err(PdError::AlreadyPaid.into()); }
    let solver = rd_pk(&e, E_SOLVER);
    let id_len = c[C_ID_LEN] as usize;
    let al = c[C_ANS_LEN] as usize;
    let solver_b58 = solver.to_string();
    let h = hashv(&[&c[C_ID..C_ID + id_len], b"|", solver_b58.as_bytes(), b"|", &c[C_ANS..C_ANS + al]]);
    let correct = h.to_bytes() == e[E_HASH..E_HASH + 32];

    e[E_SETTLED] = 1;
    e[E_CORRECT] = correct as u8;
    let settled = rd_u32(&c, C_SETTLED) + 1;
    c[C_SETTLED..C_SETTLED + 4].copy_from_slice(&settled.to_le_bytes());

    if correct {
        let es = rd_u64(&e, E_SLOT);
        let bs = rd_u64(&c, C_BEST_SLOT);
        let better = bs == NONE_SLOT || es < bs || (es == bs && rd_u32(&e, E_IDX) < rd_u32(&c, C_BEST_IDX));
        if better {
            c[C_BEST..C_BEST + 32].copy_from_slice(solver.as_ref());
            wr_u64(&mut c, C_BEST_SLOT, es);
            c[C_BEST_IDX..C_BEST_IDX + 4].copy_from_slice(&e[E_IDX..E_IDX + 4]);
        }
        msg!("ProofDrop: entry #{} from {} is CORRECT (slot {}){}", rd_u32(&e, E_IDX), solver_b58, es, if better { "; new leader" } else { "" });
    } else {
        msg!("ProofDrop: entry #{} from {} is wrong", rd_u32(&e, E_IDX), solver_b58);
    }
    msg!("ProofDrop: {} of {} entries settled", settled, rd_u32(&c, C_ENTRIES));
    Ok(())
}

/// Once every entry is settled: pay the purse to the leader, or refund the house if every entry was wrong.
/// accounts: contest (w), recipient (w) — must be the leader, or the house when there is none
fn payout(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let contest = next_account_info(it)?;
    let recipient = next_account_info(it)?;
    load_contest(program_id, contest)?;
    let (purse, to_winner) = {
        let mut c = contest.try_borrow_mut_data()?;
        if c[C_REVEALED] == 0 { return Err(PdError::NotRevealed.into()); }
        if c[C_PAID] != 0 { return Err(PdError::AlreadyPaid.into()); }
        if rd_u32(&c, C_SETTLED) != rd_u32(&c, C_ENTRIES) { return Err(PdError::NotAllSettled.into()); }
        let has_winner = rd_u64(&c, C_BEST_SLOT) != NONE_SLOT;
        let expected = if has_winner { rd_pk(&c, C_BEST) } else { rd_pk(&c, C_HOUSE) };
        if expected != *recipient.key { return Err(PdError::WrongWinner.into()); }
        c[C_PAID] = if has_winner { 1 } else { 2 };
        (rd_u64(&c, C_PURSE), has_winner)
    };
    **contest.try_borrow_mut_lamports()? = contest.lamports().checked_sub(purse).ok_or(ProgramError::InsufficientFunds)?;
    **recipient.try_borrow_mut_lamports()? = recipient.lamports().checked_add(purse).ok_or(ProgramError::ArithmeticOverflow)?;
    msg!("ProofDrop: {} lamports paid to {} ({})", purse, recipient.key, if to_winner { "winner" } else { "every entry was wrong; refunded to house" });
    Ok(())
}
