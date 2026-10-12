// ProofDrop v2 client: instruction builders and decoders for the escrow program.
// Works in Node and the browser. Pass in the web3 module so both builds share one copy.
import { submitHash, normalizeAnswer } from './protocol.mjs';

export const PROGRAM_ID = '5en4PkG2UVAGsW3JfpZdfV6t7PkUh8CLpNVg37ua7T1q';
export const CONTEST_LEN = 256;
export const ENTRY_LEN = 112;
const NONE = 18446744073709551615n;

export const ERRORS = {
  1: 'Bad instruction', 2: 'Wrong account', 3: 'Entries are closed', 4: 'Too early: entries are still open',
  5: 'Already revealed', 6: 'Not revealed yet', 7: 'Reveal does not match the commitment',
  8: 'That entry is not correct', 9: 'An earlier correct entry already leads', 10: 'Claim window still open',
  11: 'Claim window has closed', 12: 'Already paid', 13: 'Recipient is not the winner', 14: 'Only the house can do that',
};
export function explainError(e) {
  const s0 = String(e?.message || e); if (/already in use|custom program error: 0x0\b/i.test(s0)) return 'This wallet has already entered this contest';
  const m = s0.match(/custom program error: 0x([0-9a-f]+)/i);
  if (m) return ERRORS[parseInt(m[1], 16)] || `Program error ${parseInt(m[1], 16)}`;
  if (/already in use/.test(String(e?.message || e)) || /0x0\b/.test(String(e?.message || e))) return 'This wallet has already entered this contest';
  return String(e?.message || e);
}

const enc = s => new TextEncoder().encode(s);
const u64 = n => { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(n), true); return b; };
const cat = (...a) => { const out = new Uint8Array(a.reduce((n, x) => n + x.length, 0)); let o = 0; for (const x of a) { out.set(x, o); o += x.length; } return out; };
const hex2bytes = h => Uint8Array.from(h.match(/../g).map(x => parseInt(x, 16)));
const toBuf = (web3, u8) => (typeof Buffer !== 'undefined' ? Buffer.from(u8) : web3.Buffer ? web3.Buffer.from(u8) : u8);

export function pdas(web3, house, id, solver) {
  const P = new web3.PublicKey(PROGRAM_ID);
  const [contest] = web3.PublicKey.findProgramAddressSync([enc('contest'), new web3.PublicKey(house).toBytes(), enc(id)], P);
  const out = { program: P, contest };
  if (solver) [out.entry] = web3.PublicKey.findProgramAddressSync([enc('entry'), contest.toBytes(), new web3.PublicKey(solver).toBytes()], P);
  return out;
}

const ix = (web3, keys, data) => new web3.TransactionInstruction({ programId: new web3.PublicKey(PROGRAM_ID), keys, data: toBuf(web3, data) });
const acc = (pubkey, isSigner, isWritable) => ({ pubkey, isSigner, isWritable });

export function ixCreate(web3, { house, id, commitHex, closeSlot, claimSlots, purseLamports }) {
  const { contest } = pdas(web3, house, id);
  const idb = enc(id);
  const data = cat([0, idb.length], idb, hex2bytes(commitHex), u64(closeSlot), u64(claimSlots), u64(purseLamports));
  return ix(web3, [acc(new web3.PublicKey(house), true, true), acc(contest, false, true), acc(web3.SystemProgram.programId, false, false)], data);
}

export async function ixSubmit(web3, { house, id, solver, guess }) {
  const { contest, entry } = pdas(web3, house, id, solver);
  const h = await submitHash(id, new web3.PublicKey(solver).toBase58(), guess);
  return ix(web3, [acc(new web3.PublicKey(solver), true, true), acc(contest, false, true), acc(entry, false, true), acc(web3.SystemProgram.programId, false, false)], cat([1], hex2bytes(h)));
}

export function ixSubmitRawHash(web3, { house, id, solver, hashHex }) {
  const { contest, entry } = pdas(web3, house, id, solver);
  return ix(web3, [acc(new web3.PublicKey(solver), true, true), acc(contest, false, true), acc(entry, false, true), acc(web3.SystemProgram.programId, false, false)], cat([1], hex2bytes(hashHex)));
}

export function ixReveal(web3, { house, id, salt, answer }) {
  const { contest } = pdas(web3, house, id);
  const s = enc(salt), a = enc(normalizeAnswer(answer));
  return ix(web3, [acc(new web3.PublicKey(house), true, false), acc(contest, false, true)], cat([2, s.length], s, [a.length], a));
}

export function ixClaim(web3, { contest, entry }) {
  return ix(web3, [acc(new web3.PublicKey(contest), false, true), acc(new web3.PublicKey(entry), false, false)], new Uint8Array([3]));
}

export function ixPayout(web3, { contest, recipient }) {
  return ix(web3, [acc(new web3.PublicKey(contest), false, true), acc(new web3.PublicKey(recipient), false, true)], new Uint8Array([4]));
}

const dv = d => new DataView(d.buffer, d.byteOffset, d.byteLength);
export function decodeContest(web3, data) {
  const d = Uint8Array.from(data), v = dv(d);
  const pk = o => new web3.PublicKey(d.slice(o, o + 32)).toBase58();
  const idLen = d[34], ansLen = d[116], bestSlot = v.getBigUint64(181, true);
  return {
    house: pk(2), id: new TextDecoder().decode(d.slice(35, 35 + idLen)),
    commitHex: [...d.slice(51, 83)].map(b => b.toString(16).padStart(2, '0')).join(''),
    closeSlot: Number(v.getBigUint64(83, true)), claimSlots: Number(v.getBigUint64(91, true)),
    purse: Number(v.getBigUint64(99, true)), revealed: d[107] === 1, revealSlot: Number(v.getBigUint64(108, true)),
    answer: ansLen ? new TextDecoder().decode(d.slice(117, 117 + ansLen)) : null,
    leader: bestSlot === NONE ? null : pk(149), leaderSlot: bestSlot === NONE ? null : Number(bestSlot),
    entries: v.getUint32(189, true), paid: d[193],
  };
}
export function decodeEntry(web3, data) {
  const d = Uint8Array.from(data), v = dv(d);
  return {
    contest: new web3.PublicKey(d.slice(2, 34)).toBase58(), solver: new web3.PublicKey(d.slice(34, 66)).toBase58(),
    hashHex: [...d.slice(66, 98)].map(b => b.toString(16).padStart(2, '0')).join(''),
    slot: Number(v.getBigUint64(98, true)), idx: v.getUint32(106, true),
  };
}

/** Load a contest and all its entries straight from chain. */
export async function loadContest(web3, conn, house, id) {
  const { contest } = pdas(web3, house, id);
  const info = await conn.getAccountInfo(contest, 'confirmed');
  if (!info) return { address: contest.toBase58(), exists: false };
  const c = decodeContest(web3, info.data);
  const raw = await conn.getProgramAccounts(new web3.PublicKey(PROGRAM_ID), {
    commitment: 'confirmed', filters: [{ dataSize: ENTRY_LEN }, { memcmp: { offset: 2, bytes: contest.toBase58() } }],
  });
  const entries = raw.map(r => ({ address: r.pubkey.toBase58(), ...decodeEntry(web3, r.account.data) })).sort((a, b) => a.slot - b.slot || a.idx - b.idx);
  if (c.revealed) for (const e of entries) e.correct = (await submitHash(c.id, e.solver, c.answer)) === e.hashHex;
  return { address: contest.toBase58(), exists: true, lamports: info.lamports, ...c, entries };
}
