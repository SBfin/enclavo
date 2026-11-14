import assert from 'node:assert/strict';
import { test } from 'node:test';
import { generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import {
  SIGN_IN_MESSAGE, SIGN_IN_RECIPIENT, base58Decode, checkChallengeNonce, hasFullAccessKey, isAccountId,
  makeChallengeNonce, nep413Hash, readSession, signSession, verifyNep413,
} from '../src/lib/near-auth-core.ts';

const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function base58(bytes) {
  let n = BigInt('0x' + (Buffer.from(bytes).toString('hex') || '0')), out = '';
  while (n > 0n) { out = ALPHABET[Number(n % 58n)] + out; n /= 58n; }
  for (const byte of bytes) { if (byte !== 0) break; out = '1' + out; }
  return out;
}
function wallet() {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(12);
  return { publicKey: `ed25519:${base58(raw)}`, sign: (data) => sign(null, data, privateKey) };
}
const key = randomBytes(32);

test('base58 round-trips, including leading zeros', () => {
  const bytes = Buffer.concat([Buffer.from([0, 0]), randomBytes(30)]);
  assert.deepEqual(base58Decode(base58(bytes)), bytes);
});

test('a NEP-413 signature verifies only for the exact payload and key', () => {
  const w = wallet(), nonce = randomBytes(32);
  const signature = w.sign(nep413Hash(SIGN_IN_MESSAGE, nonce, SIGN_IN_RECIPIENT)).toString('base64');
  const input = { publicKey: w.publicKey, signature, message: SIGN_IN_MESSAGE, nonce, recipient: SIGN_IN_RECIPIENT };
  assert.equal(verifyNep413(input), true);
  assert.equal(verifyNep413({ ...input, signature: `ed25519:${base58(Buffer.from(signature, 'base64'))}` }), true);
  assert.equal(verifyNep413({ ...input, message: 'Something else' }), false);
  assert.equal(verifyNep413({ ...input, recipient: 'attacker' }), false);
  assert.equal(verifyNep413({ ...input, nonce: randomBytes(32) }), false);
  assert.equal(verifyNep413({ ...input, publicKey: wallet().publicKey }), false);
  assert.equal(verifyNep413({ ...input, signature: 'not a signature' }), false);
});

test('challenge nonces are tamper-evident and expire', () => {
  const nonce = makeChallengeNonce(key, 1_000);
  assert.equal(checkChallengeNonce(key, nonce, 2_000), true);
  assert.equal(checkChallengeNonce(key, nonce, 1_000 + 5 * 60_000 + 1), false);
  assert.equal(checkChallengeNonce(randomBytes(32), nonce, 2_000), false);
  const forged = Buffer.from(nonce); forged[3] ^= 1;
  assert.equal(checkChallengeNonce(key, forged, 2_000), false);
});

test('sessions carry the account, reject tampering and expire after a week', () => {
  const token = signSession(key, 'alice.testnet', 0);
  assert.equal(readSession(key, token, 1), 'alice.testnet');
  assert.equal(readSession(key, token, 7 * 24 * 60 * 60_000 + 1), null);
  assert.equal(readSession(randomBytes(32), token, 1), null);
  const [, mac] = token.split('.');
  const forged = `${Buffer.from(JSON.stringify({ a: 'bob.testnet', e: 9e15 })).toString('base64url')}.${mac}`;
  assert.equal(readSession(key, forged, 1), null);
  assert.equal(readSession(key, undefined, 1), null);
});

test('account IDs follow NEAR rules', () => {
  for (const ok of ['alice.testnet', 'a1-b_c.near', 'ab', 'f'.repeat(64)]) assert.equal(isAccountId(ok), true, ok);
  for (const bad of ['A.near', 'a', '.near', 'a..near', 'a.near.', 'x'.repeat(65), 12]) assert.equal(isAccountId(bad), false, String(bad));
});

test('only a full-access key is accepted, and RPC failures fail closed', async () => {
  const rpc = (result) => async (url, init) => {
    const body = JSON.parse(init.body);
    assert.equal(body.params.request_type, 'view_access_key');
    assert.equal(body.params.account_id, 'alice.testnet');
    return new Response(JSON.stringify(result));
  };
  assert.equal(await hasFullAccessKey('https://rpc', 'alice.testnet', 'ed25519:x', rpc({ result: { permission: 'FullAccess' } })), true);
  assert.equal(await hasFullAccessKey('https://rpc', 'alice.testnet', 'ed25519:x', rpc({ result: { permission: { FunctionCall: {} } } })), false);
  assert.equal(await hasFullAccessKey('https://rpc', 'alice.testnet', 'ed25519:x', rpc({ error: { message: 'unknown key' } })), false);
  assert.equal(await hasFullAccessKey('https://rpc', 'alice.testnet', 'ed25519:x', async () => { throw new Error('down'); }), false);
});
