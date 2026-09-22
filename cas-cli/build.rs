//! Build script for CAS
//!
//! Captures git commit hash and build timestamp for version info.
//! Also loads telemetry keys from .env file for compile-time embedding.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    generate_archive_fixture_sources();
    println!("cargo:rerun-if-changed=../hub-web/dist/index.html");
    println!("cargo:rerun-if-changed=../hub-web/dist/app.js");
    println!("cargo:rerun-if-changed=../hub-web/dist/app.css");
    println!("cargo:rerun-if-changed=../hub-web/dist/ghostty-vt.wasm");
    println!("cargo:rerun-if-changed=../hub-web/dist/ghostty-write-pty.wasm");
    println!("cargo:rerun-if-changed=../hub-web/dist/symbols.woff2");
    // Load .env file if present (for telemetry keys)
    load_env_file();

    // Get git commit hash
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Check if tracked files have uncommitted changes (ignore untracked files)
    let is_dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .ok()
        .map(|s| !s.success())
        .unwrap_or(false);

    let git_info = if is_dirty {
        format!("{git_hash}-dirty")
    } else {
        git_hash
    };

    // Get build date
    let build_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Export as environment variables for compilation
    println!("cargo:rustc-env=CAS_GIT_HASH={git_info}");
    println!("cargo:rustc-env=CAS_BUILD_DATE={build_date}");

    // Rebuild if git metadata changes. A linked worktree has a `.git` file,
    // not a directory, so resolve the per-worktree and common git directories
    // before registering inputs. Watching only files that exist also avoids
    // Cargo treating optional or packed refs as permanently dirty inputs.
    watch_git_paths();

    // Rebuild if .env changes
    watch_optional_path(Path::new("../.env"));
    watch_optional_path(Path::new(".env"));
}

/// Generate the archive test's embedded source manifest from the test tree.
///
/// The archive runner has no producer checkout to enumerate, so the generated
/// file embeds every Rust test source at build time. The portability guard can
/// then supplement that snapshot with a runtime enumeration when a checkout is
/// available, catching a newly-added file before the next rebuild as well.
fn generate_archive_fixture_sources() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
    );
    let tests_dir = manifest_dir.join("tests");
    let mut sources = Vec::new();
    match fs::metadata(&tests_dir) {
        Ok(metadata) if metadata.is_dir() => collect_archive_sources(
            &tests_dir,
            &tests_dir,
            "cas-cli/tests",
            "tests",
            false,
            &mut sources,
        ),
        Ok(_) => collect_archive_sources(
            &tests_dir,
            &tests_dir,
            "cas-cli/tests",
            "tests",
            false,
            &mut sources,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Cargo treats a missing rerun input as perpetually dirty. Create
            // the empty source root so the requested `rerun-if-changed=tests`
            // watches a stable directory and notices files added later.
            fs::create_dir_all(&tests_dir).unwrap_or_else(|create_error| {
                panic!(
                    "create test source directory {}: {create_error}",
                    tests_dir.display()
                )
            });
            fs::write(tests_dir.join(".cas-empty"), b"").unwrap_or_else(|write_error| {
                panic!(
                    "seed test source directory {}: {write_error}",
                    tests_dir.display()
                )
            });
        }
        Err(error) => panic!(
            "read test source directory {}: {error}",
            tests_dir.display()
        ),
    }

    let source_dir = manifest_dir.join("src");
    if source_dir.is_dir() {
        collect_archive_sources(
            &source_dir,
            &source_dir,
            "cas-cli/src",
            "src",
            true,
            &mut sources,
        );
    }

    let crates_dir = manifest_dir.join("../crates");
    if crates_dir.is_dir() {
        let entries = fs::read_dir(&crates_dir).unwrap_or_else(|error| {
            panic!("read crate source root {}: {error}", crates_dir.display())
        });
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "read crate source root entry {}: {error}",
                    crates_dir.display()
                )
            });
            let crate_root = entry.path();
            let Some(crate_name) = crate_root.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !crate_root.is_dir() {
                continue;
            }
            let source_dir = crate_root.join("src");
            if source_dir.is_dir() {
                collect_archive_sources(
                    &source_dir,
                    &source_dir,
                    &format!("crates/{crate_name}/src"),
                    &format!("../crates/{crate_name}/src"),
                    true,
                    &mut sources,
                );
            }
        }
    }
    sources.sort();

    let mut generated = String::from(
        "// @generated by cas-cli/build.rs; do not edit by hand.\npub(crate) const GENERATED_FIXTURE_SOURCES: &[(&str, &str)] = &[\n",
    );
    for (name, include_path) in sources {
        generated.push_str(&format!(
            "    (\"{name}\", include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{include_path}\"))),\n"
        ));
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    fs::write(out_dir.join("archive_fixture_sources.rs"), generated)
        .expect("write generated archive fixture source manifest");
    println!("cargo:rerun-if-changed=tests");
    println!("cargo:rerun-if-changed=src");
    if crates_dir.is_dir() {
        println!("cargo:rerun-if-changed=../crates");
    }
}

