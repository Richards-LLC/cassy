//! Access to the compile-time builtin catalogs from integration tests.

use cas::builtins::{
    BuiltinFile, BUILTIN_AGENTS, BUILTIN_SKILLS, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS,
    GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flavor {
    Claude,
    Codex,
    Grok,
}

pub const FLAVORS: &[(Flavor, &str)] = &[
    (Flavor::Claude, "claude"),
    (Flavor::Codex, "codex"),
    (Flavor::Grok, "grok"),
];

pub fn skills(flavor: Flavor) -> &'static [BuiltinFile] {
    match flavor {
        Flavor::Claude => BUILTIN_SKILLS,
        Flavor::Codex => CODEX_BUILTIN_SKILLS,
        Flavor::Grok => GROK_BUILTIN_SKILLS,
    }
}

pub fn agents(flavor: Flavor) -> &'static [BuiltinFile] {
    match flavor {
        Flavor::Claude => BUILTIN_AGENTS,
        Flavor::Codex => CODEX_BUILTIN_AGENTS,
        Flavor::Grok => GROK_BUILTIN_AGENTS,
    }
}

pub fn find(flavor: Flavor, relative: &str) -> &'static str {
    skills(flavor)
        .iter()
        .chain(agents(flavor))
        .find(|builtin| builtin.path == relative)
        .unwrap_or_else(|| panic!("builtin catalog is missing {relative} for {flavor:?}"))
        .content
}

/// Resolve a source-tree-shaped path such as
/// `cas-cli/src/builtins/codex/skills/cas-search.md`.
pub fn find_source_path(path: &str) -> &'static str {
    let relative = path
        .strip_prefix("cas-cli/src/builtins/")
        .unwrap_or_else(|| panic!("not a builtin source path: {path}"));
    let (flavor, relative) = if let Some(relative) = relative.strip_prefix("codex/") {
        (Flavor::Codex, relative)
    } else if let Some(relative) = relative.strip_prefix("grok/") {
        (Flavor::Grok, relative)
    } else {
        (Flavor::Claude, relative)
    };
    find(flavor, relative)
}
