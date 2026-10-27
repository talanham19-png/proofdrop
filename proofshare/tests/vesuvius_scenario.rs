//! End to end on the Solana runtime: the Vesuvius lane under the Standing Offer and
//! Governance Addendum A (every decision by vote; nobody votes on their own contribution).
//! House + three founders (Ally 1, Ally 2, Ally 3). Weights: accepted contribution = 5 points,
//! GPU hour = 1 point. Bar 3. Unanimous below 6. 75/25.

use borsh::BorshDeserialize;
use proofshare::{
    member_pda, process_instruction, project_pda, proposal_pda, vote_pda, Instruction as Ix, Project,
    Proposal, KIND_ADD_MEMBER, KIND_CONTRIBUTION, KIND_EXPENSE, KIND_SETTLE, STATUS_FAILED, STATUS_PASSED,
    STATUS_PENDING,
};
use solana_program_test::{processor, BanksClient, ProgramTest};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    account::Account,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    native_token::LAMPORTS_PER_SOL as SOL,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};

const ID: &str = "vesuvius";

struct Ctx {
    bc: BanksClient,
    hash: Hash,
    pid: Pubkey,
    payer: Keypair,
    nonce: u64,
}

impl Ctx {
    async fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), String> {
        self.hash = self.bc.get_latest_blockhash().await.unwrap();
        // A unique compute-unit price per send: identical transactions would replay a cached result
        // instead of executing, which made "a second claim fails" checks flaky.
        self.nonce += 1;
        let mut ixs_all = vec![ComputeBudgetInstruction::set_compute_unit_price(self.nonce)];
        ixs_all.extend_from_slice(ixs);
        let ixs = &ixs_all[..];
        let mut all: Vec<&Keypair> = vec![&self.payer];
        all.extend_from_slice(signers);
        let tx = Transaction::new_signed_with_payer(ixs, Some(&self.payer.pubkey()), &all, self.hash);
        self.bc.process_transaction(tx).await.map_err(|e| e.to_string())
    }
    async fn balance(&mut self, k: &Pubkey) -> u64 {
        self.bc.get_balance(*k).await.unwrap()
    }
    async fn project(&mut self, k: &Pubkey) -> Project {
        let a = self.bc.get_account(*k).await.unwrap().unwrap();
        Project::deserialize(&mut &a.data[..]).unwrap()
    }
    async fn proposal(&mut self, k: &Pubkey) -> Proposal {
        let a = self.bc.get_account(*k).await.unwrap().unwrap();
        Proposal::deserialize(&mut &a.data[..]).unwrap()
    }
}

fn ix(pid: &Pubkey, data: Ix, metas: Vec<AccountMeta>) -> Instruction {
    Instruction::new_with_borsh(*pid, &data, metas)
}