fn collect_archive_sources(
    directory: &Path,
    root: &Path,
    archive_root: &str,
    include_root: &str,
    test_only: bool,
    output: &mut Vec<(String, String)>,
) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!("read source entry in {}: {error}", directory.display())
        });
        let path = entry.path();
        if path.is_dir() {
            collect_archive_sources(&path, root, archive_root, include_root, test_only, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read source file {}: {error}", path.display()));
            if test_only && !contains_test_block(&source) {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("source is beneath the source directory")
                .to_string_lossy()
                .replace('\\', "/");
            output.push((
                format!("{archive_root}/{relative}"),
                format!("{include_root}/{relative}"),
            ));
        }
    }
}

fn contains_test_block(source: &str) -> bool {
    source.contains("#[cfg(test") || source.contains("#[test")
}

fn watch_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn watch_optional_path(path: &Path) {
    // Cargo treats a missing rerun input as dirty on every invocation. It also
    // recursively fingerprints directories, so watching the parent of an
    // absent root-level `.env` would include an in-tree `target/` directory
    // and make the build script perpetually stale. Existing optional files
    // remain precise inputs; absent files are picked up on the next build
    // triggered by another input (or a clean rebuild).
    watch_if_exists(path);
}

fn watch_existing_ancestor(path: &Path) {
    let mut candidate = path;
    loop {
        if candidate.exists() {
            watch_if_exists(candidate);
            return;
        }
        let Some(parent) = candidate.parent() else {
            return;
        };
        if parent == candidate {
            return;
        }
        candidate = parent;
    }
}

fn watch_git_paths() {
    let Some(git_dir) = git_path("--git-dir") else {
        return;
    };
    let Some(git_common_dir) = git_path("--git-common-dir") else {
        return;
    };

    watch_if_exists(&git_dir.join("HEAD"));
    watch_if_exists(&git_dir.join("index"));

    if let Some(symbolic_head) = git_output(&["symbolic-ref", "--quiet", "HEAD"])
        && symbolic_head.starts_with("refs/")
    {
        let branch_ref = git_common_dir.join(symbolic_head);
        if branch_ref.exists() {
            watch_if_exists(&branch_ref);
        } else {
            // The current ref may be packed. Watch its existing namespace so
            // creation of the loose ref is observed; after that transition,
            // the next build switches to the exact file above.
            watch_existing_ancestor(branch_ref.parent().unwrap_or(&git_common_dir));
        }
    }

    // A branch ref can be packed instead of having a loose file. Registering
    // this existing file keeps ref changes observable without adding a missing
    // path that would make every build dirty.
    watch_if_exists(&git_common_dir.join("packed-refs"));
}

fn git_path(kind: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", kind])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Load telemetry keys from .env file and pass to compiler
fn load_env_file() {
    // Try project root .env first, then cas-cli/.env
    let env_paths = ["../.env", ".env"];

    for path in env_paths {
        if std::path::Path::new(path).exists() {
            if let Ok(iter) = dotenvy::from_filename_iter(path) {
                for item in iter.flatten() {
                    let (key, value) = item;
                    // Only pass through telemetry-related keys
                    if key == "CAS_POSTHOG_API_KEY"
                        || key == "CAS_SENTRY_DSN"
                        || key == "POSTHOG_API_KEY"
                        || key == "SENTRY_DSN"
                    {
                        println!("cargo:rustc-env={key}={value}");
                    }
                }
            }
            break; // Use first .env found
        }
    }
}
