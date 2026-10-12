// End-to-end adversarial test of the escrow program (v2.1: settle-all before payout).
// Local validator (default) or devnet: RPC_URL=https://api.devnet.solana.com FUNDER=<key.json> node test/escrow.test.mjs
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash } from '../lib/protocol.mjs';
import { ixCreate, ixSubmit, ixSubmitRawHash, ixReveal, ixSettle, ixPayout, loadContest, pdas, explainError } from '../lib/escrow.mjs';

const RPC = process.env.RPC_URL || 'http://127.0.0.1:8899';
const conn = new web3.Connection(RPC, 'confirmed');
const [house, alice, bob, carol, eve, dave] = Array.from({ length: 6 }, () => web3.Keypair.generate());
let pass = 0, fail = 0;
const ok = (c, m) => { if (c) { pass++; console.log('  ✓', m); } else { fail++; console.log('  ✗', m); } };
const send = (ixx, signer) => web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixx), [signer], { commitment: 'confirmed' });
async function expectFail(p, want, m) { try { await p; ok(false, m + ' (unexpectedly succeeded)'); } catch (e) { const why = explainError(e); ok(!want || why.includes(want), `${m} → rejected: ${why}`); } }
const waitSlot = async s => { while ((await conn.getSlot('confirmed')) <= s) await new Promise(r => setTimeout(r, 400)); };

if (process.env.FUNDER) {
  const fs = await import('node:fs');
  const funder = web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(process.env.FUNDER, 'utf8'))));
  const tx = new web3.Transaction();
  for (const k of [house, alice, bob, carol, eve, dave]) tx.add(web3.SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: k.publicKey, lamports: k === house ? 0.7e9 : 0.01e9 }));
  await web3.sendAndConfirmTransaction(conn, tx, [funder], { commitment: 'confirmed' });
} else for (const k of [house, alice, bob, carol, eve, dave]) {
  const sig = await conn.requestAirdrop(k.publicKey, 2e9); await conn.confirmTransaction(sig, 'confirmed');
}
const OPEN = Number(process.env.OPEN_SLOTS || 45);
const mk = async (ANSWER, purse) => {
  const id = 'T-' + crypto.randomBytes(3).toString('hex').toUpperCase();
  const salt = crypto.randomBytes(16).toString('hex');
  const closeSlot = (await conn.getSlot('confirmed')) + OPEN;
  await send(ixCreate(web3, { house: house.publicKey, id, commitHex: await commitHash(id, salt, ANSWER), closeSlot, claimSlots: 0, purseLamports: purse }), house);
  return { id, salt, closeSlot, ANSWER, contest: pdas(web3, house.publicKey, id).contest };
};
const H = house.publicKey;

console.log('A. contest with a winner');
const A = await mk('GRANDEXPO', 500_000_000);
const E = k => pdas(web3, H, A.id, k.publicKey).entry;
let st = await loadContest(web3, conn, H, A.id);
ok(st.exists && st.purse === 500_000_000, 'purse of 0.5 SOL held by the contest account');
await send(await ixSubmit(web3, { house: H, id: A.id, solver: alice.publicKey, guess: 'world fair' }), alice);
await send(await ixSubmit(web3, { house: H, id: A.id, solver: bob.publicKey, guess: 'Grand Expo' }), bob);
await send(await ixSubmit(web3, { house: H, id: A.id, solver: carol.publicKey, guess: 'grandexpo' }), carol);
await send(ixSubmitRawHash(web3, { house: H, id: A.id, solver: eve.publicKey, hashHex: await submitHash(A.id, bob.publicKey.toBase58(), 'GRANDEXPO') }), eve);
await expectFail(send(await ixSubmit(web3, { house: H, id: A.id, solver: alice.publicKey, guess: 'again' }), alice), 'already entered', 'second entry from the same wallet');
await expectFail(send(await ixSubmit(web3, { house: H, id: A.id, solver: H, guess: 'GRANDEXPO' }), house), 'house wallet cannot enter', 'house wallet enters its own contest');
await expectFail(send(ixReveal(web3, { house: H, id: A.id, salt: A.salt, answer: A.ANSWER }), house), 'Too early', 'reveal before entries close');
await waitSlot(A.closeSlot);
await expectFail(send(await ixSubmit(web3, { house: H, id: A.id, solver: dave.publicKey, guess: 'late' }), dave), 'closed', 'entry after close');
{ // a non-house wallet signs a reveal against the REAL contest account
  const real = ixReveal(web3, { house: H, id: A.id, salt: A.salt, answer: A.ANSWER });
  const forged = new web3.TransactionInstruction({ programId: real.programId, data: real.data,
    keys: [{ pubkey: alice.publicKey, isSigner: true, isWritable: false }, { pubkey: A.contest, isSigner: false, isWritable: true }] });
  await expectFail(send(forged, alice), 'Only the house', 'non-house wallet reveals on the real contest');
}
await expectFail(send(ixReveal(web3, { house: H, id: A.id, salt: A.salt, answer: 'SOMETHINGELSE' }), house), 'does not match', 'house tries to change the answer');
await send(ixReveal(web3, { house: H, id: A.id, salt: A.salt, answer: A.ANSWER }), house);
ok((await loadContest(web3, conn, H, A.id)).revealed, 'honest reveal accepted');

