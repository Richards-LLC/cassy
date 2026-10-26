//! Project capture settings shared by the capture command and evidence gate.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Only configuration lives here; capture execution belongs to deliverables.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QaCaptureConfig {
    pub base_url: String,
    pub serve: String,
    pub ready_path: String,
    pub shots: String,
    /// Environment-variable NAME, never credentials or the resolved file path.
    pub credentials_env: String,
    /// Empty disables login for public surfaces.
    pub login_path: String,
    pub success_url: String,
    pub email_sel: String,
    pub password_sel: String,
    pub submit_sel: String,
    pub mask: Vec<String>,
    /// Width x height; mobile is used only when a shot requests it.
    pub viewports: Vec<String>,
    pub max_shots: u32,
    pub timeout_secs: u32,
    pub max_html_mb: u32,
}

impl Default for QaCaptureConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            serve: String::new(),
            ready_path: "/".into(),
            shots: "docs/qa/shots.json".into(),
            credentials_env: String::new(),
            login_path: String::new(),
            success_url: String::new(),
            email_sel: "input[type=email]".into(),
            password_sel: "input[type=password]".into(),
            submit_sel: "button[type=submit]".into(),
            mask: Vec::new(),
            viewports: vec!["1280x800".into()],
            max_shots: 6,
            timeout_secs: 240,
            max_html_mb: 8,
        }
    }
}

impl QaCaptureConfig {
    /// Budget, catalog and login defaults alone never enable capture.
    /// Gates must refuse Err: broken runtime auth is a finding, never opt-out.
    pub fn is_configured(&self) -> Result<bool, String> {
        if self.base_url.trim().is_empty() && self.serve.trim().is_empty() {
            return Ok(false);
        }
        validate_value("qa.capture.base_url", &self.base_url)?;
        validate_value("qa.capture.serve", &self.serve)?;
        validate_value("qa.capture.success_url", &self.success_url)?;
        if !self.login_path.trim().is_empty() && self.success_url.trim().is_empty() {
            return Err("qa.capture.success_url is required for login (anchor it with ^)".into());
        }
        self.credential_file()?;
        Ok(true)
    }

    /// Resolve the QA file at capture/doctor time. Config setters deliberately
    /// do not require the setting shell to export this variable. Open only to
    /// prove readability; never read or return any credential contents.
    pub fn credential_file(&self) -> Result<Option<PathBuf>, String> {
        if self.credentials_env.is_empty() {
            return if self.login_path.trim().is_empty() {
                Ok(None)
            } else {
                Err("qa.capture.credentials_env is required for login".into())
            };
        }
        validate_value("qa.capture.credentials_env", &self.credentials_env)?;
        let path = std::env::var_os(&self.credentials_env)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or("qa.capture.credentials_env must resolve to an existing readable file (variable is unset or empty)")?;
        let metadata = path
            .metadata()
            .map_err(|_| "qa.capture.credentials_env must resolve to an existing readable file")?;
        if !metadata.is_file() {
            return Err(
                "qa.capture.credentials_env must resolve to a regular readable file".into(),
            );
        }
        let file = std::fs::File::open(&path)
            .map_err(|_| "qa.capture.credentials_env must resolve to an existing readable file")?;
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Err(
                "qa.capture.credentials_env must resolve to a regular readable file".into(),
            );
        }
        Ok(Some(path))
    }
}

/// Shared metadata validation covers set, editors and imports before mutation.
pub(super) fn validate_value(key: &str, raw: &str) -> Result<(), String> {
    let value = raw.trim();
    let valid = match key {
        "qa.capture.credentials_env" => {
            value.is_empty() || {
                let mut chars = value.chars();
                chars
                    .next()
                    .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                    && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
            }
        }
        "qa.capture.success_url" => value.is_empty() || anchored_pattern(value),
        "qa.capture.base_url" => {
            value.is_empty()
                || (!value.chars().any(char::is_whitespace)
                    && url::Url::parse(value).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https")
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none()
                    }))
        }
        "qa.capture.serve" => value.is_empty() || value.contains("{port}"),
        "qa.capture.ready_path" => value.starts_with('/') && !value.starts_with("//"),
        "qa.capture.login_path" => {
            value.is_empty() || (value.starts_with('/') && !value.starts_with("//"))
        }
        "qa.capture.viewports" => {
            !value.is_empty()
                && value
                    .split(',')
                    .all(|viewport| matches!(viewport.trim(), "1280x800" | "390x844"))
        }
        _ => true,
    };
    if valid {
        return Ok(());
    }
    let requirement = match key {
        "qa.capture.credentials_env" => {
            "an environment-variable NAME (for example PROJECT_QA_CREDENTIALS), never a secret or file path"
        }
        "qa.capture.success_url" => "a valid regular expression anchored with ^, or empty to clear",
        "qa.capture.base_url" => {
            "an HTTP(S) URL without credentials, query or fragment, or empty to clear"
        }
        "qa.capture.serve" => "a serve command containing {port}, or empty to clear",
        "qa.capture.viewports" => "1280x800 and optionally 390x844, comma-separated",
        _ => "a local path beginning with /",
    };
    // Invalid values may be mistaken secrets; never echo them in findings.
    Err(format!("{key} requires {requirement}"))
}

fn anchored_pattern(pattern: &str) -> bool {
    if !pattern.starts_with('^') || regex::Regex::new(pattern).is_err() {
        return false;
    }
    // Every top-level alternative needs its own anchor. ^(a|b) is already
    // anchored as a whole, whereas ^a|b can match the login page via b.
    let (mut groups, mut classes, mut escaped) = (0_u32, 0_u32, false);
    for (index, character) in pattern.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => classes += 1,
            ']' if classes > 0 => classes -= 1,
            _ if classes > 0 => {}
            '(' => groups += 1,
            ')' => groups = groups.saturating_sub(1),
            '|' if groups == 0 && !pattern[index + 1..].starts_with('^') => return false,
            _ => {}
        }
    }
    true
}
