// The four steps of using Enclavo. Shared by the welcome dialog, the Market and the How it works page.
export const STEPS = [
  {
    title: "Connect your wallet",
    text: "Your NEAR account is your Enclavo account. Signing in signs one message; it sends no transaction.",
    href: "/dashboard/account",
    action: "Open Account",
  },
  {
    title: "Bid with USDC",
    text: "Pick a lot, how many IU/day you want and the most you'd pay per IU-day. The contract escrows the maximum.",
    href: "/dashboard/market",
    action: "Go to Market",
  },
  {
    title: "One price at close",
    text: "After the close anyone can settle. Highest bids fill first at a single clearing price; unspent escrow is unlocked.",
    href: "/dashboard/market",
    action: "See auctions",
  },
  {
    title: "Use your capacity",
    text: "Won capacity is a daily token allowance for the whole term, in Playground or with your API keys.",
    href: "/dashboard/playground",
    action: "Open Playground",
  },
] as const;

export const IU_NOTE = "1 IU = one million output-token equivalents per day. Bids and escrow are public on chain; Enclavo never holds your funds.";
