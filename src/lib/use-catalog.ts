"use client";

import { useEffect, useState } from "react";

export type GatewayModel = {
  id: string;
  model: string;
  name: string;
  owned_by: string;
  attestation: string;
  attestation_verification: string;
  available: boolean;
  input_price_per_million_tokens_usd: number;
  price_per_million_tokens_usd: number;
  max_output_tokens: number;
  description?: string;
  context_length?: number;
  supported_features?: string[];
  input_modalities?: string[];
  provider_type?: string;
  tokens_per_iu: number;
  price_per_iu_usd: number;
};

export type GatewayStatus = {
  mode: "live" | "test";
  funding: "operator_credit" | "x402";
  attestation_verification: string;
};

export function useCatalog() {
  const [catalog, setCatalog] = useState<{ models: GatewayModel[]; status: GatewayStatus | null; error: string | null }>({ models: [], status: null, error: null });
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      const read = async (path: string) => {
        const response = await fetch(`/api/v1/${path}`, { cache: "no-store", signal: controller.signal });
        const data = await response.json();
        if (!response.ok) throw new Error(data.error?.message || "Could not load the gateway catalog.");
        return data;
      };
      try {
        const [models, status] = await Promise.all([read("models"), read("status")]);
        if (!controller.signal.aborted) setCatalog({ models: models.data.filter((model: GatewayModel) => model.owned_by !== "enclavo"), status, error: null });
      } catch (error) {
        if (!controller.signal.aborted) setCatalog({ models: [], status: null, error: error instanceof Error ? error.message : "The gateway is unavailable." });
      }
    }
    void load();
    return () => controller.abort();
  }, []);
  return { ...catalog, loading: !catalog.status && !catalog.error };
}
