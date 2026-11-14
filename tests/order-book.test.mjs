import assert from 'node:assert/strict';
import { test } from 'node:test';
import { clearBook } from '../src/lib/order-book.ts';

const $ = (dollars) => Math.round(dollars * 1e6);

test('an oversubscribed lot clears at the lowest winning bid', () => {
  // The worked example on the How it works page.
  const book = clearBook([
    { index: 0, units: 6, price: $(0.9) },
    { index: 1, units: 6, price: $(0.7) },
    { index: 2, units: 4, price: $(0.6) },
  ], 10, $(0.5));
  assert.equal(book.price, $(0.7));
  assert.equal(book.filled, 10);
  assert.equal(book.demand, 16);
  assert.ok(book.oversubscribed);
  assert.deepEqual(book.levels.map((l) => [l.filled, l.cumulative]), [[6, 6], [4, 12], [0, 16]]);
});

test('a lot with spare capacity clears at the reserve', () => {
  const book = clearBook([{ index: 0, units: 3, price: $(0.9) }], 10, $(0.5));
  assert.equal(book.price, $(0.5));
  assert.equal(book.filled, 3);
  assert.equal(book.oversubscribed, false);
});

test('bids at one price share a level and earlier bids win ties', () => {
  const book = clearBook([
    { index: 1, units: 4, price: $(0.8), mine: true },
    { index: 0, units: 4, price: $(0.8) },
    { index: 2, units: 4, price: $(0.8), draft: true },
  ], 6, $(0.5));
  assert.equal(book.levels.length, 1);
  assert.deepEqual(book.levels[0], { price: $(0.8), units: 12, bids: 3, filled: 6, cumulative: 12, mine: true, draft: true });
  assert.equal(book.draftFill, 0);
  assert.equal(book.price, $(0.8));
});

test('a draft bid shows what it would win and how it moves the price', () => {
  const bids = [{ index: 0, units: 8, price: $(0.6) }];
  assert.equal(clearBook(bids, 10, $(0.5)).price, $(0.5));
  const book = clearBook([...bids, { index: Infinity, units: 5, price: $(0.75), draft: true }], 10, $(0.5));
  assert.equal(book.draftFill, 5);
  assert.equal(book.price, $(0.6));
});

test('an empty book fills nothing', () => {
  const book = clearBook([], 10, $(0.5));
  assert.deepEqual([book.levels, book.filled, book.price], [[], 0, null]);
});
