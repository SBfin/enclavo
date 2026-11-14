// The auction's order book and the outcome if it closed now. This mirrors `clear_auction` in
// contracts/auction: open bids fill from the highest maximum price down (earlier bids win ties),
// and every winner pays one price, the reserve if demand fits in the lot, otherwise the lowest
// price that still won. Prices are the token's atomic units (micro-USD) per IU-day.

export type BookBid = { index: number; units: number; price: number; mine?: boolean; draft?: boolean };

export type Level = {
  price: number;
  units: number;
  bids: number;
  /** IU/day this level would receive if the auction closed now. */
  filled: number;
  /** IU/day bid at this price or higher. */
  cumulative: number;
  mine: boolean;
  draft: boolean;
};

export type Book = {
  levels: Level[];
  demand: number;
  filled: number;
  /** Clearing price if the auction closed now; null when nothing would fill. */
  price: number | null;
  oversubscribed: boolean;
  /** IU/day the draft bid would receive, when one is given. */
  draftFill: number;
};

export function clearBook(bids: BookBid[], lot: number, reserve: number): Book {
  const order = [...bids].sort((a, b) => b.price - a.price || a.index - b.index);
  let remaining = lot, demand = 0, marginal = reserve, draftFill = 0;
  const levels: Level[] = [];
  for (const bid of order) {
    demand += bid.units;
    const fill = Math.min(remaining, bid.units);
    remaining -= fill;
    if (fill > 0) marginal = bid.price;
    if (bid.draft) draftFill = fill;
    let level = levels.at(-1);
    if (!level || level.price !== bid.price) {
      level = { price: bid.price, units: 0, bids: 0, filled: 0, cumulative: 0, mine: false, draft: false };
      levels.push(level);
    }
    level.units += bid.units;
    level.bids += 1;
    level.filled += fill;
    level.cumulative = demand;
    level.mine ||= Boolean(bid.mine);
    level.draft ||= Boolean(bid.draft);
  }
  const filled = lot - remaining;
  return { levels, demand, filled, price: filled > 0 ? (demand > lot ? marginal : reserve) : null, oversubscribed: demand > lot, draftFill };
}
