use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_peers(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("peers", "Same-repo peer supervisor scope");
    registry.register(ConfigMeta {
        key: "peers.same_repo_scope",
        section: "peers",
        name: "Same-Repo Peer Scope",
        description: "Which supervisors of this repo `cas peers`, coordination action=peers and plain-name peer messages reach. team (default) also lists teammates' supervisors on a shared team project, each labelled with its owner; own lists and messages only your own Cassy Cloud account's supervisors and refuses a send to anyone else's. Cross-project messages are always own-account only. Operator-only.",
        value_type: ConfigType::String,
        default: "team",
        constraint: Constraint::OneOf(vec!["team".into(), "own".into()]),
        advanced: false,
        requires_feature: None,
        keywords: &["peers", "supervisor", "team", "account", "user", "privacy", "message"],
        use_cases: &["Keep teammates' supervisors out of this project's peer list and messages", "See and message teammates' supervisors on a shared team project"],
    });
}
