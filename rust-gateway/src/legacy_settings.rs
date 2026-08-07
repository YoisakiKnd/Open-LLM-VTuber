use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Value, json};

pub const LEGACY_SETTINGS_SCHEMA_VERSION: u16 = 1;
const MAX_LEGACY_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LEGACY_CHARACTER_FILES: usize = 256;
const MAX_SCAN_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub struct LegacySettingsAdapter {
    config_file: PathBuf,
    characters_dir: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsSnapshotV1 {
    schema_version: u16,
    available: bool,
    config: Option<LegacySettingsDocument>,
    characters: Vec<LegacySettingsDocument>,
    /// Legacy sections not yet mapped into the Rust settings domain.
    extra: LegacyExtraV1,
    warnings: Vec<LegacySettingsWarning>,
}

/// Read-only visibility into legacy config content that the settings domain
/// does not carry yet. Nothing here is written back; it exists so the UI and
/// operators can see what remains to be migrated.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyExtraV1 {
    /// Top-level config keys with no mapping into the Rust settings domain.
    unmapped_keys: Vec<String>,
    /// Known sections that are present in the legacy config.
    detected_sections: Vec<String>,
}

/// Top-level legacy config keys understood by the gateway (settings domain or
/// runtime wiring). Keys outside this set are surfaced as unmapped.
fn mapped_legacy_config_keys() -> &'static [&'static str] {
    &[
        "system_config",
        "character_config",
        "tool_prompts",
        "config_alts_dir",
    ]
}

fn legacy_extra(config: Option<&LegacySettingsDocument>) -> LegacyExtraV1 {
    let Some(config) = config else {
        return LegacyExtraV1::default();
    };
    let mapped = mapped_legacy_config_keys();
    let mut unmapped_keys = Vec::new();
    let mut detected_sections = Vec::new();
    if let Some(object) = config.data.as_object() {
        let mut keys: Vec<&String> = object.keys().collect();
        keys.sort();
        for key in keys {
            if mapped.contains(&key.as_str()) {
                detected_sections.push(key.clone());
            } else {
                unmapped_keys.push(key.clone());
            }
        }
    }
    LegacyExtraV1 {
        unmapped_keys,
        detected_sections,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsDocument {
    file_name: String,
    data: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsWarning {
    path: String,
    code: &'static str,
    message: String,
}

impl LegacySettingsAdapter {
    pub fn new(config_file: PathBuf, characters_dir: PathBuf) -> Self {
        Self {
            config_file,
            characters_dir,
        }
    }

    pub fn snapshot(&self) -> LegacySettingsSnapshotV1 {
        let mut warnings = Vec::new();
        let config = load_document(&self.config_file, config_display_name(&self.config_file))
            .map_err(|warning| warnings.push(warning))
            .ok();
        let mut characters = Vec::new();
        let mut character_paths = Vec::new();
        collect_yaml_files(
            &self.characters_dir,
            &self.characters_dir,
            0,
            &mut character_paths,
            &mut warnings,
        );
        character_paths.sort_by(|left, right| left.1.cmp(&right.1));
        character_paths.truncate(MAX_LEGACY_CHARACTER_FILES);

        for (path, relative_name) in character_paths {
            match load_document(&path, relative_name) {
                Ok(document) => characters.push(document),
                Err(warning) => warnings.push(warning),
            }
        }

        let extra = legacy_extra(config.as_ref());
        LegacySettingsSnapshotV1 {
            schema_version: LEGACY_SETTINGS_SCHEMA_VERSION,
            available: config.is_some(),
            config,
            characters,
            extra,
            warnings,
        }
    }

    /// Returns the `persona_prompt` of a character file, if present.
    /// Used by the native chat orchestrator to rebuild the system prompt on
    /// character switch. This is a read-only lookup over the same redacted
    /// parsing path as `snapshot`; prompts are not secrets.
    pub fn find_character_prompt(&self, file_name: &str) -> Option<String> {
        self.snapshot()
            .characters
            .into_iter()
            .find_map(|character| {
                if character.file_name != file_name {
                    return None;
                }
                character
                    .data
                    .get("character_config")?
                    .get("persona_prompt")?
                    .as_str()
                    .map(str::to_owned)
            })
    }
}

fn collect_yaml_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    output: &mut Vec<(PathBuf, String)>,
    warnings: &mut Vec<LegacySettingsWarning>,
) {
    if depth > MAX_SCAN_DEPTH || output.len() >= MAX_LEGACY_CHARACTER_FILES {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            if depth == 0 && error.kind() != io::ErrorKind::NotFound {
                warnings.push(warning(
                    "characters",
                    "read_failed",
                    "legacy character directory could not be read",
                ));
            }
            return;
        }
    };

    for entry in entries.flatten() {
        if output.len() >= MAX_LEGACY_CHARACTER_FILES {
            break;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_yaml_files(root, &path, depth + 1, output, warnings);
            continue;
        }
        if !metadata.is_file() || !is_yaml_path(&path) || metadata.len() > MAX_LEGACY_FILE_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative_name = path_to_forward_slashes(relative);
        output.push((path, relative_name));
    }
}

fn load_document(
    path: &Path,
    display_name: String,
) -> Result<LegacySettingsDocument, LegacySettingsWarning> {
    let metadata = fs::metadata(path).map_err(|error| {
        warning(
            &display_name,
            if error.kind() == io::ErrorKind::NotFound {
                "not_found"
            } else {
                "read_failed"
            },
            if error.kind() == io::ErrorKind::NotFound {
                "legacy settings file was not found"
            } else {
                "legacy settings file could not be read"
            },
        )
    })?;
    if metadata.len() > MAX_LEGACY_FILE_BYTES {
        return Err(warning(
            &display_name,
            "too_large",
            "legacy settings file exceeds the size limit",
        ));
    }
    let contents = fs::read_to_string(path).map_err(|_| {
        warning(
            &display_name,
            "read_failed",
            "legacy settings file could not be read",
        )
    })?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&contents).map_err(|_| {
        warning(
            &display_name,
            "invalid_yaml",
            "legacy settings file contains invalid YAML",
        )
    })?;
    let value = serde_json::to_value(yaml).map_err(|_| {
        warning(
            &display_name,
            "unsupported_yaml",
            "legacy settings file contains unsupported YAML keys",
        )
    })?;

    Ok(LegacySettingsDocument {
        file_name: display_name,
        data: redact_secrets(value),
    })
}