await expectFail(send(ixPayout(web3, { contest: A.contest, recipient: carol.publicKey }), carol), 'Not every entry', 'payout before every entry is settled');
await send(ixSettle(web3, { contest: A.contest, entry: E(carol) }), carol);
st = await loadContest(web3, conn, H, A.id);
ok(st.leader === carol.publicKey.toBase58(), 'carol (correct, later) settles first and leads');
await expectFail(send(ixSettle(web3, { contest: A.contest, entry: E(carol) }), carol), 'already settled', 'carol settles twice');
await send(ixSettle(web3, { contest: A.contest, entry: E(eve) }), eve);
await send(ixSettle(web3, { contest: A.contest, entry: E(alice) }), alice);
st = await loadContest(web3, conn, H, A.id);
ok(st.leader === carol.publicKey.toBase58() && st.settled === 3, "eve's copied hash and alice's wrong guess settle as wrong");
await expectFail(send(ixPayout(web3, { contest: A.contest, recipient: carol.publicKey }), carol), 'Not every entry', 'carol tries to cash out while an earlier entry is unsettled');
await send(ixSettle(web3, { contest: A.contest, entry: E(bob) }), alice); // anyone may settle anyone
st = await loadContest(web3, conn, H, A.id);
ok(st.leader === bob.publicKey.toBase58(), 'bob (correct, earliest) takes the lead when settled by a third party');
await expectFail(send(ixPayout(web3, { contest: A.contest, recipient: carol.publicKey }), carol), 'not the winner', 'payout to a later correct wallet');
await expectFail(send(ixPayout(web3, { contest: A.contest, recipient: H }), house), 'not the winner', 'payout to the house while a correct entry exists');
const before = await conn.getBalance(bob.publicKey, 'confirmed');
await send(ixPayout(web3, { contest: A.contest, recipient: bob.publicKey }), dave); // anyone can trigger
ok((await conn.getBalance(bob.publicKey, 'confirmed')) - before === 500_000_000, 'bob receives exactly the 0.5 SOL purse');
await expectFail(send(ixPayout(web3, { contest: A.contest, recipient: bob.publicKey }), house), 'Already paid', 'double payout');
await expectFail(send(ixSettle(web3, { contest: A.contest, entry: E(alice) }), alice), 'already settled', 'settling after payout');

console.log('B. contest where everyone is wrong');
const B = await mk('PERISPHERE', 50_000_000);
const EB = k => pdas(web3, H, B.id, k.publicKey).entry;
await send(await ixSubmit(web3, { house: H, id: B.id, solver: alice.publicKey, guess: 'trylon' }), alice);
await waitSlot(B.closeSlot);
await send(ixReveal(web3, { house: H, id: B.id, salt: B.salt, answer: B.ANSWER }), house);
await expectFail(send(ixPayout(web3, { contest: B.contest, recipient: H }), house), 'Not every entry', 'refund to the house before every entry is proven wrong');
await send(ixSettle(web3, { contest: B.contest, entry: EB(alice) }), alice);
const hb = await conn.getBalance(H, 'confirmed');
await send(ixPayout(web3, { contest: B.contest, recipient: H }), alice);
ok((await conn.getBalance(H, 'confirmed')) - hb === 50_000_000, 'refund to the house only after every entry is proven wrong');

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
