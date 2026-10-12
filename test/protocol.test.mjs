import assert from 'node:assert/strict';
import { commitHash, submitHash, memoCommit, memoSubmit, memoReveal, adjudicate } from '../lib/protocol.mjs';
const H='HOUSE', id='T-1', salt='abc123', ans='Don\'t trust, verify';
const rec=(i,slot,signer,memo)=>({sig:'s'+i,slot,signer,memo,idx:i});
const c=await commitHash(id,salt,ans);
const good=async w=>memoSubmit(id,await submitHash(id,w,ans));
const bad=async w=>memoSubmit(id,await submitHash(id,w,'trust'));
// 1 honest: alice wrong first, bob right, carol right later, dave right after reveal
let R=[rec(0,10,H,memoCommit(id,c)),rec(1,11,'alice',await bad('alice')),rec(2,12,'bob',await good('bob')),rec(3,12,'carol',await good('carol')),rec(4,20,H,memoReveal(id,salt,ans)),rec(5,21,'dave',await good('dave'))];
let r=await adjudicate(id,H,R); assert.equal(r.valid,true); assert.equal(r.winner.signer,'bob'); assert.equal(r.submissions.find(s=>s.signer==='dave').eligible,false);
// 2 copied hash: mallory reposts bob's exact memo -> not correct for mallory's key
R=[rec(0,10,H,memoCommit(id,c)),rec(1,11,'mallory',await good('bob')),rec(2,12,'bob',await good('bob')),rec(3,20,H,memoReveal(id,salt,ans))];
r=await adjudicate(id,H,R); assert.equal(r.winner.signer,'bob'); assert.equal(r.submissions[0].correct,false);
// 3 cheating house: reveals a different answer
R=[rec(0,10,H,memoCommit(id,c)),rec(1,11,'bob',await good('bob')),rec(2,20,H,memoReveal(id,salt,'something else'))];
r=await adjudicate(id,H,R); assert.equal(r.valid,false); assert.equal(r.winner,null);
// 4 forged commit/reveal by non-house ignored
R=[rec(0,9,'eve',memoCommit(id,'00')),rec(1,10,H,memoCommit(id,c)),rec(2,11,'bob',await good('bob')),rec(3,15,'eve',memoReveal(id,'x','y')),rec(4,20,H,memoReveal(id,salt,ans))];
r=await adjudicate(id,H,R); assert.equal(r.valid,true); assert.equal(r.winner.signer,'bob');
// 5 open contest: no reveal -> no winner, sealed
R=[rec(0,10,H,memoCommit(id,c)),rec(1,11,'bob',await good('bob'))];
r=await adjudicate(id,H,R); assert.equal(r.winner,null); assert.equal(r.reveal,null);
// 6 normalization: case/punct-insensitive
assert.equal(await submitHash(id,'x','dont TRUST verify!'),await submitHash(id,'x','DONTTRUSTVERIFY'));
console.log('all 6 protocol tests pass');
