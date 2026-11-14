//! `bump-toolchain`: point rust-toolchain.toml's `channel` at the latest
//! stable Rust release. That file is the toolchain-version SSOT (flake.nix,
//! rustup and CI all consume it), so a bump is exactly this one rewrite; the
//! weekly toolchain-update workflow wraps it in a validated PR.
//!
//! Std-only like the rest of xtask; the HTTPS fetch shells out to `curl`
//! (std has no TLS stack, and curl is present on every GitHub-hosted runner).
//! When `$GITHUB_OUTPUT` is set, `changed=<true|false>` and (on change)
//! `version=<X.Y.Z>` are appended for the calling workflow step.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const CHANNEL_URL: &str = "https://static.rust-lang.org/dist/channel-rust-stable.toml";

/// Entry point: `bump-toolchain` (no arguments).
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("usage: bump-toolchain  (no arguments)");
        return ExitCode::from(2);
    }
    match bump() {
        Ok(outcome) => match report_outcome(&outcome) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn report_outcome(outcome: &Outcome) -> Result<(), String> {
    let path = std::env::var_os("GITHUB_OUTPUT").map(PathBuf::from);
    report_outcome_at(outcome, path.as_deref())
}

enum Outcome {
    UpToDate(String),
    Bumped { from: String, to: String },
}

fn bump() -> Result<Outcome, String> {
    let path = toolchain_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let current = parse_channel(&content)
        .ok_or_else(|| format!("no `channel = \"...\"` line in {}", path.display()))?;
    if parse_version(&current).is_none() {
        return Err(format!(
            "channel {current:?} is not a pinned X.Y.Z version; refusing to guess"
        ));
    }
    let latest = latest_stable()?;
    if parse_version(&latest) <= parse_version(&current) {
        return Ok(Outcome::UpToDate(current));
    }
    let updated = bumped_content(&content, &latest)
        .ok_or_else(|| format!("failed to rewrite the channel line in {}", path.display()))?;
    std::fs::write(&path, updated)
        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    Ok(Outcome::Bumped {
        from: current,
        to: latest,
    })
}

/// rust-toolchain.toml at the workspace root (crates/xtask -> ../..).
fn toolchain_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("rust-toolchain.toml")
}

/// Query the latest stable release from the official channel manifest.
fn latest_stable() -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "60", CHANNEL_URL])
        .output()
        .map_err(|err| format!("cannot run curl (present on every GitHub-hosted runner): {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "curl {CHANNEL_URL} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let content = String::from_utf8(output.stdout)
        .map_err(|err| format!("channel manifest is not UTF-8: {err}"))?;
    parse_latest_stable(&content)
        .ok_or_else(|| "no [pkg.rust] version in the channel manifest".to_string())
}

/// The `channel` value of a rust-toolchain.toml (`channel = "X.Y.Z"`).
fn parse_channel(content: &str) -> Option<String> {
    content
        .lines()
        .find(|line| is_channel_line(line))
        .and_then(quoted_value)
}

/// The latest stable version in a channel-rust-stable.toml: `[pkg.rust]`'s
/// `version = "X.Y.Z (hash date)"`, reduced to the bare `X.Y.Z`.
fn parse_latest_stable(content: &str) -> Option<String> {
    let mut in_rust_pkg = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_rust_pkg = line == "[pkg.rust]";
            continue;
        }
        if in_rust_pkg && line.starts_with("version") {
            let value = quoted_value(line)?;
            return value.split_whitespace().next().map(str::to_owned);
        }
    }
    None
}

/// A `channel = "..."` line, with any leading indentation.
fn is_channel_line(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("channel")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// The first `"..."`-quoted string on a line.
fn quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_owned())
}

/// `X.Y.Z` as a comparable tuple; `None` for anything else (`nightly`,
/// `1.98`, `1.98.0.1`, ...).
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// `content` with only the channel line rewritten, preserving comments,
/// indentation and the trailing-newline convention.
fn bumped_content(content: &str, new_version: &str) -> Option<String> {
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        if replaced || !is_channel_line(line) {
            lines.push(line.to_owned());
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        lines.push(format!("{indent}channel = \"{new_version}\""));
        replaced = true;
    }
    if !replaced {
        return None;
    }
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Append outputs for the calling GitHub Actions step; use a sink locally.
/// Once the runner requests outputs, failing to emit one is a command failure:
/// otherwise the workflow can silently skip validation and PR creation.
fn report_outcome_at(outcome: &Outcome, path: Option<&Path>) -> Result<(), String> {
    match path {
        Some(path) => {
            let writer = open_output(path)?;
            report_outcome_with(outcome, Some(path), writer)
        }
        None => report_outcome_with(outcome, None, std::io::sink()),
    }
}

fn open_output(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|err| format!("cannot open GitHub output {}: {err}", path.display()))
}