#[allow(clippy::too_many_arguments)]
fn propose_ix(pid: &Pubkey, project: &Pubkey, who: &Pubkey, index: u32, kind: u8, target: Pubkey, amount: u64, contrib_kind: u8) -> Instruction {
    let mut metas = vec![
        AccountMeta::new(*who, true),
        AccountMeta::new_readonly(member_pda(pid, project, who).0, false),
        AccountMeta::new(*project, false),
        AccountMeta::new(proposal_pda(pid, project, index).0, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    if kind == KIND_CONTRIBUTION {
        metas.push(AccountMeta::new_readonly(member_pda(pid, project, &target).0, false));
    }
    ix(pid, Ix::Propose { kind, target, amount, contrib_kind, memo: [0; 32] }, metas)
}

fn execute_ix(pid: &Pubkey, project: &Pubkey, executor: &Pubkey, index: u32, extra: Vec<AccountMeta>) -> Instruction {
    let mut metas = vec![
        AccountMeta::new(*executor, true),
        AccountMeta::new(*project, false),
        AccountMeta::new(proposal_pda(pid, project, index).0, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    metas.extend(extra);
    ix(pid, Ix::Execute, metas)
}

fn vote_ix(pid: &Pubkey, project: &Pubkey, who: &Pubkey, index: u32, yes: bool) -> Instruction {
    let prop = proposal_pda(pid, project, index).0;
    ix(pid, Ix::Vote { yes }, vec![
        AccountMeta::new(*who, true),
        AccountMeta::new_readonly(member_pda(pid, project, who).0, false),
        AccountMeta::new_readonly(*project, false),
        AccountMeta::new(prop, false),
        AccountMeta::new(vote_pda(pid, &prop, who).0, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ])
}

fn deposit_ix(pid: &Pubkey, project: &Pubkey, who: &Pubkey, amount: u64, prize: bool) -> Instruction {
    ix(pid, Ix::Deposit { amount, prize }, vec![
        AccountMeta::new(*who, true),
        AccountMeta::new(*project, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ])
}

fn claim_ix(pid: &Pubkey, project: &Pubkey, wallet: &Pubkey) -> Instruction {
    ix(pid, Ix::Claim, vec![
        AccountMeta::new(*project, false),
        AccountMeta::new(member_pda(pid, project, wallet).0, false),
        AccountMeta::new(*wallet, false),
    ])
}

#[tokio::test]
async fn vesuvius_lane_end_to_end() {
    let pid = Pubkey::new_unique();
    let mut pt = ProgramTest::new("proofshare", pid, processor!(process_instruction));
    let house = Keypair::new();
    let organizer = Keypair::new(); // Thomas Lanham's personal wallet, not the house's
    let ally1 = Keypair::new();
    let ally2 = Keypair::new();
    let ally3 = Keypair::new();
    let outsider = Keypair::new();
    let sponsor = Keypair::new(); // stands in for the prize payout arriving
    let vendor = Pubkey::new_unique(); // a GPU rental
    for k in [&house, &ally1, &ally2, &ally3, &outsider, &sponsor] {
        pt.add_account(k.pubkey(), Account { lamports: 100 * SOL, ..Account::default() });
    }
    let (bc, payer, hash) = pt.start().await;
    let mut c = Ctx { bc, hash, pid, payer, nonce: 0 };
    let (project, _) = project_pda(&pid, &house.pubkey(), ID);
    let founders = vec![ally1.pubkey(), ally2.pubkey(), ally3.pubkey()];
    let roster: Vec<Pubkey> = std::iter::once(house.pubkey()).chain(founders.iter().copied()).collect();

    // 0 · open the project with the founding roster
    let mut metas = vec![
        AccountMeta::new(house.pubkey(), true),
        AccountMeta::new(project, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    metas.extend(roster.iter().map(|w| AccountMeta::new(member_pda(&pid, &project, w).0, false)));
    let create = ix(&pid, Ix::CreateProject {
        id: ID.into(),
        organizer: organizer.pubkey(),
        organizer_fee_bps: 100,
        weights: [5, 1, 0, 0, 0, 0, 0, 0],
        min_qualify: 3,
        majority_from: 6,
        equal_split_bps: 7_500,
        founders: founders.clone(),
    }, metas);
    c.send(&[create], &[&house]).await.unwrap();
    assert_eq!(c.project(&project).await.member_count, 4);

    // 5 · the house fronts operating money; only the house may
    c.send(&[deposit_ix(&pid, &project, &house.pubkey(), 2 * SOL, false)], &[&house]).await.unwrap();
    assert!(c.send(&[deposit_ix(&pid, &project, &ally1.pubkey(), SOL, false)], &[&ally1]).await.is_err());

    let mut next: u32 = 0;
    let members_meta = |w: &[Pubkey]| -> Vec<AccountMeta> {
        w.iter().map(|w| AccountMeta::new_readonly(member_pda(&pid, &project, w).0, false)).collect()
    };

    // 1-3 · an expense: unanimous among four
    c.send(&[propose_ix(&pid, &project, &ally1.pubkey(), next, KIND_EXPENSE, vendor, SOL, 0)], &[&ally1]).await.unwrap();
    let p0 = proposal_pda(&pid, &project, next).0;
    for k in [&house, &ally1, &ally2] {
        c.send(&[vote_ix(&pid, &project, &k.pubkey(), next, true)], &[k]).await.unwrap();
    }
    assert_eq!(c.proposal(&p0).await.status, STATUS_PENDING, "three of four is not unanimous");
    // a member cannot vote twice; an outsider cannot vote at all
    assert!(c.send(&[vote_ix(&pid, &project, &ally1.pubkey(), next, true)], &[&ally1]).await.is_err());
    assert!(c.send(&[vote_ix(&pid, &project, &outsider.pubkey(), next, true)], &[&outsider]).await.is_err());
    // executing before it passes fails, whoever tries
    let exec0 = execute_ix(&pid, &project, &ally3.pubkey(), next, vec![AccountMeta::new(vendor, false)]);
    assert!(c.send(&[exec0.clone()], &[&ally3]).await.is_err());
    c.send(&[vote_ix(&pid, &project, &ally3.pubkey(), next, true)], &[&ally3]).await.unwrap();
    assert_eq!(c.proposal(&p0).await.status, STATUS_PASSED);
    // once passed, any member may execute it: no single hand holds the go
    c.send(&[exec0], &[&ally3]).await.unwrap();
    assert_eq!(c.balance(&vendor).await, SOL);
    next += 1;

    // adding a member: one "no" kills it under unanimity
    c.send(&[propose_ix(&pid, &project, &ally2.pubkey(), next, KIND_ADD_MEMBER, outsider.pubkey(), 0, 0)], &[&ally2]).await.unwrap();
    let p1 = proposal_pda(&pid, &project, next).0;
    c.send(&[vote_ix(&pid, &project, &house.pubkey(), next, true)], &[&house]).await.unwrap();
    c.send(&[vote_ix(&pid, &project, &ally3.pubkey(), next, false)], &[&ally3]).await.unwrap();
    assert_eq!(c.proposal(&p1).await.status, STATUS_FAILED);
    next += 1;

    // 4 · the ledger, by vote, the contributor recused.
    // Ally 1: 3 accepted + 40 GPU h. Ally 2: 6 accepted. Ally 3: 15 GPU h, none accepted.
    let ledger: Vec<(&Keypair, u8, u64)> = vec![(&ally1, 0, 3), (&ally1, 1, 40), (&ally2, 0, 6), (&ally3, 1, 15)];
    for (who, kind, units) in ledger {
        c.send(&[propose_ix(&pid, &project, &house.pubkey(), next, KIND_CONTRIBUTION, who.pubkey(), units, kind)], &[&house]).await.unwrap();
        // the contributor may not vote on their own work
        assert!(c.send(&[vote_ix(&pid, &project, &who.pubkey(), next, true)], &[who]).await.is_err());
        for k in [&house, &ally1, &ally2, &ally3] {
            if k.pubkey() != who.pubkey() {
                c.send(&[vote_ix(&pid, &project, &k.pubkey(), next, true)], &[k]).await.unwrap();
            }
        }
        c.send(&[execute_ix(&pid, &project, &house.pubkey(), next, vec![AccountMeta::new(member_pda(&pid, &project, &who.pubkey()).0, false)])], &[&house]).await.unwrap();
        next += 1;
    }

    // the prize arrives: 50 SOL standing in for $50,000
    c.send(&[deposit_ix(&pid, &project, &sponsor.pubkey(), 50 * SOL, true)], &[&sponsor]).await.unwrap();

    // 6 · settlement is itself a vote, and refuses an incomplete roster
    c.send(&[propose_ix(&pid, &project, &ally2.pubkey(), next, KIND_SETTLE, Pubkey::default(), 0, 0)], &[&ally2]).await.unwrap();
    for k in [&house, &ally1, &ally2, &ally3] {
        c.send(&[vote_ix(&pid, &project, &k.pubkey(), next, true)], &[k]).await.unwrap();
    }
    assert!(c.send(&[execute_ix(&pid, &project, &ally1.pubkey(), next, members_meta(&roster[..3]))], &[&ally1]).await.is_err());
    c.send(&[execute_ix(&pid, &project, &ally1.pubkey(), next, members_meta(&roster))], &[&ally1]).await.unwrap();
    let s = c.project(&project).await;
    // the bar is 3 (Ally 2 reached it), so Ally 3 does not qualify: house, Ally 1, Ally 2 = 3
    assert_eq!(s.effective_min, 3);
    assert_eq!(s.qualifiers, 3);
    assert_eq!(s.organizer_fee, 500_000_000); // 1% of 50 SOL, off the top
    assert_eq!(s.equal_share, 12_375_000_000); // 75% of 49.5 SOL / 3
    assert_eq!(s.reimbursement, SOL); // the expense the house fronted
    assert_eq!(s.bounty_pool, 11_375_000_000); // 12.375 − 1
    assert_eq!(s.total_points, 100); // 55 + 30 + 15

    // 7 · claims, each to its own wallet, once
    let mut before = vec![];
    for w in &roster {
        before.push(c.balance(w).await);
    }
    for w in &roster {
        c.send(&[claim_ix(&pid, &project, w)], &[]).await.unwrap();
    }
    assert!(c.send(&[claim_ix(&pid, &project, &ally1.pubkey())], &[]).await.is_err(), "no second claim");
    let mut after = vec![];
    for w in &roster {
        after.push(c.balance(w).await);
    }
    assert_eq!(after[0] - before[0], 12_375_000_000 + SOL); // equal share + reimbursement, no points
    assert_eq!(after[1] - before[1], 12_375_000_000 + 6_256_250_000); // 55% of the bounty
    assert_eq!(after[2] - before[2], 12_375_000_000 + 3_412_500_000); // 30%
    assert_eq!(after[3] - before[3], 1_706_250_000); // bounty only: 15%, below the bar

    // the organizer's fee: only to the organizer's own wallet, once; the sweep waits for it
    let early_sweep = ix(&pid, Ix::Sweep, vec![AccountMeta::new(project, false), AccountMeta::new(house.pubkey(), false)]);
    assert!(c.send(&[early_sweep], &[]).await.is_err(), "sweep waits for the organizer's fee");
    let wrong_org = ix(&pid, Ix::ClaimOrganizer, vec![AccountMeta::new(project, false), AccountMeta::new(house.pubkey(), false)]);
    assert!(c.send(&[wrong_org], &[]).await.is_err(), "the house cannot take the organizer's fee");
    let o_before = c.balance(&organizer.pubkey()).await;
    let org = ix(&pid, Ix::ClaimOrganizer, vec![AccountMeta::new(project, false), AccountMeta::new(organizer.pubkey(), false)]);
    c.send(&[org.clone()], &[]).await.unwrap();
    assert_eq!(c.balance(&organizer.pubkey()).await - o_before, 500_000_000);
    assert!(c.send(&[org], &[]).await.is_err(), "paid once");

    // 8 · sweep returns the unspent operating money (1 SOL) to the house wallet, and only there
    let wrong = ix(&pid, Ix::Sweep, vec![AccountMeta::new(project, false), AccountMeta::new(ally1.pubkey(), false)]);
    assert!(c.send(&[wrong], &[]).await.is_err());
    let h_before = c.balance(&house.pubkey()).await;
    let sweep = ix(&pid, Ix::Sweep, vec![AccountMeta::new(project, false), AccountMeta::new(house.pubkey(), false)]);
    c.send(&[sweep], &[]).await.unwrap();
    assert!(c.balance(&house.pubkey()).await >= h_before + SOL);
}

#[tokio::test]
async fn bar_drops_to_one_when_nobody_reached_three() {
    let pid = Pubkey::new_unique();
    let mut pt = ProgramTest::new("proofshare", pid, processor!(process_instruction));
    let house = Keypair::new();
    let a = Keypair::new();
    let b = Keypair::new();
    for k in [&house, &a, &b] {
        pt.add_account(k.pubkey(), Account { lamports: 100 * SOL, ..Account::default() });
    }
    let (bc, payer, hash) = pt.start().await;
    let mut c = Ctx { bc, hash, pid, payer, nonce: 0 };
    let (project, _) = project_pda(&pid, &house.pubkey(), "quick");
    let roster = vec![house.pubkey(), a.pubkey(), b.pubkey()];
    let mut metas = vec![
        AccountMeta::new(house.pubkey(), true),
        AccountMeta::new(project, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    metas.extend(roster.iter().map(|w| AccountMeta::new(member_pda(&pid, &project, w).0, false)));
    c.send(&[ix(&pid, Ix::CreateProject {
        id: "quick".into(), organizer: house.pubkey(), organizer_fee_bps: 100, weights: [5, 1, 0, 0, 0, 0, 0, 0], min_qualify: 3, majority_from: 6,
        equal_split_bps: 7_500, founders: vec![a.pubkey(), b.pubkey()],
    }, metas)], &[&house]).await.unwrap();
    // a shows up once (house and b vote it through); b never does; then the win
    c.send(&[propose_ix(&pid, &project, &house.pubkey(), 0, KIND_CONTRIBUTION, a.pubkey(), 1, 0)], &[&house]).await.unwrap();
    for k in [&house, &b] {
        c.send(&[vote_ix(&pid, &project, &k.pubkey(), 0, true)], &[k]).await.unwrap();
    }
    c.send(&[execute_ix(&pid, &project, &b.pubkey(), 0, vec![AccountMeta::new(member_pda(&pid, &project, &a.pubkey()).0, false)])], &[&b]).await.unwrap();
    c.send(&[deposit_ix(&pid, &project, &house.pubkey(), 10 * SOL, true)], &[&house]).await.unwrap();
    c.send(&[propose_ix(&pid, &project, &a.pubkey(), 1, KIND_SETTLE, Pubkey::default(), 0, 0)], &[&a]).await.unwrap();
    for k in [&house, &a, &b] {
        c.send(&[vote_ix(&pid, &project, &k.pubkey(), 1, true)], &[k]).await.unwrap();
    }
    let metas: Vec<AccountMeta> = roster.iter().map(|w| AccountMeta::new_readonly(member_pda(&pid, &project, w).0, false)).collect();
    c.send(&[execute_ix(&pid, &project, &a.pubkey(), 1, metas)], &[&a]).await.unwrap();
    let s = c.project(&project).await;
    assert_eq!(s.effective_min, 1);
    assert_eq!(s.qualifiers, 2); // house + a; b contributed nothing
    assert_eq!(s.organizer_fee, 100_000_000); // 1% of 10 SOL
    assert_eq!(s.equal_share, 3_712_500_000); // 75% of 9.9 / 2
}
