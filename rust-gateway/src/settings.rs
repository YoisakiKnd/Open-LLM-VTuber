use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, bail};
use atomic_write_file::AtomicWriteFile;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::{Config as TsConfig, TS};
use url::Url;

pub const SETTINGS_SCHEMA_VERSION: u16 = 1;
pub const MAX_SETTINGS_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum SettingOwner {
    Client,
    Desktop,
    Runtime,
    Character,
    Session,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum ApplyEffect {
    Preview,
    Live,
    Reconnect,
    Restart,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SettingFieldPolicy {
    path: &'static str,
    owner: SettingOwner,
    apply_effect: ApplyEffect,
    secret: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SettingsSchemaResponse {
    schema_version: u16,
    owners: [SettingOwner; 5],
    apply_effects: [ApplyEffect; 4],
    fields: Vec<SettingFieldPolicy>,
    schema: Value,
    patch_schema: Value,
    patch_response_schema: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct SettingsSnapshotV1 {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u16,
    #[schemars(range(max = 9007199254740991u64))]
    pub revision: u64,
    pub client: ClientSettingsV1,
    pub provider: ProviderSettingsV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct ClientSettingsV1 {
    pub appearance: AppearancePreferences,
    pub media: MediaPreferences,
    pub voice: VoicePreferences,
    pub behavior: BehaviorPreferences,
    pub live2d: Live2dPreferences,
    pub connection_override: Option<ConnectionOverride>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct AppearancePreferences {
    pub locale: String,
    pub background_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct MediaPreferences {
    #[schemars(range(min = 0.1, max = 1.0))]
    pub image_compression_quality: f64,
    #[schemars(range(min = 0))]
    pub image_max_width: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct VoicePreferences {
    pub auto_stop_mic: bool,
    pub auto_start_mic_on_ai_speech: bool,
    pub auto_start_mic_on_conversation_end: bool,
    pub vad: VadPreferences,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct VadPreferences {
    #[schemars(range(min = 0.0, max = 100.0))]
    pub positive_speech_threshold: f64,
    #[schemars(range(min = 0.0, max = 100.0))]
    pub negative_speech_threshold: f64,
    #[schemars(range(min = 1))]
    pub redemption_frames: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct BehaviorPreferences {
    pub proactive_speak: ProactiveSpeakPreferences,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct ProactiveSpeakPreferences {
    pub allow_button_trigger: bool,
    pub allow_proactive_speak: bool,
    #[schemars(range(min = 0.0))]
    pub idle_seconds_to_speak: f64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct Live2dPreferences {
    pub pointer_interactive: Option<bool>,
    pub scroll_to_resize: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct ConnectionOverride {
    pub ws_url: Option<String>,
    pub base_url: Option<String>,
}

/// Chat provider kinds supported by the native orchestrator. `none` disables
/// the native provider (proxy mode fallback).
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum ProviderKindSetting {
    #[default]
    None,
    OpenAi,
    Anthropic,
    Ollama,
}

/// Redacted representation of a stored secret. Plaintext never leaves the
/// gateway; clients only observe `configured` plus a masked hint.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SecretValue {
    pub configured: bool,
    /// Masked hint, e.g. `sk-...abcd`.
    pub hint: Option<String>,
}

/// Provider configuration as served to clients (secrets redacted).
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct ProviderSettingsV1 {
    pub kind: ProviderKindSetting,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: SecretValue,
}

/// Provider configuration as accepted in a PATCH. `api_key` semantics:
/// `None` keeps the stored value, `Some("")` clears it, `Some(plaintext)`
/// stores it.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct ProviderPatchV1 {
    pub kind: ProviderKindSetting,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(TS)]
pub struct SettingsPatchRequestV1 {
    #[schemars(range(max = 9007199254740991u64))]
    pub base_revision: u64,
    pub client: ClientSettingsV1,
    pub provider: ProviderPatchV1,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SettingsApplyResponse {
    pub snapshot: SettingsSnapshotV1,
    pub changed_paths: Vec<&'static str>,
    pub apply_effects: Vec<ApplyEffect>,
}

#[derive(Debug)]
pub enum SettingsApplyError {
    Conflict(Box<SettingsSnapshotV1>),
    Validation(Vec<SettingsValidationError>),
    RevisionExhausted,
    Storage(anyhow::Error),
}

pub struct SettingsRepository {
    path: PathBuf,
    secrets_path: PathBuf,
    current: Mutex<SettingsSnapshotV1>,
    secrets: Mutex<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SettingsValidationError {
    pub path: &'static str,
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(TS)]
pub struct SettingsValidationResponse {
    valid: bool,
    errors: Vec<SettingsValidationError>,
}

impl Default for SettingsSnapshotV1 {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            revision: 0,
            client: ClientSettingsV1 {
                appearance: AppearancePreferences {
                    locale: "en".to_owned(),
                    background_url: None,
                },
                media: MediaPreferences {
                    image_compression_quality: 0.8,
                    image_max_width: 0,
                },
                voice: VoicePreferences {
                    auto_stop_mic: false,
                    auto_start_mic_on_ai_speech: false,
                    auto_start_mic_on_conversation_end: false,
                    vad: VadPreferences {
                        positive_speech_threshold: 50.0,
                        negative_speech_threshold: 35.0,
                        redemption_frames: 35,
                    },
                },
                behavior: BehaviorPreferences {
                    proactive_speak: ProactiveSpeakPreferences {
                        allow_button_trigger: false,
                        allow_proactive_speak: false,
                        idle_seconds_to_speak: 5.0,
                    },
                },
                live2d: Live2dPreferences {
                    pointer_interactive: None,
                    scroll_to_resize: None,
                },
                connection_override: None,
            },
            provider: ProviderSettingsV1 {
                kind: ProviderKindSetting::None,
                base_url: None,
                model: None,
                api_key: SecretValue {
                    configured: false,
                    hint: None,
                },
            },
        }
    }
}

impl SettingsRepository {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let snapshot = if path.exists() {
            let contents = fs::read(&path)
                .with_context(|| format!("failed to read settings file {}", path.display()))?;
            let snapshot = serde_json::from_slice::<SettingsSnapshotV1>(&contents)
                .with_context(|| format!("failed to parse settings file {}", path.display()))?;
            let errors = snapshot.validate();
            if !errors.is_empty() {
                bail!(
                    "settings file {} failed validation: {}",
                    path.display(),
                    serde_json::to_string(&errors).expect("validation errors must serialize")
                );
            }
            snapshot
        } else {
            SettingsSnapshotV1::default()
        };
        let secrets_path = secrets_path_for(&path);
        let secrets = load_secrets(&secrets_path)?;
        Ok(Self {
            path,
            secrets_path,
            current: Mutex::new(snapshot),
            secrets: Mutex::new(secrets),
        })
    }

    pub fn snapshot(&self) -> SettingsSnapshotV1 {
        let current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let secrets = self
            .secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.clone().with_redacted_secrets(&secrets)
    }

    /// Plaintext secret for internal consumers (e.g. the chat orchestrator).
    /// Never serialized into snapshots or API responses.
    pub fn secret_plaintext(&self, key: &str) -> Option<String> {
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub fn apply(
        &self,
        request: SettingsPatchRequestV1,
    ) -> std::result::Result<SettingsApplyResponse, SettingsApplyError> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if request.base_revision != current.revision {
            // Build the conflict snapshot without re-locking `current` (the
            // lock is already held here); only the secret store is consulted.
            let secrets = self
                .secrets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return Err(SettingsApplyError::Conflict(Box::new(
                current.clone().with_redacted_secrets(&secrets),
            )));
        }

        // Resolve secret updates before validation so that `Some("")` clears
        // and `Some(plaintext)` stores; `None` keeps the stored value.
        let mut secrets = self
            .secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut secret_changed = false;
        if let Some(value) = &request.provider.api_key {
            if value.is_empty() {
                secret_changed = secrets.remove("provider.api_key").is_some();
            } else {
                secret_changed = secrets.insert("provider.api_key".to_owned(), value.clone())
                    != Some(value.clone());
            }
        }

        let mut candidate = SettingsSnapshotV1 {
            schema_version: SETTINGS_SCHEMA_VERSION,
            revision: current.revision,
            client: request.client,
            provider: ProviderSettingsV1 {
                kind: request.provider.kind,
                base_url: request.provider.base_url.clone(),
                model: request.provider.model.clone(),
                api_key: mask_secret(secrets.get("provider.api_key").map(String::as_str)),
            },
        };
        let errors = candidate.validate();
        if !errors.is_empty() {
            // Roll back secret mutations made above.
            if secret_changed {
                self.reload_secrets(&mut secrets);
            }
            return Err(SettingsApplyError::Validation(errors));
        }

        let (changed_paths, apply_effects) = changes_between(&current, &candidate);
        let secret_path_changed = secret_changed && !changed_paths.contains(&"provider.apiKey");
        if changed_paths.is_empty() && !secret_path_changed {
            return Ok(SettingsApplyResponse {
                snapshot: current.clone(),
                changed_paths,
                apply_effects,
            });
        }

        candidate.revision = current
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_SETTINGS_REVISION)
            .ok_or(SettingsApplyError::RevisionExhausted)?;
        if secret_changed {
            persist_secrets(&self.secrets_path, &secrets).map_err(SettingsApplyError::Storage)?;
        }
        persist_snapshot(&self.path, &candidate).map_err(SettingsApplyError::Storage)?;
        *current = candidate.clone();
        let mut changed = changed_paths;
        if secret_path_changed {
            changed.push("provider.apiKey");
        }
        Ok(SettingsApplyResponse {
            snapshot: candidate,
            changed_paths: changed,
            apply_effects,
        })
    }

    fn reload_secrets(&self, secrets: &mut HashMap<String, String>) {
        if let Ok(reloaded) = load_secrets(&self.secrets_path) {
            *secrets = reloaded;
        } else {
            secrets.clear();
        }
    }
}

impl SettingsSnapshotV1 {
    pub fn validate(&self) -> Vec<SettingsValidationError> {
        let mut errors = Vec::new();
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            errors.push(validation_error(
                "schemaVersion",
                "unsupported_schema_version",
                "schemaVersion must match the supported settings schema version",
            ));
        }
        if self.revision > MAX_SETTINGS_REVISION {
            errors.push(validation_error(
                "revision",
                "out_of_range",
                "revision must be a JavaScript safe integer",
            ));
        }
        if self.client.appearance.locale.trim().is_empty() {
            errors.push(validation_error(
                "client.appearance.locale",
                "required",
                "locale must not be empty",
            ));
        }
        validate_finite_range(
            &mut errors,
            "client.media.imageCompressionQuality",
            self.client.media.image_compression_quality,
            0.1,
            1.0,
        );
        validate_finite_range(
            &mut errors,
            "client.voice.vad.positiveSpeechThreshold",
            self.client.voice.vad.positive_speech_threshold,
            0.0,
            100.0,
        );
        validate_finite_range(
            &mut errors,
            "client.voice.vad.negativeSpeechThreshold",
            self.client.voice.vad.negative_speech_threshold,
            0.0,
            100.0,
        );
        if self.client.voice.vad.redemption_frames == 0 {
            errors.push(validation_error(
                "client.voice.vad.redemptionFrames",
                "out_of_range",
                "redemptionFrames must be greater than zero",
            ));
        }
        if let Some(base_url) = &self.provider.base_url {
            validate_optional_url(
                &mut errors,
                "provider.baseUrl",
                Some(base_url.as_str()),
                &["http", "https"],
            );
        }
        let idle_seconds = self.client.behavior.proactive_speak.idle_seconds_to_speak;
        if !idle_seconds.is_finite() || idle_seconds <= 0.0 {
            errors.push(validation_error(
                "client.behavior.proactiveSpeak.idleSecondsToSpeak",
                "out_of_range",
                "idleSecondsToSpeak must be finite and greater than zero",
            ));
        }
        if let Some(connection) = &self.client.connection_override {
            if connection.ws_url.is_none() && connection.base_url.is_none() {
                errors.push(validation_error(
                    "client.connectionOverride",
                    "empty_override",
                    "connectionOverride must contain wsUrl or baseUrl",
                ));
            }
            validate_optional_url(
                &mut errors,
                "client.connectionOverride.wsUrl",
                connection.ws_url.as_deref(),
                &["ws", "wss"],
            );
            validate_optional_url(
                &mut errors,
                "client.connectionOverride.baseUrl",
                connection.base_url.as_deref(),
                &["http", "https"],
            );
        }
        errors
    }
}

pub fn schema_response() -> SettingsSchemaResponse {
    SettingsSchemaResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        owners: [
            SettingOwner::Client,
            SettingOwner::Desktop,
            SettingOwner::Runtime,
            SettingOwner::Character,
            SettingOwner::Session,
        ],
        apply_effects: [
            ApplyEffect::Preview,
            ApplyEffect::Live,
            ApplyEffect::Reconnect,
            ApplyEffect::Restart,
        ],
        fields: field_policies(),
        schema: serde_json::to_value(schema_for!(SettingsSnapshotV1))
            .expect("settings schema must serialize"),
        patch_schema: serde_json::to_value(schema_for!(SettingsPatchRequestV1))
            .expect("settings patch schema must serialize"),
        patch_response_schema: serde_json::to_value(schema_for!(SettingsApplyResponse))
            .expect("settings patch response schema must serialize"),
    }
}

pub fn validation_response(snapshot: &SettingsSnapshotV1) -> SettingsValidationResponse {
    let errors = snapshot.validate();
    SettingsValidationResponse {
        valid: errors.is_empty(),
        errors,
    }
}

pub fn typescript_bindings() -> String {
    let config = TsConfig::default().with_large_int("number");
    let declarations = [
        SettingOwner::decl(&config),
        ApplyEffect::decl(&config),
        SettingFieldPolicy::decl(&config),
        SettingsSchemaResponse::decl(&config),
        AppearancePreferences::decl(&config),
        MediaPreferences::decl(&config),
        VadPreferences::decl(&config),
        VoicePreferences::decl(&config),
        ProactiveSpeakPreferences::decl(&config),
        BehaviorPreferences::decl(&config),
        Live2dPreferences::decl(&config),
        ConnectionOverride::decl(&config),
        ProviderKindSetting::decl(&config),
        SecretValue::decl(&config),
        ProviderSettingsV1::decl(&config),
        ProviderPatchV1::decl(&config),
        ClientSettingsV1::decl(&config),
        SettingsSnapshotV1::decl(&config),
        SettingsPatchRequestV1::decl(&config),
        SettingsApplyResponse::decl(&config),
        SettingsValidationError::decl(&config),
        SettingsValidationResponse::decl(&config),
    ];
    format!(
        "// Generated from rust-gateway/src/settings.rs. Do not edit.\n\nexport const SETTINGS_SCHEMA_VERSION = {SETTINGS_SCHEMA_VERSION} as const;\nexport const MAX_SETTINGS_REVISION = {MAX_SETTINGS_REVISION} as const;\n\nexport type JsonValue = null | boolean | number | string | Array<JsonValue> | {{ [key: string]: JsonValue }};\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

pub fn export_typescript_bindings(path: &Path) -> Result<()> {
    atomic_write(path, typescript_bindings().as_bytes())
        .with_context(|| format!("failed to export settings bindings to {}", path.display()))
}

const SECRETS_SCHEMA_VERSION: u16 = 1;

/// Plaintext secret store persisted next to the settings file. This file is
/// never served: snapshots only expose the redacted [`SecretValue`].
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretsFileV1 {
    schema_version: u16,
    secrets: HashMap<String, String>,
}

impl SettingsSnapshotV1 {
    /// Replaces secret placeholders with the redacted view of stored secrets.
    fn with_redacted_secrets(mut self, secrets: &HashMap<String, String>) -> Self {
        self.provider.api_key = mask_secret(secrets.get("provider.api_key").map(String::as_str));
        self
    }
}

fn secrets_path_for(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("secrets.v1.json")
}

fn load_secrets(path: &Path) -> Result<HashMap<String, String>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let contents = fs::read(path)
        .with_context(|| format!("failed to read secrets file {}", path.display()))?;
    let file: SecretsFileV1 = serde_json::from_slice(&contents)
        .with_context(|| format!("failed to parse secrets file {}", path.display()))?;
    Ok(file.secrets)
}

fn persist_secrets(path: &Path, secrets: &HashMap<String, String>) -> Result<()> {
    let file = SecretsFileV1 {
        schema_version: SECRETS_SCHEMA_VERSION,
        secrets: secrets.clone(),
    };
    let mut contents = serde_json::to_vec_pretty(&file).context("failed to serialize secrets")?;
    contents.push(b'\n');
    atomic_write(path, &contents)
}

/// Redacts a stored secret into a `{configured, hint}` view for clients.
fn mask_secret(value: Option<&str>) -> SecretValue {
    match value {
        Some(value) if !value.is_empty() => {
            let hint = if value.chars().count() > 8 {
                let prefix: String = value.chars().take(4).collect();
                let suffix: String = value
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                format!("{prefix}…{suffix}")
            } else {
                "…".to_owned()
            };
            SecretValue {
                configured: true,
                hint: Some(hint),
            }
        }
        _ => SecretValue {
            configured: false,
            hint: None,
        },
    }
}

fn persist_snapshot(path: &Path, snapshot: &SettingsSnapshotV1) -> Result<()> {
    let mut contents =
        serde_json::to_vec_pretty(snapshot).context("failed to serialize settings")?;
    contents.push(b'\n');
    atomic_write(path, &contents)
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let mut file = AtomicWriteFile::open(path)
        .with_context(|| format!("failed to open file {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write file {}", path.display()))?;
    file.commit()
        .with_context(|| format!("failed to commit file {}", path.display()))
}

fn changes_between(
    before: &SettingsSnapshotV1,
    after: &SettingsSnapshotV1,
) -> (Vec<&'static str>, Vec<ApplyEffect>) {
    let mut paths = Vec::new();
    let mut effects = Vec::new();
    record_change(
        &mut paths,
        &mut effects,
        "client.appearance.locale",
        ApplyEffect::Live,
        &before.client.appearance.locale,
        &after.client.appearance.locale,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.appearance.backgroundUrl",
        ApplyEffect::Preview,
        &before.client.appearance.background_url,
        &after.client.appearance.background_url,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.media.imageCompressionQuality",
        ApplyEffect::Live,
        &before.client.media.image_compression_quality,
        &after.client.media.image_compression_quality,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.media.imageMaxWidth",
        ApplyEffect::Live,
        &before.client.media.image_max_width,
        &after.client.media.image_max_width,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.autoStopMic",
        ApplyEffect::Live,
        &before.client.voice.auto_stop_mic,
        &after.client.voice.auto_stop_mic,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.autoStartMicOnAiSpeech",
        ApplyEffect::Live,
        &before.client.voice.auto_start_mic_on_ai_speech,
        &after.client.voice.auto_start_mic_on_ai_speech,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.autoStartMicOnConversationEnd",
        ApplyEffect::Live,
        &before.client.voice.auto_start_mic_on_conversation_end,
        &after.client.voice.auto_start_mic_on_conversation_end,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.vad.positiveSpeechThreshold",
        ApplyEffect::Live,
        &before.client.voice.vad.positive_speech_threshold,
        &after.client.voice.vad.positive_speech_threshold,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.vad.negativeSpeechThreshold",
        ApplyEffect::Live,
        &before.client.voice.vad.negative_speech_threshold,
        &after.client.voice.vad.negative_speech_threshold,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.voice.vad.redemptionFrames",
        ApplyEffect::Live,
        &before.client.voice.vad.redemption_frames,
        &after.client.voice.vad.redemption_frames,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.behavior.proactiveSpeak.allowButtonTrigger",
        ApplyEffect::Live,
        &before.client.behavior.proactive_speak.allow_button_trigger,
        &after.client.behavior.proactive_speak.allow_button_trigger,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.behavior.proactiveSpeak.allowProactiveSpeak",
        ApplyEffect::Live,
        &before.client.behavior.proactive_speak.allow_proactive_speak,
        &after.client.behavior.proactive_speak.allow_proactive_speak,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.behavior.proactiveSpeak.idleSecondsToSpeak",
        ApplyEffect::Live,
        &before.client.behavior.proactive_speak.idle_seconds_to_speak,
        &after.client.behavior.proactive_speak.idle_seconds_to_speak,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.live2d.pointerInteractive",
        ApplyEffect::Preview,
        &before.client.live2d.pointer_interactive,
        &after.client.live2d.pointer_interactive,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.live2d.scrollToResize",
        ApplyEffect::Preview,
        &before.client.live2d.scroll_to_resize,
        &after.client.live2d.scroll_to_resize,
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.connectionOverride.wsUrl",
        ApplyEffect::Reconnect,
        &before
            .client
            .connection_override
            .as_ref()
            .and_then(|connection| connection.ws_url.as_ref()),
        &after
            .client
            .connection_override
            .as_ref()
            .and_then(|connection| connection.ws_url.as_ref()),
    );
    record_change(
        &mut paths,
        &mut effects,
        "client.connectionOverride.baseUrl",
        ApplyEffect::Reconnect,
        &before
            .client
            .connection_override
            .as_ref()
            .and_then(|connection| connection.base_url.as_ref()),
        &after
            .client
            .connection_override
            .as_ref()
            .and_then(|connection| connection.base_url.as_ref()),
    );
    record_change(
        &mut paths,
        &mut effects,
        "provider.kind",
        ApplyEffect::Restart,
        &before.provider.kind,
        &after.provider.kind,
    );
    record_change(
        &mut paths,
        &mut effects,
        "provider.baseUrl",
        ApplyEffect::Restart,
        &before.provider.base_url,
        &after.provider.base_url,
    );
    record_change(
        &mut paths,
        &mut effects,
        "provider.model",
        ApplyEffect::Restart,
        &before.provider.model,
        &after.provider.model,
    );
    record_change(
        &mut paths,
        &mut effects,
        "provider.apiKey",
        ApplyEffect::Restart,
        &before.provider.api_key,
        &after.provider.api_key,
    );
    effects.sort_by_key(|effect| match effect {
        ApplyEffect::Preview => 0,
        ApplyEffect::Live => 1,
        ApplyEffect::Reconnect => 2,
        ApplyEffect::Restart => 3,
    });
    (paths, effects)
}

fn record_change<T: PartialEq>(
    paths: &mut Vec<&'static str>,
    effects: &mut Vec<ApplyEffect>,
    path: &'static str,
    effect: ApplyEffect,
    before: &T,
    after: &T,
) {
    if before != after {
        paths.push(path);
        if !effects.contains(&effect) {
            effects.push(effect);
        }
    }
}

fn field_policies() -> Vec<SettingFieldPolicy> {
    vec![
        field("client.appearance.locale", ApplyEffect::Live),
        field("client.appearance.backgroundUrl", ApplyEffect::Preview),
        field("client.media.imageCompressionQuality", ApplyEffect::Live),
        field("client.media.imageMaxWidth", ApplyEffect::Live),
        field("client.voice.autoStopMic", ApplyEffect::Live),
        field("client.voice.autoStartMicOnAiSpeech", ApplyEffect::Live),
        field(
            "client.voice.autoStartMicOnConversationEnd",
            ApplyEffect::Live,
        ),
        field(
            "client.voice.vad.positiveSpeechThreshold",
            ApplyEffect::Live,
        ),
        field(
            "client.voice.vad.negativeSpeechThreshold",
            ApplyEffect::Live,
        ),
        field("client.voice.vad.redemptionFrames", ApplyEffect::Live),
        field(
            "client.behavior.proactiveSpeak.allowButtonTrigger",
            ApplyEffect::Live,
        ),
        field(
            "client.behavior.proactiveSpeak.allowProactiveSpeak",
            ApplyEffect::Live,
        ),
        field(
            "client.behavior.proactiveSpeak.idleSecondsToSpeak",
            ApplyEffect::Live,
        ),
        field("client.live2d.pointerInteractive", ApplyEffect::Preview),
        field("client.live2d.scrollToResize", ApplyEffect::Preview),
        field("client.connectionOverride.wsUrl", ApplyEffect::Reconnect),
        field("client.connectionOverride.baseUrl", ApplyEffect::Reconnect),
        runtime_field("provider.kind", ApplyEffect::Restart, false),
        runtime_field("provider.baseUrl", ApplyEffect::Restart, false),
        runtime_field("provider.model", ApplyEffect::Restart, false),
        runtime_field("provider.apiKey", ApplyEffect::Restart, true),
    ]
}

fn runtime_field(
    path: &'static str,
    apply_effect: ApplyEffect,
    secret: bool,
) -> SettingFieldPolicy {
    SettingFieldPolicy {
        path,
        owner: SettingOwner::Runtime,
        apply_effect,
        secret,
    }
}

fn field(path: &'static str, apply_effect: ApplyEffect) -> SettingFieldPolicy {
    SettingFieldPolicy {
        path,
        owner: SettingOwner::Client,
        apply_effect,
        secret: false,
    }
}

fn validation_error(
    path: &'static str,
    code: &'static str,
    message: &'static str,
) -> SettingsValidationError {
    SettingsValidationError {
        path,
        code,
        message,
    }
}

fn validate_finite_range(
    errors: &mut Vec<SettingsValidationError>,
    path: &'static str,
    value: f64,
    minimum: f64,
    maximum: f64,
) {
    if !value.is_finite() || value < minimum || value > maximum {
        errors.push(validation_error(
            path,
            "out_of_range",
            "value must be finite and within the supported range",
        ));
    }
}

fn validate_optional_url(
    errors: &mut Vec<SettingsValidationError>,
    path: &'static str,
    value: Option<&str>,
    allowed_schemes: &[&str],
) {
    let Some(value) = value else {
        return;
    };
    let valid = !value.trim().is_empty()
        && Url::parse(value).is_ok_and(|url| allowed_schemes.contains(&url.scheme()));
    if !valid {
        errors.push(validation_error(
            path,
            "invalid_url",
            "URL must be absolute and use a supported scheme",
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn provider_patch() -> ProviderPatchV1 {
        ProviderPatchV1 {
            kind: ProviderKindSetting::None,
            base_url: None,
            model: None,
            api_key: None,
        }
    }

    #[test]
    fn unsupported_schema_version_is_rejected_with_actionable_error() {
        let snapshot = SettingsSnapshotV1 {
            schema_version: 99,
            ..SettingsSnapshotV1::default()
        };
        let errors = snapshot.validate();
        assert!(errors.iter().any(|error| {
            error.path == "schemaVersion" && error.code == "unsupported_schema_version"
        }));
    }

    #[test]
    fn provider_secret_is_redacted_in_snapshots_and_persisted_plaintext_only() {
        let directory =
            std::env::temp_dir().join(format!("olv-provider-secret-{}", uuid::Uuid::new_v4()));
        let settings_file = directory.join("settings.v1.json");
        let repository = SettingsRepository::load(&settings_file).unwrap();
        assert!(!repository.snapshot().provider.api_key.configured);

        // Store a secret: the snapshot only exposes a mask.
        let client = repository.snapshot().client;
        let applied = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client: client.clone(),
                provider: ProviderPatchV1 {
                    kind: ProviderKindSetting::OpenAi,
                    base_url: Some("https://api.example.com/v1".to_owned()),
                    model: Some("gpt-test".to_owned()),
                    api_key: Some("sk-super-secret-value-1234".to_owned()),
                },
            })
            .unwrap();
        let api_key = &applied.snapshot.provider.api_key;
        assert!(api_key.configured);
        assert_eq!(api_key.hint.as_deref(), Some("sk-s…1234"));
        assert_eq!(applied.snapshot.provider.kind, ProviderKindSetting::OpenAi);
        assert!(applied.changed_paths.contains(&"provider.apiKey"));
        assert!(applied.changed_paths.contains(&"provider.kind"));

        // The persisted settings file must never contain the plaintext.
        let file_contents = fs::read_to_string(&settings_file).unwrap();
        assert!(!file_contents.contains("sk-super-secret-value-1234"));
        assert!(file_contents.contains("sk-s…1234"));

        // The secrets file holds the plaintext, and a reload keeps it.
        let reloaded = SettingsRepository::load(&settings_file).unwrap();
        assert_eq!(
            reloaded.secret_plaintext("provider.api_key").as_deref(),
            Some("sk-super-secret-value-1234")
        );
        assert!(reloaded.snapshot().provider.api_key.configured);

        // `Some("")` clears the secret.
        let cleared = reloaded
            .apply(SettingsPatchRequestV1 {
                base_revision: 1,
                client: client.clone(),
                provider: ProviderPatchV1 {
                    kind: ProviderKindSetting::OpenAi,
                    base_url: None,
                    model: None,
                    api_key: Some(String::new()),
                },
            })
            .unwrap();
        assert!(!cleared.snapshot.provider.api_key.configured);
        assert!(reloaded.secret_plaintext("provider.api_key").is_none());

        // `None` keeps the stored secret.
        reloaded
            .apply(SettingsPatchRequestV1 {
                base_revision: 2,
                client,
                provider: ProviderPatchV1 {
                    kind: ProviderKindSetting::OpenAi,
                    base_url: None,
                    model: None,
                    api_key: None,
                },
            })
            .unwrap();
        assert!(reloaded.secret_plaintext("provider.api_key").is_none());
    }

    #[test]
    fn provider_base_url_validation_rejects_bad_schemes() {
        let directory =
            std::env::temp_dir().join(format!("olv-provider-url-{}", uuid::Uuid::new_v4()));
        let settings_file = directory.join("settings.v1.json");
        let repository = SettingsRepository::load(&settings_file).unwrap();
        let client = repository.snapshot().client;
        let error = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client,
                provider: ProviderPatchV1 {
                    kind: ProviderKindSetting::Ollama,
                    base_url: Some("ftp://nope".to_owned()),
                    model: None,
                    api_key: None,
                },
            })
            .unwrap_err();
        assert!(
            matches!(error, SettingsApplyError::Validation(errors) if errors.iter().any(|e| e.path == "provider.baseUrl"))
        );
    }

    #[test]
    fn generated_typescript_bindings_are_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("apps/desktop/src/renderer/src/settings/generated/settings-v1.generated.ts");
        let actual = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read generated settings bindings {}: {error}",
                path.display()
            )
        });
        assert_eq!(
            actual,
            typescript_bindings(),
            "regenerate with `cargo run --manifest-path rust-gateway/Cargo.toml -- --export-settings-types apps/desktop/src/renderer/src/settings/generated/settings-v1.generated.ts`"
        );
    }

    #[test]
    fn default_snapshot_is_valid_and_excludes_session_state() {
        let snapshot = SettingsSnapshotV1::default();
        assert!(snapshot.validate().is_empty());

        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["schemaVersion"], SETTINGS_SCHEMA_VERSION);
        assert_eq!(value["revision"], 0);
        assert!(value.get("micOn").is_none());
        assert!(value["client"].get("legacy").is_none());
    }

    #[test]
    fn validation_reports_all_invalid_client_values() {
        let mut snapshot = SettingsSnapshotV1 {
            schema_version: 2,
            revision: MAX_SETTINGS_REVISION + 1,
            ..SettingsSnapshotV1::default()
        };
        snapshot.client.appearance.locale = "  ".to_owned();
        snapshot.client.media.image_compression_quality = f64::NAN;
        snapshot.client.voice.vad.positive_speech_threshold = 101.0;
        snapshot.client.voice.vad.negative_speech_threshold = -1.0;
        snapshot.client.voice.vad.redemption_frames = 0;
        snapshot
            .client
            .behavior
            .proactive_speak
            .idle_seconds_to_speak = 0.0;
        snapshot.client.connection_override = Some(ConnectionOverride {
            ws_url: Some("https://wrong.example/client-ws".to_owned()),
            base_url: Some("not a URL".to_owned()),
        });

        let paths = snapshot
            .validate()
            .into_iter()
            .map(|error| error.path)
            .collect::<HashSet<_>>();
        assert_eq!(paths.len(), 10);
        assert!(paths.contains("schemaVersion"));
        assert!(paths.contains("revision"));
        assert!(paths.contains("client.appearance.locale"));
        assert!(paths.contains("client.media.imageCompressionQuality"));
        assert!(paths.contains("client.voice.vad.positiveSpeechThreshold"));
        assert!(paths.contains("client.voice.vad.negativeSpeechThreshold"));
        assert!(paths.contains("client.voice.vad.redemptionFrames"));
        assert!(paths.contains("client.behavior.proactiveSpeak.idleSecondsToSpeak"));
        assert!(paths.contains("client.connectionOverride.wsUrl"));
        assert!(paths.contains("client.connectionOverride.baseUrl"));
    }

    #[test]
    fn schema_catalog_has_unique_paths_and_declares_constraints() {
        let response = serde_json::to_value(schema_response()).unwrap();
        let fields = response["fields"].as_array().unwrap();
        let paths = fields
            .iter()
            .map(|field| field["path"].as_str().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(fields.len(), 21);
        assert_eq!(paths.len(), fields.len());
        assert_eq!(response["schemaVersion"], SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            response["owners"],
            serde_json::json!(["client", "desktop", "runtime", "character", "session"])
        );
        assert_eq!(
            response["applyEffects"],
            serde_json::json!(["preview", "live", "reconnect", "restart"])
        );
        let schema = response["schema"].to_string();
        assert!(schema.contains("imageCompressionQuality"));
        assert!(schema.contains("maximum"));
        assert!(schema.contains("minimum"));
        assert_eq!(response["patchSchema"]["title"], "SettingsPatchRequestV1");
        assert_eq!(
            response["patchResponseSchema"]["title"],
            "SettingsApplyResponse"
        );
    }

    #[test]
    fn repository_persists_changes_and_enforces_revision_conflicts() {
        let directory =
            std::env::temp_dir().join(format!("olv-settings-repository-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.v1.json");
        let repository = SettingsRepository::load(&path).unwrap();
        assert_eq!(repository.snapshot().revision, 0);
        assert!(!path.exists());

        let mut client = repository.snapshot().client;
        client.appearance.background_url = Some("https://example.com/bg.png".to_owned());
        client.connection_override = Some(ConnectionOverride {
            ws_url: Some("wss://example.com/client-ws".to_owned()),
            base_url: None,
        });
        let applied = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client: client.clone(),
                provider: provider_patch(),
            })
            .unwrap();
        assert_eq!(applied.snapshot.revision, 1);
        assert_eq!(
            applied.changed_paths,
            vec![
                "client.appearance.backgroundUrl",
                "client.connectionOverride.wsUrl"
            ]
        );
        assert_eq!(
            applied.apply_effects,
            vec![ApplyEffect::Preview, ApplyEffect::Reconnect]
        );

        let reloaded = SettingsRepository::load(&path).unwrap();
        assert_eq!(reloaded.snapshot(), applied.snapshot);
        let conflict = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client: ClientSettingsV1 {
                    appearance: AppearancePreferences {
                        locale: "zh".to_owned(),
                        ..client.appearance.clone()
                    },
                    ..client.clone()
                },
                provider: provider_patch(),
            })
            .unwrap_err();
        assert!(matches!(
            conflict,
            SettingsApplyError::Conflict(snapshot) if snapshot.revision == 1
        ));

        let no_op = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 1,
                client,
                provider: provider_patch(),
            })
            .unwrap();
        assert_eq!(no_op.snapshot.revision, 1);
        assert!(no_op.changed_paths.is_empty());
        assert!(no_op.apply_effects.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_rejects_invalid_data_without_writing() {
        let directory =
            std::env::temp_dir().join(format!("olv-settings-validation-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.v1.json");
        let repository = SettingsRepository::load(&path).unwrap();
        let mut client = repository.snapshot().client;
        client.media.image_compression_quality = 2.0;

        let error = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client,
                provider: provider_patch(),
            })
            .unwrap_err();
        assert!(matches!(error, SettingsApplyError::Validation(_)));
        assert_eq!(repository.snapshot(), SettingsSnapshotV1::default());
        assert!(!path.exists());
    }

    #[test]
    fn repository_updates_memory_only_after_atomic_commit() {
        let directory =
            std::env::temp_dir().join(format!("olv-settings-atomic-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let blocked_parent = directory.join("not-a-directory");
        fs::write(&blocked_parent, b"blocker").unwrap();
        let repository = SettingsRepository::load(blocked_parent.join("settings.json")).unwrap();
        let mut client = repository.snapshot().client;
        client.appearance.locale = "zh".to_owned();

        let error = repository
            .apply(SettingsPatchRequestV1 {
                base_revision: 0,
                client,
                provider: provider_patch(),
            })
            .unwrap_err();
        assert!(matches!(error, SettingsApplyError::Storage(_)));
        assert_eq!(repository.snapshot(), SettingsSnapshotV1::default());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_refuses_malformed_or_future_files() {
        let directory =
            std::env::temp_dir().join(format!("olv-settings-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let malformed_path = directory.join("malformed.json");
        fs::write(&malformed_path, b"not json").unwrap();
        assert!(SettingsRepository::load(&malformed_path).is_err());

        let future_path = directory.join("future.json");
        let future = SettingsSnapshotV1 {
            schema_version: SETTINGS_SCHEMA_VERSION + 1,
            ..SettingsSnapshotV1::default()
        };
        fs::write(&future_path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(SettingsRepository::load(&future_path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
