//! The Scrypt Collaborator Standing Offer, as arithmetic.
//!
//! Ruled by Salvador, 27 Oct 2026. Every rule the program enforces lives here as a
//! pure function, so it can be tested on any machine without a validator.
//!
//! 0. First, the organizer's fee (1%, ruled 27 Oct 2026) comes off the top of every prize and
//!    every income split, paid to the organizer personally, not to the house.
//! 1. Of what remains, 75% is split EQUALLY among qualifying contributors. The house
//!    (Scrypt LLC) always counts as one qualifier.
//! 2. 25% is the Reserve. It first repays approved project expenses the house fronted.
//!    What remains is the Bounty Pool, paid pro rata by contribution points.
//!    If expenses exceed the Reserve, the shortfall comes off the top of the Equal Split.
//! 3. A collaborator qualifies with at least `min_qualify` (3) accepted contributions.
//!    If no collaborator has reached that bar when the prize is settled, the bar drops to 1.
//! 4. Each project defines its own contribution kinds and weights when it is opened
//!    (ruled 27 Oct: terms are redefined per project to its documented bounty work).
//!    Kind 0 is always "accepted contribution" and is what qualification counts.
//!    Points = sum over kinds of units[k] * weight[k].
//! 5. Adding a member or spending money takes a vote of the members:
//!    unanimous while there are fewer than `majority_from` (6) members;
//!    at six or more, a simple majority, with ties broken by the house's own vote.
//!    If the house cannot break a tie (it was recused or abstained), the proposal fails.

/// Basis points: 10_000 = 100%.
pub const BPS: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Passed,
    Failed,
}

/// Tally a vote.
///
/// * `electorate` — members eligible to vote on this proposal (snapshot at proposal time).
/// * `yes`, `no` — votes cast so far.
/// * `house_vote` — `Some(true|false)` once the house has voted, used only to break a tie.
/// * `majority_from` — member count at which unanimity gives way to a simple majority.
pub fn tally(
    electorate: u32,
    yes: u32,
    no: u32,
    house_vote: Option<bool>,
    majority_from: u32,
) -> Outcome {
    if electorate == 0 || yes + no > electorate {
        return Outcome::Failed;
    }
    if electorate < majority_from {
        // Unanimous: one "no" kills it; every member must say yes.
        if no > 0 {
            return Outcome::Failed;
        }
        if yes == electorate {
            return Outcome::Passed;
        }
        return Outcome::Pending;
    }
    // Simple majority of the whole electorate.
    if yes * 2 > electorate {
        return Outcome::Passed;
    }
    if no * 2 > electorate {
        return Outcome::Failed;
    }
    // Exact tie, only possible with an even electorate once everyone has voted.
    if yes + no == electorate && yes == no {
        return match house_vote {
            Some(true) => Outcome::Passed,
            Some(false) => Outcome::Failed,
            None => Outcome::Failed, // the house was recused or did not vote: a tie fails
        };
    }
    Outcome::Pending
}

/// The qualifying bar in force at settlement.
/// `max_collab_accepted` is the highest accepted-contribution count among collaborators
/// (the house excluded).
pub fn effective_min(min_qualify: u32, max_collab_accepted: u32) -> u32 {
    if max_collab_accepted >= min_qualify {
        min_qualify
    } else {
        1
    }
}

pub fn qualifies(is_house: bool, accepted: u32, effective_min: u32) -> bool {
    is_house || accepted >= effective_min
}

/// Maximum contribution kinds a project may define.
pub const MAX_KINDS: usize = 8;

