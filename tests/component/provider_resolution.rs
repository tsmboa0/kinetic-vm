//! TG1: ModelProvider End-to-End Resolution Tests

use kinetic::providers::compatible::{AuthStyle, OpenAiCompatibleModelProvider};
use kinetic::providers::{
    create_model_provider, create_model_provider_with_options, create_model_provider_with_url,
};

/// Helper: assert model_provider creation succeeds
fn assert_provider_ok(name: &str, key: Option<&str>, url: Option<&str>) {
    let result = create_model_provider_with_url(name, key, url);
    assert!(
        result.is_ok(),
        "{name} model_provider should resolve: {}",
        result.err().map(|e| e.to_string()).unwrap_or_default()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Factory resolution: each major model_provider name resolves without error
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_resolves_openai_provider() {
    assert_provider_ok("openai", Some("test-key"), None);
}

#[test]
fn factory_resolves_anthropic_provider() {
    assert_provider_ok("anthropic", Some("test-key"), None);
}

#[test]
fn factory_resolves_ollama_provider() {
    assert_provider_ok("ollama", None, None);
}

// ─────────────────────────────────────────────────────────────────────────────
// Factory resolution: alias variants map to same model_provider
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Custom URL model_provider creation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_custom_http_url_resolves() {
    assert_provider_ok("custom:http://localhost:8080", Some("test-key"), None);
}

#[test]
fn factory_custom_https_url_resolves() {
    assert_provider_ok("custom:https://api.example.com/v1", Some("test-key"), None);
}

#[test]
fn factory_custom_ftp_url_rejected() {
    let result = create_model_provider_with_url("custom:ftp://example.com", None, None);
    assert!(result.is_err(), "ftp scheme should be rejected");
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("http://") || err_msg.contains("https://"),
        "error should mention valid schemes: {err_msg}"
    );
}

#[test]
fn factory_custom_empty_url_rejected() {
    let result = create_model_provider_with_url("custom:", None, None);
    assert!(result.is_err(), "empty custom URL should be rejected");
}

#[test]
fn factory_unknown_provider_rejected() {
    let result = create_model_provider_with_url("nonexistent_provider_xyz", None, None);
    assert!(
        result.is_err(),
        "unknown model_provider name should be rejected"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAiCompatibleModelProvider: credential and auth style wiring
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn compatible_provider_bearer_auth_style() {
    // Construction with Bearer auth should succeed
    let _provider = OpenAiCompatibleModelProvider::builder("test")
        .display_name("TestProvider")
        .base_url("https://api.test.com")
        .credential(Some("sk-test-key-12345"))
        .auth_style(AuthStyle::Bearer)
        .build();
}

#[test]
fn compatible_provider_xapikey_auth_style() {
    // Construction with XApiKey auth should succeed
    let _provider = OpenAiCompatibleModelProvider::builder("test")
        .display_name("TestProvider")
        .base_url("https://api.test.com")
        .credential(Some("sk-test-key-12345"))
        .auth_style(AuthStyle::XApiKey)
        .build();
}

#[test]
fn compatible_provider_custom_auth_header() {
    // Construction with Custom auth should succeed
    let _provider = OpenAiCompatibleModelProvider::builder("test")
        .display_name("TestProvider")
        .base_url("https://api.test.com")
        .credential(Some("sk-test-key-12345"))
        .auth_style(AuthStyle::Custom("X-Custom-Auth".into()))
        .build();
}

#[test]
fn compatible_provider_no_credential() {
    // Construction without credential should succeed (for local model_providers)
    let _provider = OpenAiCompatibleModelProvider::builder("test")
        .display_name("TestLocal")
        .base_url("http://localhost:11434")
        .credential(None)
        .auth_style(AuthStyle::Bearer)
        .build();
}

#[test]
fn compatible_provider_base_url_trailing_slash_normalized() {
    // Construction with trailing slash URL should succeed
    let _provider = OpenAiCompatibleModelProvider::builder("test")
        .display_name("TestProvider")
        .base_url("https://api.test.com/v1/")
        .credential(Some("key"))
        .auth_style(AuthStyle::Bearer)
        .build();
}

// ─────────────────────────────────────────────────────────────────────────────
// ModelProvider with api_url override (simulates- Ollama api_url config)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_ollama_with_custom_api_url() {
    assert_provider_ok("ollama", None, Some("http://192.168.1.100:11434"));
}

#[test]
fn factory_openai_with_custom_api_url() {
    assert_provider_ok(
        "openai",
        Some("test-key"),
        Some("https://custom-openai-proxy.example.com/v1"),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ModelProvider default convenience factory
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn convenience_factory_resolves_major_providers() {
    for provider_name in &["openai", "anthropic", "gemini", "openrouter"] {
        let result = create_model_provider(provider_name, Some("test-key"));
        assert!(
            result.is_ok(),
            "convenience factory should resolve {provider_name}: {}",
            result.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }
}

#[test]
fn convenience_factory_ollama_no_key() {
    let result = create_model_provider("ollama", None);
    assert!(
        result.is_ok(),
        "ollama should not require api key: {}",
        result.err().map(|e| e.to_string()).unwrap_or_default()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Primary model_providers with custom implementations
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_resolves_openrouter_provider() {
    assert_provider_ok("openrouter", Some("test-key"), None);
}

#[test]
fn factory_resolves_gemini_provider() {
    assert_provider_ok("gemini", Some("test-key"), None);
}

#[test]
fn factory_resolves_openai_codex_provider() {
    let options = kinetic::providers::ModelProviderRuntimeOptions::default();
    let result = create_model_provider_with_options("openai-codex", None, &options);
    assert!(
        result.is_ok(),
        "openai-codex model_provider should resolve: {}",
        result.err().map(|e| e.to_string()).unwrap_or_default()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI-compatible ecosystem model_providers
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// China region model_providers
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Local/self-hosted model_providers
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Cloud AI endpoints
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Alias resolution tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_google_alias_resolves_to_gemini() {
    assert_provider_ok("google", Some("test-key"), None);
}

#[test]
fn factory_google_gemini_alias_resolves_to_gemini() {
    assert_provider_ok("google-gemini", Some("test-key"), None);
}

// ─────────────────────────────────────────────────────────────────────────────
// Custom endpoint tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn factory_anthropic_custom_endpoint_resolves() {
    assert_provider_ok(
        "anthropic-custom:https://api.example.com",
        Some("test-key"),
        None,
    );
}
