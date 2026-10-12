// ProofDrop protocol v1 — provably fair puzzle contests on Solana.
// Shared by the house scripts (Node) and the browser app.
//
// Every contest has an INBOX address. All protocol transactions touch the inbox
// (a 0-lamport transfer), so `getSignaturesForAddress(inbox)` is the complete,
// ordered public record of the contest. Each transaction carries one SPL Memo:
//
//   COMMIT  house  ProofDrop:v1:COMMIT:<contestId>:<sha256(contestId|salt|ANSWER)>
//   SUBMIT  solver ProofDrop:v1:SUBMIT:<contestId>:<sha256(contestId|solverPubkey|ANSWER)>
//   REVEAL  house  ProofDrop:v1:REVEAL:<contestId>:<salt>:<ANSWER>
//
// Why it is fair:
//  - The house is bound to its answer before the clue is released (COMMIT is on-chain
//    first; changing the answer later breaks the hash).
//  - A solver's guess is hidden until the reveal, and is bound to the solver's own
//    public key, so nobody can copy a correct submission from the chain.
//  - The winner is the earliest valid SUBMIT (by slot, then by position in the inbox
//    history) that lands before the REVEAL. Anyone can recompute it from public data.

export const PREFIX = 'ProofDrop:v1';
export const MEMO_PROGRAM_ID = 'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr';

export function normalizeAnswer(s) {
  return String(s).normalize('NFKD').toUpperCase().replace(/[^A-Z0-9]/g, '');
}

export async function sha256Hex(text) {
  const data = new TextEncoder().encode(text);
  const buf = await globalThis.crypto.subtle.digest('SHA-256', data);
  return [...new Uint8Array(buf)].map(b => b.toString(16).padStart(2, '0')).join('');
}

export const commitHash = (id, salt, answer) => sha256Hex(`${id}|${salt}|${normalizeAnswer(answer)}`);
export const submitHash = (id, solver, answer) => sha256Hex(`${id}|${solver}|${normalizeAnswer(answer)}`);

export const memoCommit = (id, h) => `${PREFIX}:COMMIT:${id}:${h}`;
export const memoSubmit = (id, h) => `${PREFIX}:SUBMIT:${id}:${h}`;
export const memoReveal = (id, salt, answer) => `${PREFIX}:REVEAL:${id}:${salt}:${normalizeAnswer(answer)}`;

export function parseMemo(memo) {
  // RPC memos may arrive as "[len] text"; strip any bracketed length prefix.
  const m = String(memo || '').replace(/^\[\d+\]\s*/, '');
  if (!m.startsWith(PREFIX + ':')) return null;
  const p = m.split(':');
  // p: ProofDrop, v1, KIND, id, ...
  const kind = p[2], id = p[3];
  if (kind === 'COMMIT' || kind === 'SUBMIT') return { kind, id, hash: p[4] };
  if (kind === 'REVEAL') return { kind, id, salt: p[4], answer: p[5] };
  return null;
}

/**
 * Adjudicate a contest from its ordered public record.
 * records: [{ sig, slot, blockTime, signer, memo, idx }] in chain order (oldest first).
 * houseKey: base58 house public key.
 */
export async function adjudicate(contestId, houseKey, records) {
  const out = { commit: null, reveal: null, submissions: [], winner: null, valid: false, notes: [] };
  for (const r of records) {
    const pm = parseMemo(r.memo);
    if (!pm || pm.id !== contestId) continue;
    if (pm.kind === 'COMMIT') {
      if (r.signer !== houseKey) { out.notes.push(`Ignored COMMIT not signed by the house: ${r.sig}`); continue; }
      if (!out.commit) out.commit = { ...r, hash: pm.hash };
    } else if (pm.kind === 'REVEAL') {
      if (r.signer !== houseKey) { out.notes.push(`Ignored REVEAL not signed by the house: ${r.sig}`); continue; }
      if (!out.reveal) out.reveal = { ...r, salt: pm.salt, answer: pm.answer };
    } else if (pm.kind === 'SUBMIT') {
      out.submissions.push({ ...r, hash: pm.hash });
    }
  }
  if (!out.commit) { out.notes.push('No house COMMIT found.'); return out; }
  for (const s of out.submissions) {
    s.beforeCommit = s.slot < out.commit.slot || (s.slot === out.commit.slot && s.idx < out.commit.idx);
    s.afterReveal = out.reveal && (s.slot > out.reveal.slot || (s.slot === out.reveal.slot && s.idx > out.reveal.idx));
  }
  if (!out.reveal) { out.notes.push('Contest open: answer not yet revealed. Submissions are sealed.'); return out; }
  const expect = await commitHash(contestId, out.reveal.salt, out.reveal.answer);
  out.valid = expect === out.commit.hash;
  if (!out.valid) { out.notes.push('REVEAL does not match COMMIT. The house broke its promise; no winner can be declared.'); return out; }
  for (const s of out.submissions) {
    s.correct = (await submitHash(contestId, s.signer, out.reveal.answer)) === s.hash;
    s.eligible = s.correct && !s.afterReveal && !s.beforeCommit;
  }
  const elig = out.submissions.filter(s => s.eligible);
  elig.sort((a, b) => a.slot - b.slot || a.idx - b.idx);
  out.winner = elig[0] || null;
  return out;
}
