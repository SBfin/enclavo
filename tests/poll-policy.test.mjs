import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MAX_DELAY_MS, ago, describeError, nextDelay } from '../src/lib/poll-policy.ts';

test('polls at the interval while reads succeed and backs off after failures', () => {
  assert.equal(nextDelay(20_000, 0), 20_000);
  assert.equal(nextDelay(20_000, 1), 40_000);
  assert.equal(nextDelay(20_000, 2), 80_000);
  assert.equal(nextDelay(20_000, 3), MAX_DELAY_MS);
  assert.equal(nextDelay(20_000, 1_000), MAX_DELAY_MS);
});

test('rate limits and blocked responses read as an unreachable RPC', () => {
  const unreachable = describeError(new TypeError('Failed to fetch'));
  assert.match(unreachable, /isn't answering/);
  assert.equal(describeError(new Error('NEAR RPC returned 429')), unreachable);
  assert.equal(describeError(new Error('NetworkError when attempting to fetch resource.')), unreachable);
  assert.equal(describeError(new Error('Load failed')), unreachable);
});

test('contract errors pass through', () => {
  assert.equal(describeError(new Error('Auction not found')), 'Auction not found');
  assert.equal(describeError(undefined), 'Could not read the chain.');
});

test('ages', () => {
  const now = 1_000_000_000;
  assert.equal(ago(now - 2_000, now), 'just now');
  assert.equal(ago(now - 40_000, now), '40s ago');
  assert.equal(ago(now - 180_000, now), '3 min ago');
  assert.equal(ago(now - 2 * 3_600_000, now), '2 h ago');
});
