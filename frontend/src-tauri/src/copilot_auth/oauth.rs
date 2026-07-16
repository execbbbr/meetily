// GitHub Copilot OAuth device flow + token management.
//
// Self-contained module (no dependency on other Meetily modules beyond reqwest/serde),
// so it can be lifted into a standalone crate later. Reimplements the public
// VS Code Copilot OAuth protocol in Rust:
//   1. POST github.com/login/device/code           -> user_code + device_code
//   2. poll github.com/login/oauth/access_token     -> long-lived github token
//   3. GET  api.github.com/copilot_internal/v2/token -> short-lived copilot token
//      (the copilot token embeds `proxy-ep=` which yields the chat API base URL)
//
// The short-lived copilot token is what we hand to the OpenAI-compatible
// /chat/completions endpoint as the bearer credential.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Public VS Code Copilot Chat client id (same value pi-ai / community clients use).
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const DEFAULT_API_BASE: &str = "https://api.individual.githubcopilot.com";

/// Headers Copilot expects to look like the VS Code chat plugin.
pub fn copilot_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("User-Agent", "GitHubCopilotChat/0.35.0"),
        ("Editor-Version", "vscode/1.107.0"),
        ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
        ("Copilot-Integration-Id", "vscode-chat"),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeInfo {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub interval: Option<u64>,
    pub expires_in: u64,
}

/// Persisted credential. `github_token` is the long-lived refresh credential;
/// `copilot_token` + `expires_at_ms` is the short-lived chat credential.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CopilotCredential {
    pub github_token: String,
    pub copilot_token: String,
    /// Unix ms when the copilot_token should be considered expired (with margin).
    pub expires_at_ms: u64,
    pub base_url: String,
}

impl CopilotCredential {
    pub fn is_copilot_token_valid(&self) -> bool {
        if self.copilot_token.is_empty() {
            return false;
        }
        now_ms() < self.expires_at_ms
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn form_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// Step 1: start the device flow. Returns the code to show the user.
pub async fn start_device_flow() -> Result<DeviceCodeInfo, String> {
    let client = form_client()?;
    let resp = client
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.35.0")
        .form(&[("client_id", CLIENT_ID), ("scope", "read:user")])
        .send()
        .await
        .map_err(|e| format!("Device code request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Device code request returned {}: {}", status, body));
    }

    let info: DeviceCodeInfo = resp
        .json()
        .await
        .map_err(|e| format!("Invalid device code response: {}", e))?;

    // Guard: verification_uri must be http(s).
    if !info.verification_uri.starts_with("https://") && !info.verification_uri.starts_with("http://") {
        return Err("Untrusted verification_uri in device code response".to_string());
    }

    Ok(info)
}

#[derive(Debug)]
pub enum PollOutcome {
    /// User authorized; returns the long-lived github access token.
    Complete(String),
    /// Still waiting for the user to authorize.
    Pending,
    /// Server asked us to slow down; optional new interval seconds.
    SlowDown(Option<u64>),
}

/// Step 2 (single poll): exchange device_code for a github access token.
/// Call repeatedly on `interval` until Complete or expiry.
pub async fn poll_access_token(device_code: &str) -> Result<PollOutcome, String> {
    let client = form_client()?;
    let resp = client
        .post(ACCESS_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.35.0")
        .form(&[
            ("client_id", CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("Access token request failed: {}", e))?;

    let body = resp
        .text()
        .await
        .map_err(|e| format!("Failed reading access token response: {}", e))?;

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Invalid access token response: {} ({})", e, body))?;

    if let Some(token) = json.get("access_token").and_then(|v| v.as_str()) {
        return Ok(PollOutcome::Complete(token.to_string()));
    }

    match json.get("error").and_then(|v| v.as_str()) {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown(
            json.get("interval").and_then(|v| v.as_u64()),
        )),
        Some(err) => {
            let desc = json
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Err(format!("Device flow failed: {} {}", err, desc))
        }
        None => Err("Invalid device token response".to_string()),
    }
}

/// Step 3: exchange the long-lived github token for a short-lived copilot token.
/// Also derives the chat API base URL from the token's `proxy-ep=` field.
pub async fn fetch_copilot_token(github_token: &str) -> Result<CopilotCredential, String> {
    let client = form_client()?;
    let mut req = client
        .get(COPILOT_TOKEN_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("token {}", github_token));
    for (k, v) in copilot_headers() {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Copilot token request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Copilot token request returned {}: {}", status, body));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid copilot token response: {}", e))?;

    let token = json
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Copilot token response missing token".to_string())?;
    let expires_at = json
        .get("expires_at")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Copilot token response missing expires_at".to_string())?;

    let base_url = base_url_from_token(token).unwrap_or_else(|| DEFAULT_API_BASE.to_string());

    Ok(CopilotCredential {
        github_token: github_token.to_string(),
        copilot_token: token.to_string(),
        // Refresh 5 minutes before actual expiry.
        expires_at_ms: expires_at.saturating_mul(1000).saturating_sub(5 * 60 * 1000),
        base_url,
    })
}

/// Parse `proxy-ep=proxy.xxx` from the copilot token and turn it into the api host.
/// Token format: `tid=...;exp=...;proxy-ep=proxy.individual.githubcopilot.com;...`
pub fn base_url_from_token(token: &str) -> Option<String> {
    let proxy = token
        .split(';')
        .find_map(|part| part.trim().strip_prefix("proxy-ep="))?;
    let api_host = proxy.strip_prefix("proxy.").map(|h| format!("api.{}", h)).unwrap_or_else(|| proxy.to_string());
    Some(format!("https://{}", api_host))
}

/// A model the user's Copilot subscription can use for chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotModel {
    pub id: String,
    pub name: String,
}

/// Fetch the list of chat-capable models available to this Copilot account.
/// Mirrors the VS Code Copilot `/models` endpoint and filters to models that
/// are picker-enabled, not policy-disabled, and support tool calls.
pub async fn list_available_models(
    copilot_token: &str,
    base_url: &str,
) -> Result<Vec<CopilotModel>, String> {
    let client = form_client()?;
    let mut req = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", copilot_token))
        .header("X-GitHub-Api-Version", "2026-06-01");
    for (k, v) in copilot_headers() {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Copilot models request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Copilot models request returned {}: {}", status, body));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid copilot models response: {}", e))?;

