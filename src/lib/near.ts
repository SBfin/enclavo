// Read-only access to the NEAR auction contract, plus the shapes its views return.
// Amounts are the settlement token's atomic units as decimal strings; with 6 decimals that is micro-USD.

export type ChainInfo = { network: "testnet" | "mainnet"; auction_contract: string; rpc_url: string };

export type ChainAuction = { id: number; route_id: string; units: number; term_days: number; reserve_price: string; tokens_per_iu: number; created_at_ms: number; ends_at_ms: number; status: "open" | "cleared"; cleared_at_ms: number | null; clearing_price: string | null; filled_units: number; open_bids: number; open_bid_units: number };
export type ChainBid = { auction_id: number; index: number; bidder: string; units: number; max_price: string; escrow: string; created_at_ms: number; status: "open" | "filled" | "partial" | "outbid" | "cancelled"; filled_units: number; charged: string };
export type ChainEntitlement = { id: number; auction_id: number; owner: string; route_id: string; units: number; tokens_per_iu: number; starts_at_ms: number; ends_at_ms: number; paid: string };

export const USD = 1_000_000;
export const usd = (atomic: string | number | null | undefined) => Number(atomic ?? 0) / USD;
/// Gas and deposit for a call that moves tokens: NEP-141 requires exactly one yoctoNEAR attached.
export const TOKEN_CALL = { gas: "100000000000000", deposit: "1" };
export const PLAIN_CALL = { gas: "100000000000000", deposit: "0" };

export async function view<T>(chain: ChainInfo, contract: string, method: string, args: object = {}, signal?: AbortSignal): Promise<T> {
  const response = await fetch(chain.rpc_url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    signal,
    body: JSON.stringify({ jsonrpc: "2.0", id: "enclavo", method: "query", params: { request_type: "call_function", finality: "optimistic", account_id: contract, method_name: method, args_base64: btoa(JSON.stringify(args)) } }),
  });
  const body = await response.json();
  const error = body.error ?? body.result?.error;
  if (error) throw new Error(typeof error === "string" ? error : error.data ?? error.message ?? "NEAR RPC error");
  return JSON.parse(new TextDecoder().decode(new Uint8Array(body.result.result)));
}

export async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// A gateway API key in the same format the gateway issues. Only its hash goes on chain.
export function newApiKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return `ek_${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}
