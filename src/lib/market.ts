// Snapshot of hardware-attested routes, priced in IU (1 IU = 1M GLM 5.3 Flash tokens per day).
// Conversion rates and prices come from the IU index draft: AA Intelligence Index v4.3.2
// and provider price lists captured Oct 26, 2026.

export type Route = {
  id: string;
  model: string;
  provider: string;
  attestation: string;
  score: number;
  tokensPerIU: number; // tokens per day that make up 1 IU on this model
  pricePerIUDay: number; // USD
};

export const FLOOR = 40;

export const ROUTES: Route[] = [
  { id: "glm-5.3-flash@near-ai", model: "GLM 5.3 Flash", provider: "NEAR AI", attestation: "Own TEE", score: 42, tokensPerIU: 1_000_000, pricePerIUDay: 0.185 },
  { id: "deepseek-v4.1-flash-1017@redpill", model: "DeepSeek V4.1 Flash 1017", provider: "RedPill", attestation: "Tinfoil TEE", score: 45, tokensPerIU: 241_149, pricePerIUDay: 0.232 },
  { id: "glm-5.3-flash@redpill", model: "GLM 5.3 Flash", provider: "RedPill", attestation: "Partner TEE", score: 42, tokensPerIU: 1_000_000, pricePerIUDay: 0.2375 },
  { id: "glm-5.3-flash@venice", model: "GLM 5.3 Flash", provider: "Venice", attestation: "TEE + E2EE", score: 42, tokensPerIU: 1_000_000, pricePerIUDay: 0.255 },
  { id: "glm-5.3-max@redpill", model: "GLM 5.3 (max)", provider: "RedPill", attestation: "Tinfoil TEE", score: 45, tokensPerIU: 455_229, pricePerIUDay: 0.978 },
  { id: "glm-5.3-max@venice", model: "GLM 5.3 (max)", provider: "Venice", attestation: "TEE + E2EE", score: 45, tokensPerIU: 455_229, pricePerIUDay: 1.223 },
  { id: "qwen-3.8-27b@near-ai", model: "Qwen 3.8 27B", provider: "NEAR AI", attestation: "Own TEE", score: 34, tokensPerIU: 3_300_000, pricePerIUDay: 1.231 },
  { id: "glm-5.3-max@near-ai", model: "GLM 5.3 (max)", provider: "NEAR AI", attestation: "Partner TEE", score: 45, tokensPerIU: 455_229, pricePerIUDay: 1.268 },
];

export const INDEX = {
  privateIndex: 0.255, // median of attested offers above the floor
  privateCheapest: 0.185,
  reference: 0.116, // fitted cost curve at the calibration model's score
};

export const TERMS = [7, 30, 90] as const;

export function findRoute(id: string): Route | undefined {
  return ROUTES.find((r) => r.id === id);
}

export function fmtTokens(n: number): string {
  const a = Math.abs(n);
  if (a >= 1e12) return `${(n / 1e12).toFixed(a >= 1e13 ? 0 : 1)}T`;
  if (a >= 1e9) return `${(n / 1e9).toFixed(a >= 1e10 ? 0 : 1)}B`;
  if (a >= 1e6) return `${(n / 1e6).toFixed(a >= 1e7 ? 0 : 1)}M`;
  if (a >= 1e3) return `${(n / 1e3).toFixed(a >= 1e4 ? 0 : 1)}k`;
  return n.toFixed(0);
}

export function fmtUSD(n: number, digits?: number): string {
  if (digits !== undefined) return `$${n.toFixed(digits)}`;
  if (Math.abs(n) >= 1000) return `$${Math.round(n).toLocaleString("en-US")}`;
  if (Math.abs(n) >= 1) return `$${n.toFixed(2)}`;
  return `$${n.toFixed(3)}`;
}
