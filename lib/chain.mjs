// Chain helpers shared by Node scripts and the browser. Pass in the web3 module.
import { MEMO_PROGRAM_ID } from './protocol.mjs';

export function buildProtocolTx(web3, { payer, inbox, memo }) {
  const { Transaction, SystemProgram, TransactionInstruction, PublicKey } = web3;
  const tx = new Transaction();
  // 0-lamport touch so the inbox's signature history is the contest's full record
  tx.add(SystemProgram.transfer({ fromPubkey: payer, toPubkey: new PublicKey(inbox), lamports: 0 }));
  tx.add(new TransactionInstruction({
    programId: new PublicKey(MEMO_PROGRAM_ID),
    keys: [{ pubkey: payer, isSigner: true, isWritable: false }],
    data: encodeUtf8(web3, memo),
  }));
  return tx;
}

function encodeUtf8(web3, s) {
  const bytes = new TextEncoder().encode(s);
  // web3.js expects a Buffer; the IIFE build exposes one through its internals,
  // and Node has a global Buffer.
  if (typeof Buffer !== 'undefined') return Buffer.from(bytes);
  if (web3.Buffer) return web3.Buffer.from(bytes);
  return bytes;
}

/** Fetch the full ordered record of a contest inbox: oldest first. */
export async function fetchRecords(web3, connection, inbox) {
  const { PublicKey } = web3;
  const sigs = [];
  let before;
  for (;;) {
    const page = await connection.getSignaturesForAddress(new PublicKey(inbox), { before, limit: 1000 }, 'confirmed');
    sigs.push(...page);
    if (page.length < 1000) break;
    before = page[page.length - 1].signature;
  }
  sigs.reverse(); // oldest first
  const out = [];
  for (let i = 0; i < sigs.length; i++) {
    const s = sigs[i];
    if (s.err) continue;
    const tx = await connection.getParsedTransaction(s.signature, { commitment: 'confirmed', maxSupportedTransactionVersion: 0 });
    if (!tx) continue;
    const signer = tx.transaction.message.accountKeys.find(k => k.signer)?.pubkey?.toBase58?.() || String(tx.transaction.message.accountKeys[0].pubkey);
    const memoIx = tx.transaction.message.instructions.find(ix => (ix.programId?.toBase58?.() || String(ix.programId)) === 'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr');
    const memo = memoIx ? (memoIx.parsed ?? '') : (s.memo || '');
    out.push({ sig: s.signature, slot: s.slot, blockTime: s.blockTime, signer, memo, idx: i });
  }
  return out;
}