fn redact_secrets(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    if is_secret_key(&key) {
                        (key, secret_status(&value))
                    } else {
                        (key, redact_secrets(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_secrets).collect()),
        other => other,
    }
}

fn secret_status(value: &Value) -> Value {
    let configured = match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    };
    let hint = value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(mask_hint);
    json!({ "configured": configured, "hint": hint })
}

fn mask_hint(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() <= 4 {
        return "••••".to_owned();
    }
    let suffix: String = characters[characters.len() - 4..].iter().collect();
    format!("••••{suffix}")
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase().replace('-', "_");
    matches!(
        normalized.as_str(),
        "api_key"
            | "llm_api_key"
            | "password"
            | "secret"
            | "secret_id"
            | "secret_key"
            | "access_token"
            | "auth_token"
            | "bearer_token"
            | "client_secret"
            | "private_key"
            | "token"
    ) || normalized.ends_with("_api_key")
        || normalized.ends_with("_password")
        || normalized.ends_with("_secret")
        || (normalized.ends_with("_token") && normalized != "max_token")
}

fn warning(
    path: impl Into<String>,
    code: &'static str,
    message: impl Into<String>,
) -> LegacySettingsWarning {
    LegacySettingsWarning {
        path: path.into(),
        code,
        message: message.into(),
    }
}

fn config_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("conf.yaml")
        .to_owned()
}

fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "yaml" | "yml"))
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("olv-legacy-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn extra_reports_unmapped_top_level_keys() {
        let root = test_directory("extra");
        let config = root.join("conf.yaml");
        fs::write(
            &config,
            "system_config:\n  host: localhost\ncharacter_config:\n  conf_name: Mao\ntool_prompts:\n  web_search: search.txt\nemotion_config:\n  api_key: xyz\nlegacy_custom_plugin:\n  enabled: true\n",
        )
        .unwrap();
        let adapter = LegacySettingsAdapter::new(config, root.join("characters"));
        let snapshot = adapter.snapshot();
        assert!(snapshot.available);
        assert_eq!(snapshot.config.as_ref().unwrap().file_name, "conf.yaml");

        assert_eq!(
            snapshot.extra.detected_sections,
            vec!["character_config", "system_config", "tool_prompts"]
        );
        assert_eq!(
            snapshot.extra.unmapped_keys,
            vec!["emotion_config", "legacy_custom_plugin"]
        );

        // Secrets inside unmapped sections are still redacted.
        let emotion = snapshot.config.as_ref().unwrap().data["emotion_config"]
            .as_object()
            .unwrap();
        assert_eq!(emotion["api_key"]["configured"], serde_json::json!(true));
        assert!(emotion["api_key"]["hint"].is_string());
    }

    #[test]
    fn extra_is_empty_when_config_is_missing() {
        let root = test_directory("extra-missing");
        let adapter =
            LegacySettingsAdapter::new(root.join("missing.yaml"), root.join("characters"));
        let snapshot = adapter.snapshot();
        assert!(!snapshot.available);
        assert!(snapshot.extra.unmapped_keys.is_empty());
        assert!(snapshot.extra.detected_sections.is_empty());
    }

    #[test]
    fn snapshot_redacts_secrets_but_preserves_model_token_paths() {
        let root = test_directory("redaction");
        let config = root.join("conf.yaml");
        let characters = root.join("characters");
        fs::create_dir_all(&characters).unwrap();
        fs::write(
            &config,
            r#"system_config:
  host: localhost
character_config:
  agent_config:
    api_key: super-secret-key
  asr_config:
    tokens: ./models/tokens.txt
  tts_config:
    password: tiny
"#,
        )
        .unwrap();
        fs::write(
            characters.join("mao.yaml"),
            "character_config:\n  conf_name: Mao\n  auth_token: abcdefgh\n",
        )
        .unwrap();

        let snapshot = LegacySettingsAdapter::new(config, characters).snapshot();
        let encoded = serde_json::to_value(snapshot).unwrap();

        assert_eq!(encoded["available"], true);
        assert_eq!(
            encoded["config"]["data"]["character_config"]["agent_config"]["api_key"],
            json!({ "configured": true, "hint": "••••-key" })
        );
        assert_eq!(
            encoded["config"]["data"]["character_config"]["asr_config"]["tokens"],
            "./models/tokens.txt"
        );
        assert_eq!(
            encoded["config"]["data"]["character_config"]["tts_config"]["password"],
            json!({ "configured": true, "hint": "••••" })
        );
        assert_eq!(encoded["characters"][0]["fileName"], "mao.yaml");
        assert_eq!(
            encoded["characters"][0]["data"]["character_config"]["auth_token"],
            json!({ "configured": true, "hint": "••••efgh" })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_reports_missing_and_invalid_files_without_exposing_paths() {
        let root = test_directory("warnings");
        let characters = root.join("characters");
        fs::create_dir_all(&characters).unwrap();
        fs::write(characters.join("broken.yaml"), "character_config: [").unwrap();

        let snapshot =
            LegacySettingsAdapter::new(root.join("private-conf.yaml"), characters).snapshot();
        let encoded = serde_json::to_value(snapshot).unwrap();

        assert_eq!(encoded["available"], false);
        assert_eq!(encoded["warnings"][0]["path"], "private-conf.yaml");
        assert_eq!(encoded["warnings"][0]["code"], "not_found");
        assert_eq!(encoded["warnings"][1]["path"], "broken.yaml");
        assert_eq!(encoded["warnings"][1]["code"], "invalid_yaml");
        assert!(
            !encoded
                .to_string()
                .contains(&root.to_string_lossy().to_string())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secret_key_detection_does_not_hide_non_secret_token_counts() {
        assert!(is_secret_key("openai_api_key"));
        assert!(is_secret_key("client-secret"));
        assert!(is_secret_key("refresh_token"));
        assert!(!is_secret_key("max_tokens"));
        assert!(!is_secret_key("tokens"));
    }
}
