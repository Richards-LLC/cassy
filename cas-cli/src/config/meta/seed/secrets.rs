use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_secrets(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("secrets", "Secrets pasted in Commander for agents");
    registry.register(ConfigMeta {
        key: "secrets.destination",
        section: "secrets",
        name: "Secret Destination",
        description: "Folder where Commander saves a secret an agent asked for (coordination message target=operator kind=secret). Each secret is one mode-600 file named after the request, in a mode-700 folder; the agent gets the path, never the value. ~ expands to the home directory and <project> to the project folder's name. Empty uses the user default from ~/.cas/config.toml, then ~/.cas/private/creds/<project>. Saved files are kept until you delete them, unless secrets.auto_delete or the card chooses otherwise. Operator-only: agents cannot change it.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["secrets", "credentials", "creds", "key", "token", "commander"],
        use_cases: &[
            "Keep pasted keys for this project in its own creds folder",
            "Set one user-wide folder in ~/.cas/config.toml",
        ],
    });
    registry.register(ConfigMeta {
        key: "secrets.auto_delete",
        section: "secrets",
        name: "Secret Auto-Delete",
        description: "When Commander deletes a saved secret unless its card chooses otherwise: off (default, kept until you delete it), after_use (deleted once the supervisor replies to the saved message confirming use), or a time from 1m to 30d after saving, such as 1h. Each deletion appends a receipt without the value to .cas/secret-receipts.jsonl. Operator-only: agents cannot change it.",
        value_type: ConfigType::String,
        default: "off",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["secrets", "credentials", "delete", "expire", "auto-delete", "commander"],
        use_cases: &[
            "Delete pasted keys once the agent has used them",
            "Expire pasted keys an hour after saving",
        ],
    });
}
