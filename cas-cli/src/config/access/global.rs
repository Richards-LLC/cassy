use crate::config::*;

pub fn global_cas_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("cas"))
}

/// Load the global Cassy config from ~/.config/cas/
///
/// Returns default config if the directory or config file doesn't exist.
pub fn load_global_config() -> Config {
    if let Some(global_dir) = global_cas_dir() {
        if global_dir.exists() {
            Config::load(&global_dir).unwrap_or_default()
        } else {
            Config::default()
        }
    } else {
        Config::default()
    }
}

/// Save config to the global Cassy directory (~/.config/cas/)
///
/// Creates the directory if it doesn't exist.
pub fn save_global_config(config: &Config) -> Result<(), MemError> {
    if let Some(global_dir) = global_cas_dir() {
        std::fs::create_dir_all(&global_dir)?;
        config.save(&global_dir)
    } else {
        Err(MemError::Other(
            "Could not determine global config directory".to_string(),
        ))
    }
}

/// Check if telemetry consent has been given (either way)
///
/// Returns None if consent hasn't been asked yet, Some(true) if opted in,
/// Some(false) if opted out.
pub fn get_telemetry_consent() -> Option<bool> {
    load_global_config().telemetry().consent_given
}

/// Set telemetry consent in the global config
pub fn set_telemetry_consent(consent: bool) -> Result<(), MemError> {
    // Reads may degrade to defaults for status checks, but a mutating path
    // must never save those defaults over a malformed existing document.
    let mut config = match global_cas_dir() {
        Some(global_dir) => Config::load(&global_dir)?,
        None => Config::default(),
    };
    let telemetry = config
        .telemetry
        .get_or_insert_with(TelemetryConfig::default);
    telemetry.consent_given = Some(consent);
    telemetry.enabled = consent;
    save_global_config(&config)
}

/// Prompt the user for telemetry consent
///
/// Returns true if user consents, false otherwise.
/// This function reads from stdin and should only be called in interactive contexts.
pub fn prompt_telemetry_consent() -> bool {
    use std::io::{self, Write};

    println!();
    println!("Cassy collects anonymous usage data to improve the product.");
    println!("- No personal data or file contents collected");
    println!("- You can disable anytime: cas config telemetry.enabled false");
    println!();
    print!("Enable anonymous telemetry? [Y/n] ");
    let _ = io::stdout().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }

    let input = input.trim().to_lowercase();
    // Default to yes if empty, otherwise check for explicit no
    input.is_empty() || !input.starts_with('n')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn telemetry_mutation_preserves_malformed_global_config() {
        let mut env = TestEnvGuard::temp_home();
        let config_home = env.home().join(".config");
        env.set("XDG_CONFIG_HOME", &config_home);
        let global_dir = config_home.join("cas");
        std::fs::create_dir_all(&global_dir).unwrap();
        let config_path = global_dir.join("config.toml");
        let original = "[project]\naliases = []\nlse\n";
        std::fs::write(&config_path, original).unwrap();

        let error = set_telemetry_consent(true).unwrap_err().to_string();

        assert!(
            error.contains("Failed to parse config.toml at line 3"),
            "{error}"
        );
        assert!(
            error.contains("Restore a known-good config.toml backup"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), original);
    }
}
