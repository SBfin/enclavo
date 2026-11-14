import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_FILTERS, countByStatus, filterAuctions, narrowed, phase, timeLeft } from '../src/lib/auction-listing.ts';

const NOW = 1_800_000_000_000;
const HOUR = 3_600_000;

function auction(id, overrides = {}) {
  return {
    id, route_id: 'z-ai/glm@near-ai', units: 10, term_days: 7, reserve_price: '500000', tokens_per_iu: 1_000_000,
    created_at_ms: NOW - 10 * HOUR + id, ends_at_ms: NOW + HOUR, status: 'open', cleared_at_ms: null, clearing_price: null,
    filled_units: 0, open_bids: 0, open_bid_units: 0, ...overrides,
  };
}
const NAMES = { 'z-ai/glm@near-ai': 'GLM 5.3 Flash', 'qwen/qwen@near-ai': 'Qwen 3.8 27B' };
const nameOf = (route) => NAMES[route] ?? route;
const ids = (list) => list.map((a) => a.id);
const run = (auctions, filters) => ids(filterAuctions(auctions, { ...DEFAULT_FILTERS, ...filters }, NOW, nameOf));

const BOOK = [
  auction(1, { ends_at_ms: NOW + 3 * HOUR, open_bid_units: 12 }),
  auction(2, { ends_at_ms: NOW + HOUR, route_id: 'qwen/qwen@near-ai', term_days: 30, reserve_price: '1350000', units: 50 }),
  auction(3, { ends_at_ms: NOW - HOUR, open_bid_units: 4 }),
  auction(4, { ends_at_ms: NOW - 9 * HOUR, status: 'cleared', cleared_at_ms: NOW - 8 * HOUR, filled_units: 10, clearing_price: '800000', reserve_price: '400000' }),
  auction(5, { ends_at_ms: NOW - 5 * HOUR, status: 'cleared', cleared_at_ms: NOW - 4 * HOUR, filled_units: 3, clearing_price: '500000' }),
];

test('phases follow the clock and the contract status', () => {
  assert.deepEqual(BOOK.map((a) => phase(a, NOW)), ['open', 'open', 'closed', 'settled', 'settled']);
  // Before the page has a clock nothing counts as closed yet.
  assert.equal(phase(BOOK[2], 0), 'open');
  assert.deepEqual(countByStatus(BOOK, NOW), { open: 2, closed: 1, settled: 2, all: 5 });
});

test('the default view lists open auctions, closing soonest first', () => {
  assert.deepEqual(run(BOOK, {}), [2, 1]);
});

test('closing order across every status: open, then waiting to settle, then latest settled', () => {
  assert.deepEqual(run(BOOK, { status: 'all' }), [2, 1, 3, 5, 4]);
});

test('filters by model, text, term and unclaimed capacity', () => {
  assert.deepEqual(run(BOOK, { status: 'all', route: 'qwen/qwen@near-ai' }), [2]);
  assert.deepEqual(run(BOOK, { status: 'all', query: 'GLM' }), [1, 3, 5, 4]);
  assert.deepEqual(run(BOOK, { status: 'all', query: '  qwen/QWEN ' }), [2]);
  assert.deepEqual(run(BOOK, { status: 'all', term: 30 }), [2]);
  // Auction 1 is oversubscribed and auction 4 sold out; the rest still have room.
  assert.deepEqual(run(BOOK, { status: 'all', unclaimed: true }), [2, 3, 5]);
  assert.deepEqual(run(BOOK, { status: 'closed', term: 30 }), []);
});

test('sorts by reserve, lot, demand and age, breaking ties by newest id', () => {
  assert.deepEqual(run(BOOK, { status: 'all', sort: 'reserve-asc' }), [4, 5, 3, 1, 2]);
  assert.deepEqual(run(BOOK, { status: 'all', sort: 'reserve-desc' }), [2, 5, 3, 1, 4]);
  assert.deepEqual(run(BOOK, { status: 'all', sort: 'lot' }), [2, 5, 4, 3, 1]);
  assert.deepEqual(run(BOOK, { status: 'all', sort: 'demand' }), [1, 4, 3, 5, 2]);
  assert.deepEqual(run(BOOK, { status: 'all', sort: 'newest' }), [5, 4, 3, 2, 1]);
});

test('knows when the list is narrowed beyond status and sort', () => {
  assert.equal(narrowed(DEFAULT_FILTERS), false);
  assert.equal(narrowed({ ...DEFAULT_FILTERS, status: 'all', sort: 'lot' }), false);
  assert.equal(narrowed({ ...DEFAULT_FILTERS, query: ' ' }), false);
  assert.equal(narrowed({ ...DEFAULT_FILTERS, term: 7 }), true);
});

test('countdowns', () => {
  assert.equal(timeLeft(NOW + 26 * HOUR, NOW), '1d 2h');
  assert.equal(timeLeft(NOW + 90 * 60_000, NOW), '1h 30m');
  assert.equal(timeLeft(NOW + 61_000, NOW), '1m 1s');
  assert.equal(timeLeft(NOW - 1, NOW), 'closed');
  assert.equal(timeLeft(NOW + HOUR, 0), '…');
});
