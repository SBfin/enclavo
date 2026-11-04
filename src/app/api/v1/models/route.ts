import { ROUTES } from "@/lib/market";

// OpenAI-style model list. Each route is addressable as its own model ID.
export function GET() {
  return Response.json({
    object: "list",
    data: ROUTES.map((r) => ({
      id: r.id,
      object: "model",
      owned_by: r.provider,
      name: r.model,
      attestation: r.attestation,
      score: r.score,
      tokens_per_iu: r.tokensPerIU,
      price_per_iu_day: r.pricePerIUDay,
    })),
  });
}