    Ok(parse_available_models(&json))
}

/// Pure parser for the Copilot `/models` response (unit-testable).
pub fn parse_available_models(json: &serde_json::Value) -> Vec<CopilotModel> {
    let data = match json.get("data").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    let mut models = Vec::new();
    for item in data {
        let id = match item.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        if !is_selectable_model(item) {
            continue;
        }
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();
        models.push(CopilotModel {
            id: id.to_string(),
            name,
        });
    }
    models
}

/// A model is offered to the user only if it's picker-enabled, not disabled by
/// org policy, and supports tool calls.
fn is_selectable_model(item: &serde_json::Value) -> bool {
    let picker_enabled = item
        .get("model_picker_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let policy_ok = item
        .get("policy")
        .and_then(|p| p.get("state"))
        .and_then(|s| s.as_str())
        .map(|s| s != "disabled")
        .unwrap_or(true);

    // supports.tool_calls defaults to allowed unless explicitly false.
    let tool_calls_ok = item
        .get("capabilities")
        .and_then(|c| c.get("supports"))
        .and_then(|s| s.get("tool_calls"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    picker_enabled && policy_ok && tool_calls_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_available_models_filters_correctly() {
        let json = serde_json::json!({
            "data": [
                {
                    "id": "gpt-4o",
                    "name": "GPT-4o",
                    "model_picker_enabled": true,
                    "policy": {"state": "enabled"},
                    "capabilities": {"supports": {"tool_calls": true}}
                },
                {
                    "id": "claude-sonnet-4",
                    "name": "Claude Sonnet 4",
                    "model_picker_enabled": true
                    // no policy / capabilities -> defaults allow it
                },
                {
                    "id": "disabled-by-policy",
                    "name": "Blocked",
                    "model_picker_enabled": true,
                    "policy": {"state": "disabled"}
                },
                {
                    "id": "not-picker",
                    "name": "Hidden",
                    "model_picker_enabled": false
                },
                {
                    "id": "no-tools",
                    "name": "No Tools",
                    "model_picker_enabled": true,
                    "capabilities": {"supports": {"tool_calls": false}}
                }
            ]
        });

        let models = parse_available_models(&json);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-4o", "claude-sonnet-4"]);
        assert_eq!(models[0].name, "GPT-4o");
    }

    #[test]
    fn parses_available_models_empty_when_no_data() {
        assert!(parse_available_models(&serde_json::json!({})).is_empty());
        assert!(parse_available_models(&serde_json::json!({"data": "nope"})).is_empty());
    }

    #[test]
    fn model_name_falls_back_to_id() {
        let json = serde_json::json!({
            "data": [{"id": "some-model", "model_picker_enabled": true}]
        });
        let models = parse_available_models(&json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "some-model");
    }

    #[test]
    fn parses_base_url_from_proxy_ep() {
        let token = "tid=abc;exp=123;proxy-ep=proxy.individual.githubcopilot.com;ssc=1";
        assert_eq!(
            base_url_from_token(token).as_deref(),
            Some("https://api.individual.githubcopilot.com")
        );
    }

    #[test]
    fn base_url_without_proxy_prefix_kept() {
        let token = "tid=abc;proxy-ep=gateway.example.com;x=1";
        assert_eq!(
            base_url_from_token(token).as_deref(),
            Some("https://gateway.example.com")
        );
    }

    #[test]
    fn base_url_none_when_no_proxy_ep() {
        assert_eq!(base_url_from_token("tid=abc;exp=123"), None);
    }

    #[test]
    fn credential_validity_respects_expiry() {
        let mut cred = CopilotCredential::default();
        assert!(!cred.is_copilot_token_valid()); // empty token
        cred.copilot_token = "t".to_string();
        cred.expires_at_ms = now_ms() + 60_000;
        assert!(cred.is_copilot_token_valid());
        cred.expires_at_ms = now_ms().saturating_sub(1);
        assert!(!cred.is_copilot_token_valid());
    }
}
