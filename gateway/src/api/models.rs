use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::{Shared, catalog::AUTO_MODEL, router::CircuitState};

/// OpenAI-style model list. Every route is addressable as its own model id.
pub async fn list(State(state): State<Shared>) -> Json<Value> {
    let mut data: Vec<Value> = state
        .catalog
        .routes
        .iter()
        .map(|r| {
            let usd_per_iu = r.micro_usd_per_iu as f64 / 1e6;
            json!({
                "id": r.id,
                "object": "model",
                "created": 0,
                "owned_by": r.provider.id,
                "name": r.display_name,
                "model": r.model,
                "description": r.metadata["modelDescription"],
                "context_length": r.metadata["contextLength"],
                "supported_features": r.metadata["supportedFeatures"],
                "input_modalities": r.metadata["architecture"]["inputModalities"],
                "provider_type": r.metadata["providerType"],
                "attestation": r.attestation,
                "attested": r.attested,
                "score": r.score,
                "tokens_per_iu": r.tokens_per_iu,
                "price_per_iu_usd": usd_per_iu,
                "price_per_million_tokens_usd": usd_per_iu * 1e6 / r.tokens_per_iu as f64,
                "input_price_per_million_tokens_usd": r.input_micro_usd_per_iu.map(|price| price as f64 / r.tokens_per_iu as f64).unwrap_or(usd_per_iu * 1e6 / r.tokens_per_iu as f64 * r.prompt_weight_bps as f64 / 10_000.0),
                "attestation_verification": "not_verified",
                "max_output_tokens": r.max_output_tokens,
                "available": state.router.circuit(&r.id) != CircuitState::Open,
            })
        })
        .collect();
    data.push(json!({
        "id": AUTO_MODEL,
        "object": "model",
        "created": 0,
        "owned_by": "enclavo",
        "name": "Best available route (use `iu:<score>` to set a minimum score)",
    }));
    Json(json!({ "object": "list", "data": data }))
}
