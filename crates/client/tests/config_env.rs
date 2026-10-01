//! Reading connection settings from the environment.
//!
//! The variables are process-global, so these tests take a lock to keep from
//! stepping on each other.

use std::sync::Mutex;

use srud_client::openai::{ClientConfig, DEFAULT_MODEL};
use srud_core::client::ModelError;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Clears the settings variables so each test starts from a known state.
fn clear() {
    for name in [
        ClientConfig::API_KEY_VAR,
        ClientConfig::BASE_URL_VAR,
        ClientConfig::MODEL_VAR,
    ] {
        std::env::remove_var(name);
    }
}

#[test]
fn settings_are_read_from_the_environment() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    clear();
    std::env::set_var(ClientConfig::API_KEY_VAR, "sk-test");
    std::env::set_var(ClientConfig::BASE_URL_VAR, "https://example.test/v1");
    std::env::set_var(ClientConfig::MODEL_VAR, "some-model");

    let settings = ClientConfig::from_env().expect("the environment is complete");

    assert_eq!(settings.api_key, "sk-test");
    assert_eq!(
        settings.base_url.as_deref(),
        Some("https://example.test/v1")
    );
    assert_eq!(settings.model, "some-model");
    clear();
}

#[test]
fn a_missing_api_key_is_a_configuration_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    clear();

    let error = ClientConfig::from_env().expect_err("the key is required");

    assert!(matches!(error, ModelError::Config(message) if message.contains("SRUD_API_KEY")));
    clear();
}

#[test]
fn the_model_falls_back_to_the_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    clear();
    std::env::set_var(ClientConfig::API_KEY_VAR, "sk-test");

    let settings = ClientConfig::from_env().expect("only the key is required");

    assert_eq!(settings.model, DEFAULT_MODEL);
    assert_eq!(settings.base_url, None);
    clear();
}

#[test]
fn blank_values_count_as_unset() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    clear();
    std::env::set_var(ClientConfig::API_KEY_VAR, "   ");
    std::env::set_var(ClientConfig::MODEL_VAR, "");

    let error = ClientConfig::from_env().expect_err("a blank key is no key");

    assert!(matches!(error, ModelError::Config(_)));
    clear();
}