fn report_outcome_with(
    outcome: &Outcome,
    output_path: Option<&Path>,
    mut writer: impl std::io::Write,
) -> Result<(), String> {
    match outcome {
        Outcome::UpToDate(current) => {
            write_output_at(&mut writer, output_path, "changed", "false")?;
            println!("rust toolchain is up to date ({current})");
        }
        Outcome::Bumped { from, to } => {
            write_output_at(&mut writer, output_path, "changed", "true")?;
            write_output_at(&mut writer, output_path, "version", to)?;
            println!("bumped rust toolchain: {from} -> {to}");
        }
    }
    Ok(())
}

fn write_output_at(
    writer: &mut impl std::io::Write,
    path: Option<&Path>,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    write_output(writer, path, key, value)
}

fn write_output(
    writer: &mut impl std::io::Write,
    path: &Path,
    key: &str,
    value: &str,
) -> Result<(), String> {
    writeln!(writer, "{key}={value}")
        .map_err(|err| format!("cannot write GitHub output {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOLCHAIN: &str =
        "# a comment\n[toolchain]\nchannel = \"1.94.0\"\ncomponents = [\"rustfmt\", \"clippy\"]\n";
    const MANIFEST: &str = "manifest-version = \"2\"\ndate = \"2026-08-18\"\n\n[pkg.rust]\nversion = \"1.98.0 (88d9e12ae 2026-08-18)\"\ngit_commit_hash = \"88d9e12ae\"\n\n[pkg.cargo]\nversion = \"0.99.0 (abc 2026-08-18)\"\n";

    #[test]
    fn parses_channel_and_manifest_version() {
        assert_eq!(parse_channel(TOOLCHAIN).as_deref(), Some("1.94.0"));
        assert_eq!(parse_latest_stable(MANIFEST).as_deref(), Some("1.98.0"));
    }

    #[test]
    fn compares_only_three_part_versions() {
        assert!(parse_version("1.98.0") < parse_version("1.98.1"));
        assert!(parse_version("1.98.0") > parse_version("1.97.1"));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.98"), None);
    }

    #[test]
    fn rewrites_only_the_channel_line() {
        let updated = bumped_content(TOOLCHAIN, "1.98.0").unwrap();
        assert!(updated.contains("channel = \"1.98.0\""));
        assert!(updated.contains("# a comment"));
        assert!(updated.ends_with('\n'));
        assert!(!updated.contains("1.94.0"));
    }

    fn bumped_outcome() -> Outcome {
        Outcome::Bumped {
            from: "1.97.0".to_string(),
            to: "1.98.0".to_string(),
        }
    }

    #[test]
    fn report_is_a_noop_without_an_output_path() {
        let outcome = Outcome::UpToDate("1.98.0".to_string());
        report_outcome_at(&outcome, None).unwrap();
    }

    #[test]
    fn report_bumped_writes_changed_and_version() {
        let outcome = Outcome::Bumped {
            from: "1.94.0".to_string(),
            to: "1.98.0".to_string(),
        };
        let mut output = Vec::new();
        report_outcome_with(&outcome, Some(Path::new("output.txt")), &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "changed=true\nversion=1.98.0\n"
        );
    }

    #[test]
    fn report_up_to_date_writes_only_changed() {
        let outcome = Outcome::UpToDate("1.98.0".to_string());
        let mut output = Vec::new();
        report_outcome_with(&outcome, Some(Path::new("output.txt")), &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "changed=false\n");
    }

    #[test]
    fn output_writes_the_expected_line() {
        let mut output = Vec::new();
        write_output(&mut output, Path::new("unused"), "changed", "true").unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "changed=true\n");
    }

    #[test]
    fn report_returns_output_open_failure_with_the_path() {
        let parent = std::env::temp_dir().join(format!(
            "xtask-toolchain-output-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&parent);
        let path = parent.join("output.txt");
        let error = report_outcome_at(&bumped_outcome(), Some(&path)).unwrap_err();
        assert!(error.contains("cannot open GitHub output"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn report_returns_output_write_failure_with_the_path() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected write failure"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let path = Path::new("runner-output.txt");
        let error = report_outcome_with(&bumped_outcome(), Some(path), FailingWriter).unwrap_err();
        assert!(error.contains("cannot write GitHub output"), "{error}");
        assert!(error.contains("runner-output.txt"), "{error}");
        assert!(error.contains("injected write failure"), "{error}");
    }
}
