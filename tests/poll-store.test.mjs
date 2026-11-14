import assert from 'node:assert/strict';
import { mock, test } from 'node:test';
import { createPoller } from '../src/lib/poll-store.ts';

// Backoff doubles the interval per failure, uncapped here; poll-policy tests cover the cap.
const delay = (interval, failures) => interval * 2 ** failures;
const describe = (error) => error.message;
const flush = () => new Promise((resolve) => setImmediate(resolve));

function setup() {
  mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 1_000_000 });
  const page = { hidden: false };
  const poller = createPoller({ delay, describe, hidden: () => page.hidden });
  return { page, poller };
}

/** A load whose reads the test settles by hand. */
function manualLoad() {
  const reads = [];
  const load = (signal) => new Promise((resolve, reject) => reads.push({ resolve, reject, signal }));
  return { reads, load };
}

async function tick(ms) {
  mock.timers.tick(ms);
  await flush();
}

test('subscribers to one key share one loop', async (t) => {
  t.after(() => mock.timers.reset());
  const { poller } = setup();
  const { reads, load } = manualLoad();
  const seen = [];
  poller.subscribe('k', load, 20_000, () => seen.push('a'));
  poller.subscribe('k', load, 20_000, () => seen.push('b'));
  await tick(0);
  assert.equal(reads.length, 1);
  reads[0].resolve(7);
  await flush();
  assert.deepEqual(seen, ['a', 'b']);
  assert.equal(poller.state('k').data, 7);
  await tick(19_999);
  assert.equal(reads.length, 1);
  await tick(1);
  assert.equal(reads.length, 2);
});

test('failures keep the last data and back off; a success resets', async (t) => {
  t.after(() => mock.timers.reset());
  const { poller } = setup();
  const { reads, load } = manualLoad();
  poller.subscribe('k', load, 10_000, () => {});
  await tick(0);
  reads[0].resolve('good');
  await flush();
  const loadedAt = poller.state('k').updatedAt;

  await tick(10_000);
  reads[1].reject(new Error('Failed to fetch'));
  await flush();
  assert.deepEqual(poller.state('k'), { data: 'good', error: 'Failed to fetch', updatedAt: loadedAt, failures: 1 });

  await tick(19_999);
  assert.equal(reads.length, 2, 'waits twice the interval after one failure');
  await tick(1);
  reads[2].reject(new Error('Failed to fetch'));
  await flush();
  assert.equal(poller.state('k').failures, 2);
  await tick(39_999);
  assert.equal(reads.length, 3, 'waits four times the interval after two failures');
  await tick(1);
  reads[3].resolve('fresh');
  await flush();
  assert.equal(poller.state('k').error, null);
  assert.equal(poller.state('k').failures, 0);
  await tick(10_000);
  assert.equal(reads.length, 5, 'back to the normal interval');
});

test('a loop pauses while the page is hidden and reads on return when overdue', async (t) => {
  t.after(() => mock.timers.reset());
  const { page, poller } = setup();
  const { reads, load } = manualLoad();
  poller.subscribe('k', load, 10_000, () => {});
  await tick(0);
  reads[0].resolve(1);
  await flush();

  page.hidden = true;
  poller.visibilityChanged();
  await tick(60_000);
  assert.equal(reads.length, 1);

  page.hidden = false;
  poller.visibilityChanged();
  await tick(0);
  assert.equal(reads.length, 2);
});

test('returning early to the page waits out the rest of the interval', async (t) => {
  t.after(() => mock.timers.reset());
  const { page, poller } = setup();
  const { reads, load } = manualLoad();
  poller.subscribe('k', load, 10_000, () => {});
  await tick(0);
  reads[0].resolve(1);
  await flush();
  page.hidden = true;
  poller.visibilityChanged();
  await tick(4_000);
  page.hidden = false;
  poller.visibilityChanged();
  await tick(5_999);
  assert.equal(reads.length, 1);
  await tick(1);
  assert.equal(reads.length, 2);
});

test('rejoining while a cancelled read is still pending starts a new read', async (t) => {
  t.after(() => mock.timers.reset());
  const { poller } = setup();
  const { reads, load } = manualLoad();
  const leave = poller.subscribe('k', load, 10_000, () => {});
  await tick(0);
  assert.equal(reads.length, 1);
  // Navigating between pages: every subscriber leaves, cancelling the read, then new ones join.
  leave();
  assert.equal(reads[0].signal.aborted, true);
  let notified = 0;
  poller.subscribe('k', load, 10_000, () => notified++);
  await tick(0);
  assert.equal(reads.length, 2);
  reads[0].reject(new DOMException('aborted', 'AbortError'));
  await flush();
  assert.equal(poller.state('k').failures, 0, 'a cancelled read is not a failure');
  reads[1].resolve('ok');
  await flush();
  assert.equal(notified, 1);
  assert.equal(poller.state('k').data, 'ok');
});

test('refresh reads now, after a read in progress', async (t) => {
  t.after(() => mock.timers.reset());
  const { poller } = setup();
  const { reads, load } = manualLoad();
  poller.subscribe('k', load, 60_000, () => {});
  await tick(0);
  const refreshed = poller.refresh('k');
  reads[0].resolve('stale');
  await flush();
  assert.equal(reads.length, 2);
  reads[1].resolve('after tx');
  await refreshed;
  assert.equal(poller.state('k').data, 'after tx');
});
