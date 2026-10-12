// End-to-end adversarial test of the escrow program.
// Run against a local validator (default) or devnet: RPC_URL=... node test/escrow.test.mjs
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash } from '../lib/protocol.mjs';
import { ixCreate, ixSubmit, ixSubmitRawHash, ixReveal, ixClaim, ixPayout, loadContest, pdas, explainError } from '../lib/escrow.mjs';

const RPC = process.env.RPC_URL || 'http://127.0.0.1:8899';
const conn = new web3.Connection(RPC, 'confirmed');
const [house, alice, bob, carol, eve] = Array.from({ length: 5 }, () => web3.Keypair.generate());
const name = new Map([[house, 'house'], [alice, 'alice'], [bob, 'bob'], [carol, 'carol'], [eve, 'eve']]);
let pass = 0, fail = 0;
const ok = (c, m) => { if (c) { pass++; console.log('  ✓', m); } else { fail++; console.log('  ✗', m); } };
const send = (ixx, signer) => web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixx), [signer], { commitment: 'confirmed' });
async function expectFail(p, want, m) { try { await p; ok(false, m + ' (unexpectedly succeeded)'); } catch (e) { const why = explainError(e); ok(!want || why.includes(want), `${m} → rejected: ${why}`); } }
const waitSlot = async s => { while ((await conn.getSlot('confirmed')) <= s) await new Promise(r => setTimeout(r, 400)); };

if (process.env.FUNDER) { // devnet: fund test wallets from a funded key instead of the rate-limited airdrop
  const fs = await import('node:fs');
  const funder = web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(process.env.FUNDER, 'utf8'))));
  const tx = new web3.Transaction();
  for (const k of [house, alice, bob, carol, eve]) tx.add(web3.SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: k.publicKey, lamports: k === house ? 0.6e9 : 0.01e9 }));
  await web3.sendAndConfirmTransaction(conn, tx, [funder], { commitment: 'confirmed' });
} else for (const k of [house, alice, bob, carol, eve]) {
  const sig = await conn.requestAirdrop(k.publicKey, 2e9); await conn.confirmTransaction(sig, 'confirmed');
}
const id = 'T-' + crypto.randomBytes(3).toString('hex').toUpperCase();
const ANSWER = 'GRANDEXPO', salt = crypto.randomBytes(16).toString('hex');
const commitHex = await commitHash(id, salt, ANSWER);
const now = await conn.getSlot('confirmed');
const closeSlot = now + 60, claimSlots = 25, purse = 500_000_000;
const { contest } = pdas(web3, house.publicKey, id);
console.log(`contest ${id} at ${contest.toBase58()}, close slot ${closeSlot}`);

console.log('1. create with escrow');
await send(ixCreate(web3, { house: house.publicKey, id, commitHex, closeSlot, claimSlots, purseLamports: purse }), house);
let st = await loadContest(web3, conn, house.publicKey, id);
ok(st.exists && st.purse === purse && st.lamports >= purse, `purse of ${purse / 1e9} SOL held by the contest account`);

console.log('2. entries');
await send(await ixSubmit(web3, { house: house.publicKey, id, solver: alice.publicKey, guess: 'world fair' }), alice);
await send(await ixSubmit(web3, { house: house.publicKey, id, solver: bob.publicKey, guess: 'Grand Expo' }), bob);
await send(await ixSubmit(web3, { house: house.publicKey, id, solver: carol.publicKey, guess: 'grandexpo' }), carol);
const bobHash = await submitHash(id, bob.publicKey.toBase58(), ANSWER);
await send(ixSubmitRawHash(web3, { house: house.publicKey, id, solver: eve.publicKey, hashHex: bobHash }), eve); // copy attack
await expectFail(send(await ixSubmit(web3, { house: house.publicKey, id, solver: alice.publicKey, guess: 'again' }), alice), '', 'second entry from the same wallet');
await expectFail(send(ixReveal(web3, { house: house.publicKey, id, salt, answer: ANSWER }), house), 'Too early', 'reveal before entries close');
st = await loadContest(web3, conn, house.publicKey, id);
ok(st.entries.length === 4, `4 sealed entries recorded (${st.entries.map(e => e.slot).join(', ')})`);

console.log('3. close, then reveal');
await waitSlot(closeSlot);
await expectFail(send(await ixSubmit(web3, { house: house.publicKey, id, solver: house.publicKey, guess: 'late' }), house), 'closed', 'entry after close');
await expectFail(send(ixReveal(web3, { house: alice.publicKey, id, salt, answer: ANSWER }), alice), '', 'reveal by a non-house wallet');
await expectFail(send(ixReveal(web3, { house: house.publicKey, id, salt, answer: 'SOMETHINGELSE' }), house), 'does not match', 'house tries to change the answer');
await send(ixReveal(web3, { house: house.publicKey, id, salt, answer: ANSWER }), house);
st = await loadContest(web3, conn, house.publicKey, id);
ok(st.revealed && st.answer === ANSWER, 'honest reveal accepted on chain');

console.log('4. claims');
const E = k => pdas(web3, house.publicKey, id, k.publicKey).entry;
await expectFail(send(ixClaim(web3, { contest, entry: E(alice) }), alice), 'not correct', 'claim for a wrong answer');
await expectFail(send(ixClaim(web3, { contest, entry: E(eve) }), eve), 'not correct', "copied hash (eve reposted bob's)");
await send(ixClaim(web3, { contest, entry: E(carol) }), carol);
st = await loadContest(web3, conn, house.publicKey, id);
ok(st.leader === carol.publicKey.toBase58(), 'carol (correct, later) leads after her claim');
await send(ixClaim(web3, { contest, entry: E(bob) }), alice); // anyone can claim on bob's behalf
st = await loadContest(web3, conn, house.publicKey, id);
ok(st.leader === bob.publicKey.toBase58(), 'bob (correct, earliest) takes the lead, claimed by a third party');
await expectFail(send(ixClaim(web3, { contest, entry: E(carol) }), carol), 'earlier', 'carol re-claims');
await expectFail(send(ixPayout(web3, { contest, recipient: bob.publicKey }), bob), 'still open', 'payout during the claim window');

console.log('5. payout');
await waitSlot(st.revealSlot + claimSlots);
await expectFail(send(ixPayout(web3, { contest, recipient: carol.publicKey }), carol), 'not the winner', 'payout to the wrong wallet');
const before = await conn.getBalance(bob.publicKey, 'confirmed');
await send(ixPayout(web3, { contest, recipient: bob.publicKey }), house); // anyone can trigger
const after = await conn.getBalance(bob.publicKey, 'confirmed');
ok(after - before === purse, `bob received exactly ${purse / 1e9} SOL from escrow`);
await expectFail(send(ixPayout(web3, { contest, recipient: bob.publicKey }), house), 'Already paid', 'double payout');

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
