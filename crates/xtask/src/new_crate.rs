//! `new-crate <name>` — add an internal workspace crate (unpublished by
//! default; rules in CONTRIBUTING.md "Adding a crate").

use std::path::Path;
use std::process::ExitCode;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

const CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "NAME_PLACEHOLDER"
description = "TODO: one-line description of this crate"
# Internal crates carry no semver burden. To publish later, switch to
# version.workspace = true, drop publish = false, and register the package
# in release-plz.toml — see CONTRIBUTING.md.
version = "0.0.0"
publish = false
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
"#;

const LIB_RS_TEMPLATE: &str =
    "//! TODO: crate documentation (the contract for consumers, not an implementation recap)\n";

/// Conservative package-name budget: keeps generated names portable and turns
/// filesystem component-length failures into input validation.
pub const MAX_CRATE_NAME_LEN: usize = 64;

/// Entry point: `new-crate <name>` (kebab-case).
pub fn run(args: &[String]) -> ExitCode {
    run_at(args, Path::new(ROOT))
}

fn run_at(args: &[String], root: &Path) -> ExitCode {
    let [name] = args else {
        eprintln!("usage: new-crate <name>  (kebab-case: lowercase letters, digits, hyphens)");
        return ExitCode::from(2);
    };
    if name.len() > MAX_CRATE_NAME_LEN {
        eprintln!(
            "error: crate name is {} bytes; the limit is {MAX_CRATE_NAME_LEN}",
            name.len()
        );
        return ExitCode::FAILURE;
    }
    if !is_kebab_case(name) {
        eprintln!("error: crate name must be kebab-case (lowercase letters, digits, hyphens)");
        return ExitCode::FAILURE;
    }
    if is_rust_keyword(name) {
        eprintln!(
            "error: crates/{name} cannot be used in Rust source because {name:?} is a Rust keyword"
        );
        return ExitCode::FAILURE;
    }
    let crate_dir = root.join("crates").join(name);
    match std::fs::symlink_metadata(&crate_dir) {
        Ok(_) => {
            eprintln!("error: crates/{name} already exists");
            return ExitCode::FAILURE;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            eprintln!("error: cannot inspect {}: {err}", crate_dir.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(err) = scaffold(&crate_dir, name) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    println!("created crates/{name}");
    println!(
        "to depend on it from another crate: register {{ path = \"crates/{name}\" }} in the root [workspace.dependencies] and inherit with workspace = true."
    );
    ExitCode::SUCCESS
}

fn scaffold(crate_dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(crate_dir.join("src"))?;
    let manifest = CARGO_TOML_TEMPLATE.replace("NAME_PLACEHOLDER", name);
    std::fs::write(crate_dir.join("Cargo.toml"), manifest)?;
    std::fs::write(crate_dir.join("src/lib.rs"), LIB_RS_TEMPLATE)?;
    Ok(())
}

/// `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` — hand-rolled so `xtask` stays std-only.
#[must_use]
pub fn is_kebab_case(name: &str) -> bool {
    let mut segments = name.split('-');
    let Some(first) = segments.next() else {
        return false;
    };
    let valid_first = first.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    valid_first
        && segments.all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// Keywords and reserved words cannot appear as the plain crate identifier in
/// generated Rust source. Raw identifiers do not cover every path keyword, so
/// scaffolding rejects the whole set rather than producing invalid code.
#[must_use]
pub fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "abstract"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "xtask-new-crate-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("crates")).unwrap();
        root
    }

    #[test]
    fn entry_point_rejects_keywords_and_long_names_before_writing() {
        let root = temp_root("invalid");
        for name in ["type".to_string(), "a".repeat(MAX_CRATE_NAME_LEN + 1)] {
            let args = vec![name];
            assert!(matches!(run_at(&args, &root), ExitCode::FAILURE));
        }
        assert_eq!(std::fs::read_dir(root.join("crates")).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entry_point_scaffolds_a_valid_crate() {
        let root = temp_root("valid");
        let args = vec!["demo".to_string()];
        assert!(matches!(run_at(&args, &root), ExitCode::SUCCESS));
        assert!(root.join("crates/demo/Cargo.toml").is_file());
        assert!(root.join("crates/demo/src/lib.rs").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rust_keywords_are_rejected() {
        for keyword in ["type", "crate", "self", "async", "gen", "yield"] {
            assert!(is_rust_keyword(keyword));
        }
        for name in ["my-cli", "typewriter", "async-runtime", "generator"] {
            assert!(!is_rust_keyword(name));
        }
    }

    #[test]
    fn kebab_case_validation() {
        assert!(is_kebab_case("a"));
        assert!(is_kebab_case("my-cli"));
        assert!(is_kebab_case("x1-y2"));
        assert!(!is_kebab_case("1abc"));
        assert!(!is_kebab_case("My-Cli"));
        assert!(!is_kebab_case("my_cli"));
        assert!(!is_kebab_case("my--cli"));
        assert!(!is_kebab_case("-cli"));
        assert!(!is_kebab_case("cli-"));
        assert!(!is_kebab_case(""));
    }
}