/// Points for one member: sum of units logged per kind times that kind's weight.
pub fn points(units: &[u64; MAX_KINDS], weights: &[u64; MAX_KINDS]) -> u64 {
    units
        .iter()
        .zip(weights.iter())
        .fold(0u64, |acc, (u, w)| acc.saturating_add(u.saturating_mul(*w)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// Off the top, to the organizer personally.
    pub organizer_fee: u64,
    /// Paid to each qualifier (house included).
    pub equal_share: u64,
    /// Repaid to the house for expenses it fronted.
    pub reimbursement: u64,
    /// Shared pro rata by points.
    pub bounty_pool: u64,
}

/// Split a prize.
///
/// * `prize` — the prize amount being settled (lamports or token base units).
/// * `organizer_fee_bps` — 100 (1%) under the standing offer; taken off the top first.
/// * `expenses_paid` — approved expenses already paid out of the treasury, fronted by the house.
/// * `qualifiers` — number of qualifying contributors, house included (always ≥ 1).
/// * `equal_split_bps` — 7_500 under the standing offer.
pub fn settle(prize: u64, organizer_fee_bps: u64, expenses_paid: u64, qualifiers: u32, equal_split_bps: u64) -> Settlement {
    let qualifiers = qualifiers.max(1) as u64;
    let organizer_fee = mul_bps(prize, organizer_fee_bps);
    let prize = prize - organizer_fee;
    let equal_total = mul_bps(prize, equal_split_bps);
    let reserve = prize - equal_total;
    let (reimbursement, bounty_pool, equal_after) = if expenses_paid <= reserve {
        (expenses_paid, reserve - expenses_paid, equal_total)
    } else {
        let shortfall = (expenses_paid - reserve).min(equal_total);
        (reserve + shortfall, 0, equal_total - shortfall)
    };
    Settlement {
        organizer_fee,
        equal_share: equal_after / qualifiers,
        reimbursement,
        bounty_pool,
    }
}

/// One member's bounty, by points. Rounds down; dust returns to the house at sweep.
pub fn bounty_share(bounty_pool: u64, member_points: u64, total_points: u64) -> u64 {
    if total_points == 0 {
        return 0;
    }
    ((bounty_pool as u128 * member_points as u128) / total_points as u128) as u64
}

fn mul_bps(amount: u64, bps: u64) -> u64 {
    ((amount as u128 * bps as u128) / BPS as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: u32 = 6;

    #[test]
    fn unanimous_below_six() {
        // four members: everyone must say yes
        assert_eq!(tally(4, 3, 0, Some(true), M), Outcome::Pending);
        assert_eq!(tally(4, 4, 0, Some(true), M), Outcome::Passed);
        assert_eq!(tally(4, 3, 1, Some(true), M), Outcome::Failed);
        // two members, one says no
        assert_eq!(tally(2, 1, 1, Some(true), M), Outcome::Failed);
        // the house alone
        assert_eq!(tally(1, 1, 0, Some(true), M), Outcome::Passed);
        // five is still unanimous
        assert_eq!(tally(5, 4, 1, Some(true), M), Outcome::Failed);
    }

    #[test]
    fn majority_from_six() {
        assert_eq!(tally(6, 4, 0, None, M), Outcome::Passed);
        assert_eq!(tally(6, 3, 0, Some(true), M), Outcome::Pending);
        assert_eq!(tally(6, 0, 4, None, M), Outcome::Failed);
        assert_eq!(tally(7, 4, 3, Some(false), M), Outcome::Passed);
        assert_eq!(tally(10, 6, 2, Some(false), M), Outcome::Passed);
    }

    #[test]
    fn house_breaks_the_tie() {
        assert_eq!(tally(6, 3, 3, Some(true), M), Outcome::Passed);
        assert_eq!(tally(6, 3, 3, Some(false), M), Outcome::Failed);
        assert_eq!(tally(10, 5, 5, Some(true), M), Outcome::Passed);
        // the house recused (its own contribution): a tie fails
        assert_eq!(tally(6, 3, 3, None, M), Outcome::Failed);
    }

    #[test]
    fn overcount_is_rejected() {
        assert_eq!(tally(4, 4, 1, Some(true), M), Outcome::Failed);
        assert_eq!(tally(0, 0, 0, None, M), Outcome::Failed);
    }

    #[test]
    fn qualifying_bar_and_its_exception() {
        // someone reached three: the bar is three
        assert_eq!(effective_min(3, 5), 3);
        assert!(!qualifies(false, 2, effective_min(3, 5)));
        assert!(qualifies(false, 3, effective_min(3, 5)));
        // nobody reached three before the win: the bar drops to one
        assert_eq!(effective_min(3, 1), 1);
        assert!(qualifies(false, 1, effective_min(3, 1)));
        assert!(!qualifies(false, 0, effective_min(3, 1)));
        // the house always qualifies
        assert!(qualifies(true, 0, 3));
    }

    #[test]
    fn the_worked_example() {
        // $50,000 prize, $2,000 approved expenses, 4 qualifiers (house + 3)
        let s = settle(50_000, 0, 2_000, 4, 7_500);
        assert_eq!(s.equal_share, 9_375);
        assert_eq!(s.reimbursement, 2_000);
        assert_eq!(s.bounty_pool, 10_500);
        // 2 qualifiers
        assert_eq!(settle(50_000, 0, 2_000, 2, 7_500).equal_share, 18_750);
        // 10 qualifiers
        assert_eq!(settle(50_000, 0, 2_000, 10, 7_500).equal_share, 3_750);
    }

    #[test]
    fn organizer_fee_comes_off_the_top() {
        // $50,000 (in cents), 1% organizer's fee, $2,000 expenses, 4 qualifiers
        let s = settle(50_000_00, 100, 2_000_00, 4, 7_500);
        assert_eq!(s.organizer_fee, 500_00);
        assert_eq!(s.equal_share, 9_281_25); // 75% of 49,500 / 4
        assert_eq!(s.reimbursement, 2_000_00);
        assert_eq!(s.bounty_pool, 10_375_00); // 25% of 49,500 - 2,000
        assert!(s.organizer_fee + s.equal_share * 4 + s.reimbursement + s.bounty_pool <= 50_000_00);
        // with the fee in place, nothing is ever overpaid
        for &(prize, exp, q) in &[(999_999u64, 123u64, 7u32), (10, 0, 3), (50_000, 60_000, 2)] {
            let s = settle(prize, 100, exp, q, 7_500);
            assert!(s.organizer_fee + s.equal_share * q as u64 + s.reimbursement + s.bounty_pool <= prize);
        }
    }

    #[test]
    fn expenses_beyond_the_reserve_come_off_the_top() {
        // reserve 12_500, expenses 15_000 -> shortfall 2_500 off the 37_500
        let s = settle(50_000, 0, 15_000, 4, 7_500);
        assert_eq!(s.reimbursement, 15_000);
        assert_eq!(s.bounty_pool, 0);
        assert_eq!(s.equal_share, (37_500 - 2_500) / 4);
    }

    #[test]
    fn nothing_is_created_from_nothing() {
        for &(prize, exp, q) in &[(50_000u64, 2_000u64, 4u32), (999_999, 123, 7), (10, 0, 3), (50_000, 60_000, 2)] {
            let s = settle(prize, 0, exp, q, 7_500);
            let paid = s.organizer_fee + s.equal_share * q as u64 + s.reimbursement + s.bounty_pool;
            assert!(paid <= prize, "overpaid: {:?}", s);
        }
    }

    #[test]
    fn bounty_by_points() {
        // Vesuvius: kind 0 accepted contribution = 5 points, kind 1 GPU hour = 1 point
        let w = [5, 1, 0, 0, 0, 0, 0, 0];
        let a = points(&[3, 40, 0, 0, 0, 0, 0, 0], &w); // 55
        let b = points(&[6, 0, 0, 0, 0, 0, 0, 0], &w); // 30
        let c = points(&[0, 15, 0, 0, 0, 0, 0, 0], &w); // 15
        let total = a + b + c; // 100
        assert_eq!(bounty_share(10_500, a, total), 5_775);
        assert_eq!(bounty_share(10_500, b, total), 3_150);
        assert_eq!(bounty_share(10_500, c, total), 1_575);
        assert_eq!(bounty_share(10_500, 0, 0), 0);
    }
}
