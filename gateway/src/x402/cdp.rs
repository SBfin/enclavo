//! Coinbase CDP API-key authentication: a short-lived JWT per request, scoped to its method and
//! path (docs.cdp.coinbase.com/api-reference/v2/authentication).

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::json;

/// Lifetime of each token; CDP's own SDKs use the same.
const TOKEN_SECS: u64 = 120;

enum Key {
    Ed25519(ed25519_dalek::SigningKey),
    Es256(p256::ecdsa::SigningKey),
}

pub struct CdpSigner {
    key_id: String,
    key: Key,
}

impl CdpSigner {
    /// `secret` is either a base64 Ed25519 key (32-byte seed followed by the 32-byte public key,
    /// as CDP issues them) or an EC private key in PEM, for older ECDSA keys.
    pub fn new(key_id: &str, secret: &str) -> anyhow::Result<Self> {
        let secret = secret.trim().replace("\\n", "\n");
        let key = if secret.starts_with("-----BEGIN") {
            use p256::pkcs8::DecodePrivateKey;
            let key = p256::ecdsa::SigningKey::from_pkcs8_pem(&secret)
                .or_else(|_| p256::SecretKey::from_sec1_pem(&secret).map(p256::ecdsa::SigningKey::from))
                .context("CDP secret looks like PEM but is not a P-256 private key")?;
            Key::Es256(key)
        } else {
            let bytes = STANDARD.decode(&secret).context("CDP secret is neither PEM nor base64")?;
            let seed: [u8; 32] = match bytes.len() {
                32 | 64 => bytes[..32].try_into().expect("length checked"),
                n => bail!("CDP Ed25519 secret decodes to {n} bytes; expected 64 (seed and public key)"),
            };
            let key = ed25519_dalek::SigningKey::from_bytes(&seed);
            if bytes.len() == 64 && key.verifying_key().to_bytes()[..] != bytes[32..] {
                bail!("CDP Ed25519 secret is inconsistent: its public half does not match its seed");
            }
            Key::Ed25519(key)
        };
        Ok(Self { key_id: key_id.trim().to_string(), key })
    }

    /// `Authorization` header value for one request to `url`.
    pub fn authorization(&self, method: &str, url: &reqwest::Url) -> String {
        format!("Bearer {}", self.token(method, url, now()))
    }

    fn token(&self, method: &str, url: &reqwest::Url, now: u64) -> String {
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        let alg = match self.key {
            Key::Ed25519(_) => "EdDSA",
            Key::Es256(_) => "ES256",
        };
        let nonce = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        let header = json!({ "alg": alg, "kid": self.key_id, "typ": "JWT", "nonce": nonce });
        let claims = json!({
            "sub": self.key_id,
            "iss": "cdp",
            "aud": ["cdp_service"],
            "nbf": now,
            "exp": now + TOKEN_SECS,
            "uri": format!("{} {host}{}", method.to_uppercase(), url.path()),
        });
        let signing_input =
            format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(claims.to_string()));
        let signature = match &self.key {
            Key::Ed25519(key) => {
                use ed25519_dalek::Signer;
                key.sign(signing_input.as_bytes()).to_bytes().to_vec()
            }
            Key::Es256(key) => {
                use p256::ecdsa::signature::Signer;
                let sig: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
                sig.to_bytes().to_vec()
            }
        };
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn parts(token: &str) -> (Value, Value, Vec<u8>, String) {
        let mut it = token.split('.');
        let (h, c, s) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        assert!(it.next().is_none());
        let decode = |p: &str| serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p).unwrap()).unwrap();
        (decode(h), decode(c), URL_SAFE_NO_PAD.decode(s).unwrap(), format!("{h}.{c}"))
    }

    fn url(u: &str) -> reqwest::Url {
        u.parse().unwrap()
    }

    #[test]
    fn ed25519_token_matches_cdp_format_and_verifies() {
        let seed = [7u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
        let secret = STANDARD.encode([seed.as_slice(), public.as_bytes()].concat());
        let signer = CdpSigner::new("key-123", &secret).unwrap();

        let token = signer.token("post", &url("https://api.cdp.coinbase.com/platform/v2/x402/settle"), 1_000);
        let (header, claims, sig, input) = parts(&token);
        assert_eq!(header["alg"], "EdDSA");
        assert_eq!(header["kid"], "key-123");
        assert_eq!(header["typ"], "JWT");
        assert_eq!(header["nonce"].as_str().unwrap().len(), 16);
        assert_eq!(claims["sub"], "key-123");
        assert_eq!(claims["iss"], "cdp");
        assert_eq!(claims["aud"], json!(["cdp_service"]));
        assert_eq!(claims["nbf"], 1_000);
        assert_eq!(claims["exp"], 1_120);
        assert_eq!(claims["uri"], "POST api.cdp.coinbase.com/platform/v2/x402/settle");

        let sig = ed25519_dalek::Signature::from_bytes(&sig.try_into().unwrap());
        public.verify_strict(input.as_bytes(), &sig).expect("signature verifies with the key's public half");
    }

    #[test]
    fn es256_pem_keys_sign_too() {
        use p256::pkcs8::{EncodePrivateKey, LineEnding};
        let key = p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        // Secrets pasted into env vars often carry literal "\n" sequences.
        let signer = CdpSigner::new("organizations/o/apiKeys/k", &pem.replace('\n', "\\n")).unwrap();

        let token = signer.token("POST", &url("http://127.0.0.1:4021/verify"), 5);
        let (header, claims, sig, input) = parts(&token);
        assert_eq!(header["alg"], "ES256");
        assert_eq!(claims["uri"], "POST 127.0.0.1:4021/verify");
        let sig = p256::ecdsa::Signature::from_slice(&sig).unwrap();
        use p256::ecdsa::signature::Verifier;
        key.verifying_key().verify(input.as_bytes(), &sig).unwrap();
    }

    #[test]
    fn rejects_malformed_secrets() {
        assert!(CdpSigner::new("k", "not base64 !!").is_err());
        assert!(CdpSigner::new("k", &STANDARD.encode([1u8; 10])).is_err());
        let mismatched = STANDARD.encode([[1u8; 32], [2u8; 32]].concat());
        assert!(CdpSigner::new("k", &mismatched).is_err());
    }

    #[test]
    fn every_token_has_a_fresh_nonce() {
        let signer = CdpSigner::new("k", &STANDARD.encode([3u8; 32])).unwrap();
        let u = url("https://api.cdp.coinbase.com/platform/v2/x402/verify");
        assert_ne!(parts(&signer.token("POST", &u, 1)).0["nonce"], parts(&signer.token("POST", &u, 1)).0["nonce"]);
    }
}
