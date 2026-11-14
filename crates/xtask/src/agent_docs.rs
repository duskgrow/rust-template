//! Smoke check for agent-facing docs ("can the host even load it?"):
//!
//! - every `.agents/skills/<name>/SKILL.md` has frontmatter whose `name` equals
//!   the directory name, whose `description` is 1..=1024 chars, whose keys stay
//!   within [`ALLOWED_KEYS`], and whose body fits the progressive-disclosure budget;
//! - `.claude/skills` points at `.agents/skills` (single source of truth —
//!   tolerates git's text-file fallback on platforms without symlink support);
//! - `AGENTS.md` exists, stays within the always-on line budget, carries its
//!   freshness note, and lists every discovered skill (and no stale ones);
//! - `CLAUDE.md` is only a reference to AGENTS.md, never a second copy;
//! - relative `.md` links in agent-facing and project Markdown resolve.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The repository root is baked in at compile time, so the check works from
/// any working directory (hooks, CI jobs and the bootstrap app alike).
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

// ~5k tokens of body text per skill: level-2 progressive-disclosure budget
const SKILL_BODY_BUDGET_CHARS: usize = 20_000;
const AGENTS_LINE_BUDGET: usize = 250;
// The SKILL.md standard guarantees only `name` and `description` are loaded at
// level 1; anything else would be metadata no host reads (see ADR-0005).
const ALLOWED_KEYS: [&str; 5] = [
    "name",
    "description",
    "license",
    "allowed-tools",
    "metadata",
];

/// Entry point: `agent-check` (no arguments).
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("usage: agent-check  (takes no arguments)");
        return ExitCode::from(2);
    }
    let root = Path::new(ROOT);
    let mut failures: Vec<String> = Vec::new();

    let (skill_files, skills_inventory_valid) =
        match collect_skill_files(&root.join(".agents/skills")) {
            Ok(files) => {
                if files.is_empty() {
                    failures.push(
                        ".agents/skills: no skills found (expected at least one */SKILL.md)"
                            .to_string(),
                    );
                }
                (files, true)
            }
            Err(err) => {
                failures.push(err);
                (Vec::new(), false)
            }
        };
    for skill_file in &skill_files {
        check_skill(skill_file, root, &mut failures);
    }

    check_claude_skills_link(root, &mut failures);
    check_agents_md(
        root,
        skills_inventory_valid.then_some(skill_files.as_slice()),
        &mut failures,
    );
    check_claude_md(root, &mut failures);
    check_internal_links(root, &mut failures);

    if failures.is_empty() {
        println!(
            "agent-doc smoke check ok: {} skill(s), AGENTS.md, CLAUDE.md, .claude/skills",
            skill_files.len()
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("agent-doc smoke check FAILED:");
    for failure in &failures {
        eprintln!("  - {failure}");
    }
    ExitCode::FAILURE
}

fn read_dir_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|err| format!("read directory {}: {err}", dir.display()))?;
    entries
        .map(|entry| {
            entry.map_err(|err| format!("read directory entry in {}: {err}", dir.display()))
        })
        .collect()
}

fn collect_skill_files(skills_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in read_dir_entries(skills_dir)? {
        let path = entry.path();
        let metadata =
            std::fs::metadata(&path).map_err(|err| format!("inspect {}: {err}", path.display()))?;
        if !metadata.is_dir() {
            continue;
        }
        let candidate = path.join("SKILL.md");
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => files.push(candidate),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("inspect {}: {err}", candidate.display())),
        }
    }
    files.sort();
    Ok(files)
}

fn check_skill(path: &Path, root: &Path, failures: &mut Vec<String>) {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let Ok(text) = std::fs::read_to_string(path) else {
        failures.push(format!("{rel}: unreadable"));
        return;
    };
    let (frontmatter_text, body) = match split_frontmatter(&text) {
        Ok(parts) => parts,
        Err(reason) => {
            failures.push(format!("{rel}: {reason}"));
            return;
        }
    };

    let frontmatter = parse_frontmatter(&frontmatter_text, &rel, failures);
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    validate_frontmatter(&frontmatter, &rel, &dir_name, failures);

    if body.chars().count() > SKILL_BODY_BUDGET_CHARS {
        failures.push(format!(
            "{rel}: body is {} chars, exceeding the {SKILL_BODY_BUDGET_CHARS}-char budget — move detail into reference files inside the skill directory",
            body.chars().count()
        ));
    }
}

fn validate_frontmatter(
    frontmatter: &HashMap<String, String>,
    rel: &str,
    dir_name: &str,
    failures: &mut Vec<String>,
) {
    let mut unknown: Vec<&String> = frontmatter
        .keys()
        .filter(|key| !ALLOWED_KEYS.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        failures.push(format!(
            "{rel}: unsupported frontmatter keys {unknown:?} — only {ALLOWED_KEYS:?} are allowed; activation conditions belong in `description`, not custom fields"
        ));
    }

    match frontmatter.get("name") {
        None => failures.push(format!("{rel}: frontmatter requires `name`")),
        Some(name) if name.is_empty() => {
            failures.push(format!("{rel}: frontmatter `name` must not be empty"));
        }
        Some(name) if name != dir_name => failures.push(format!(
            "{rel}: name {name:?} must equal directory name {dir_name:?}"
        )),
        Some(_) => {}
    }

    match frontmatter.get("description") {
        None => failures.push(format!("{rel}: frontmatter requires `description`")),
        Some(description) if description.is_empty() => {
            failures.push(format!(
                "{rel}: frontmatter `description` must not be empty"
            ));
        }
        Some(description) if description.chars().count() > 1024 => {
            failures.push(format!(
                "{rel}: description must be 1..1024 chars (got {})",
                description.chars().count()
            ));
        }
        Some(_) => {}
    }
}

/// Split raw file text into (frontmatter, body), keeping the two failure
/// modes apart for actionable messages. Windows checkouts may carry CRLF
/// line endings (git autocrlf); YAML hosts accept both, so normalize before
/// the line-oriented split.
fn split_frontmatter(text: &str) -> Result<(String, String), &'static str> {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err("missing YAML frontmatter (must start with ---)");
    };
    let Some(end) = rest.find("\n---\n") else {
        return Err("frontmatter is not closed with ---");
    };
    Ok((rest[..end].to_string(), rest[end + 5..].to_string()))
}

/// Parse the single-line `key: value` subset this checker supports. There is
/// no YAML parser in std, so validate against what strict YAML hosts reject:
/// an unquoted `: ` (or trailing `:`) ends the scalar there, and a leading
/// indicator char starts a construct this line-based format forbids. Plain
/// scalar comments start at a `#` preceded by YAML separation whitespace.
fn parse_frontmatter(text: &str, rel: &str, failures: &mut Vec<String>) -> HashMap<String, String> {
    let mut frontmatter = HashMap::new();
    let mut seen_keys = HashSet::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            failures.push(format!("{rel}: unparseable frontmatter line: {line:?}"));
            continue;
        };
        let key = key.trim().to_string();
        if !seen_keys.insert(key.clone()) {
            failures.push(format!("{rel}: duplicate frontmatter key {key:?}"));
            continue;
        }
        let value = value.trim();
        let quoted = value.starts_with('"') || value.starts_with('\'');
        let value = if quoted {
            value
        } else {
            strip_plain_comment(value)
        };
        if quoted {
            let quote = value.as_bytes()[0];
            if value.len() < 2 || !value.ends_with(char::from(quote)) {
                failures.push(format!("{rel}: unterminated quoted scalar for {key:?}"));
                continue;
            }
        } else if !value.is_empty() {
            if value.contains(": ") || value.ends_with(':') {
                failures.push(format!(
                    "{rel}: plain scalar for {key:?} contains ':' — strict YAML hosts reject this; wrap the value in double quotes"
                ));
                continue;
            }
            if ">|&*!%@`[{".contains(value.as_bytes()[0] as char) {
                failures.push(format!(
                    "{rel}: value for {key:?} starts with a YAML indicator — only single-line plain or quoted scalars are supported in SKILL.md frontmatter"
                ));
                continue;
            }
        }
        frontmatter.insert(key, value.trim_matches(['"', '\'']).to_string());
    }
    frontmatter
}

/// Strip a YAML plain-scalar comment without treating `value#part` as a
/// comment. The value has already been trimmed, so a leading `#` is a comment.
fn strip_plain_comment(value: &str) -> &str {
    let comment = value.char_indices().find_map(|(index, ch)| {
        if ch != '#' {
            return None;
        }
        let separated = index == 0 || matches!(value.as_bytes()[index - 1], b' ' | b'\t');
        separated.then_some(index)
    });
    comment.map_or(value, |index| value[..index].trim_end())
}

fn check_claude_skills_link(root: &Path, failures: &mut Vec<String>) {
    let link = root.join(".claude/skills");
    let expected = Path::new("../.agents/skills");
    match std::fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => match std::fs::read_link(&link) {
            Ok(target) if target == expected => {}
            Ok(target) => failures.push(format!(
                ".claude/skills: symlink must point to ../.agents/skills (got {})",
                target.display()
            )),
            Err(err) => failures.push(format!(".claude/skills: unreadable symlink: {err}")),
        },
        Ok(metadata) if metadata.is_file() => {
            // git on platforms without symlink support materializes a text
            // file containing the link target; accept that degradation.
            match std::fs::read_to_string(&link) {
                Ok(content) if content.trim() == "../.agents/skills" => {}
                Ok(content) => failures.push(format!(
                    ".claude/skills: unexpected pointer content {content:?}"
                )),
                Err(err) => failures.push(format!(".claude/skills: unreadable: {err}")),
            }
        }
        _ => failures
            .push(".claude/skills: missing — it must symlink to ../.agents/skills".to_string()),
    }
}

fn check_agents_md(root: &Path, skill_files: Option<&[PathBuf]>, failures: &mut Vec<String>) {
    let path = root.join("AGENTS.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        failures.push("AGENTS.md: missing".to_string());
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > AGENTS_LINE_BUDGET {
        failures.push(format!(
            "AGENTS.md: {} lines exceeds the {AGENTS_LINE_BUDGET}-line budget",
            lines.len()
        ));
    }
    if !lines
        .iter()
        .take(5)
        .any(|line| line.contains("Last reviewed"))
    {
        failures.push(
            "AGENTS.md: missing the freshness note (Last reviewed) in the header".to_string(),
        );
    }
    if let Some(skill_files) = skill_files {
        let actual: BTreeSet<String> = skill_files
            .iter()
            .filter_map(|path| path.parent())
            .filter_map(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        check_agents_skill_list(&text, &actual, failures);
    }
}

/// Compare only the dedicated Skills section's list items with the on-disk
/// inventory. Mentions in prose or other sections must not satisfy the listing.
fn check_agents_skill_list(text: &str, actual: &BTreeSet<String>, failures: &mut Vec<String>) {
    let entries = match agents_skill_entries(text) {
        Ok(entries) => entries,
        Err(reason) => {
            failures.push(format!("AGENTS.md: {reason}"));
            return;
        }
    };
    let mut listed = BTreeSet::new();
    for (line, name) in entries {
        if !listed.insert(name.clone()) {
            failures.push(format!(
                "AGENTS.md:{line}: duplicate skill listing {name:?}"
            ));
        }
        if !actual.contains(&name) {
            failures.push(format!(
                "AGENTS.md:{line}: listed skill {name:?} has no .agents/skills/{name}/SKILL.md"
            ));
        }
    }
    for name in actual.difference(&listed) {
        failures.push(format!(
            "AGENTS.md Skills: missing one-line listing for {name:?}"
        ));
    }
}

fn agents_skill_entries(text: &str) -> Result<Vec<(usize, String)>, &'static str> {
    let mut in_skills = false;
    let mut found = false;
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.starts_with("## ") {
            if in_skills {
                break;
            }
            in_skills = line
                .strip_prefix("## Skills")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
            found |= in_skills;
            continue;
        }
        if !in_skills {
            continue;
        }
        if let Some(name) = agents_skill_entry(line) {
            entries.push((index + 1, name));
        }
    }
    if found {
        Ok(entries)
    } else {
        Err("missing `## Skills` section")
    }
}

fn agents_skill_entry(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("- `")?;
    let (name, suffix) = rest.split_once('`')?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !(suffix.is_empty() || suffix.starts_with(char::is_whitespace))
    {
        return None;
    }
    Some(name.to_string())
}

/// Check relative inline Markdown links whose file part names a `.md` file.
/// Anchors are deliberately out of scope. Collection and parsing stay
/// conservative: ambiguous targets are skipped rather than misparsed.
fn check_internal_links(root: &Path, failures: &mut Vec<String>) {
    let files = match markdown_files(root) {
        Ok(files) => files,
        Err(err) => {
            failures.push(format!("Markdown link scan: {err}"));
            return;
        }
    };
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err) => {
                failures.push(format!("{rel}: cannot read Markdown: {err}"));
                continue;
            }
        };
        let parent = file.parent().unwrap_or(root);
        for target in find_md_link_targets(&text) {
            if !parent.join(&target).is_file() {
                failures.push(format!("{rel}: broken internal link -> {target}"));
            }
        }
    }
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in read_dir_entries(root)? {
        let file_type = entry
            .file_type()
            .map_err(|err| format!("inspect {}: {err}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let prefixed = name.starts_with("README") || name.starts_with("CONTRIBUTING");
        if name == "AGENTS.md" || (prefixed && name.ends_with(".md")) {
            files.push(entry.path());
        }
    }
    for dir in [root.join("docs"), root.join(".agents/skills")] {
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                collect_md_tree(&dir, &mut files)?;
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("inspect {}: {err}", dir.display())),
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_md_tree(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in read_dir_entries(dir)? {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("inspect {}: {err}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_md_tree(&path, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    Ok(())
}

/// Extract file parts from relative inline links. External URLs, pure anchors,
/// non-Markdown files, fenced code and inline code are ignored. Targets with a
/// title are skipped rather than risking a false path diagnostic.
fn find_md_link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some((marker, len)) = fence {
            if is_fence_close(trimmed, marker, len) {
                fence = None;
            }
            continue;
        }
        if let Some(start) = fence_start(trimmed) {
            fence = Some(start);
            continue;
        }

        let line = strip_inline_code(line);
        let mut rest = line.as_str();
        while let Some(index) = rest.find("](") {
            let after = &rest[index + 2..];
            let Some(close) = after.find(')') else {
                break;
            };
            let raw = after[..close].trim();
            rest = &after[close + 1..];
            if let Some(target) = md_link_file_part(raw) {
                targets.push(target);
            }
        }
    }
    targets
}

fn fence_start(line: &str) -> Option<(char, usize)> {
    let mut chars = line.chars();
    let marker = chars.next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let len = 1 + chars.take_while(|ch| *ch == marker).count();
    (len >= 3).then_some((marker, len))
}

fn is_fence_close(line: &str, marker: char, opening_len: usize) -> bool {
    let line = line.trim();
    let mut chars = line.chars();
    if chars.next() != Some(marker) {
        return false;
    }
    let len = 1 + chars.take_while(|ch| *ch == marker).count();
    len >= opening_len && line[len..].trim().is_empty()
}

/// Remove complete backtick code spans so literal examples such as
/// `[English](./X.md)` are not treated as links. An unmatched delimiter is
/// literal Markdown and is therefore retained.
fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        out.push_str(&rest[..open]);
        let ticks = &rest[open..];
        let delimiter_len = ticks.bytes().take_while(|byte| *byte == b'`').count();
        let content = &ticks[delimiter_len..];
        let Some(close) = find_closing_backticks(content, delimiter_len) else {
            out.push_str(ticks);
            return out;
        };
        rest = &content[close + delimiter_len..];
    }
    out.push_str(rest);
    out
}

fn find_closing_backticks(text: &str, delimiter_len: usize) -> Option<usize> {
    let mut offset = 0;
    while let Some(found) = text[offset..].find('`') {
        let start = offset + found;
        let run = text[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        if run == delimiter_len {
            return Some(start);
        }
        offset = start + run;
    }
    None
}

fn md_link_file_part(raw: &str) -> Option<String> {
    let bracketed = raw.starts_with('<') && raw.ends_with('>');
    let raw = if bracketed {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    if raw.is_empty()
        || raw.starts_with('#')
        || raw.starts_with('/')
        || raw.contains('%')
        || has_uri_scheme(raw)
    {
        return None;
    }
    let file_part = raw.split('#').next().unwrap_or("").trim();
    if file_part.is_empty()
        || (!bracketed && file_part.chars().any(char::is_whitespace))
        || Path::new(file_part)
            .extension()
            .is_none_or(|ext| ext != "md")
    {
        return None;
    }
    Some(file_part.to_string())
}

fn has_uri_scheme(target: &str) -> bool {
    let Some(colon) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon];
    scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn check_claude_md(root: &Path, failures: &mut Vec<String>) {
    let path = root.join("CLAUDE.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        failures.push("CLAUDE.md: missing (must be a one-line reference to AGENTS.md)".to_string());
        return;
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("<!--"))
        .collect();
    if lines != ["@AGENTS.md"] {
        failures.push(
            "CLAUDE.md: must contain only the @AGENTS.md reference (SSOT — never a copy)"
                .to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (HashMap<String, String>, Vec<String>) {
        let mut failures = Vec::new();
        let frontmatter = parse_frontmatter(text, "test/SKILL.md", &mut failures);
        (frontmatter, failures)
    }

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("xtask-agent-docs-{name}-{}", std::process::id()))
    }

    #[test]
    fn split_tolerates_crlf_checkouts() {
        // git materializes CRLF on Windows checkouts (autocrlf); YAML hosts
        // accept both endings, so the checker must too.
        let (frontmatter, body) =
            split_frontmatter("---\r\nname: x\r\n---\r\nbody\r\n").expect("valid frontmatter");
        assert_eq!(frontmatter, "name: x");
        assert_eq!(body, "body\n");
    }

    #[test]
    fn split_distinguishes_open_and_close_failures() {
        assert_eq!(
            split_frontmatter("name: x\n").unwrap_err(),
            "missing YAML frontmatter (must start with ---)"
        );
        assert_eq!(
            split_frontmatter("---\nname: x\n").unwrap_err(),
            "frontmatter is not closed with ---"
        );
    }

    #[test]
    fn plain_scalars_parse() {
        let (frontmatter, failures) = parse("name: pr-preflight\ndescription: short\n");
        assert!(failures.is_empty());
        assert_eq!(frontmatter["description"], "short");
    }

    #[test]
    fn plain_scalar_comments_are_stripped_only_at_separation_whitespace() {
        let (frontmatter, failures) =
            parse("name: demo # directory name\ndescription: short # see: details\n");
        assert!(failures.is_empty());
        assert_eq!(frontmatter["name"], "demo");
        assert_eq!(frontmatter["description"], "short");

        let (frontmatter, failures) = parse("description: value#part\n");
        assert!(failures.is_empty());
        assert_eq!(frontmatter["description"], "value#part");
    }

    #[test]
    fn comment_only_name_and_description_fail_non_empty_validation() {
        let (frontmatter, mut failures) = parse("name: # comment\ndescription: # comment\n");
        assert!(failures.is_empty());
        assert_eq!(frontmatter["name"], "");
        assert_eq!(frontmatter["description"], "");

        validate_frontmatter(&frontmatter, "test/SKILL.md", "demo", &mut failures);
        assert_eq!(failures.len(), 2);
        assert!(
            failures
                .iter()
                .any(|error| error.contains("`name` must not be empty"))
        );
        assert!(
            failures
                .iter()
                .any(|error| error.contains("`description` must not be empty"))
        );
    }

    #[test]
    fn duplicate_frontmatter_keys_are_rejected() {
        let (frontmatter, failures) = parse("name: first\nname: second\ndescription: short\n");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("duplicate frontmatter key \"name\""));
        assert_eq!(frontmatter["name"], "first");
    }

    #[test]
    fn unquoted_colon_space_is_rejected() {
        let (_, failures) = parse("description: layer on top: diff self-review\n");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("wrap the value in double quotes"));
    }

    #[test]
    fn quoted_colon_space_and_hash_are_preserved() {
        let (frontmatter, failures) = parse("description: \"layer on top: diff # literal\"\n");
        assert!(failures.is_empty());
        assert_eq!(frontmatter["description"], "layer on top: diff # literal");
    }

    #[test]
    fn unterminated_quote_and_indicators_are_rejected() {
        let (_, failures) = parse("description: \"never closed\n");
        assert_eq!(failures.len(), 1);
        let (_, failures) = parse("description: >- folded\n");
        assert_eq!(failures.len(), 1);
    }

    #[test]
    fn missing_skill_directory_is_an_explicit_error() {
        let missing = temp_root("missing-dir");
        let _ = std::fs::remove_dir_all(&missing);
        let error = collect_skill_files(&missing).unwrap_err();
        assert!(error.contains("read directory"), "{error}");
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }

    #[test]
    fn agents_skill_listing_is_bidirectional_and_section_scoped() {
        let actual = BTreeSet::from(["missing".to_string(), "present".to_string()]);
        let text = "Prose mentions `outside` and `present`.\n\
                    ## Skills (procedures)\n\
                    - `present` — actual skill\n\
                    - `present` — duplicate in the same section\n\
                    - `stale` — no such directory\n\
                    ## Style\n\
                    - `outside` — must not count as a listing\n\
                    - `present` — must not duplicate the real entry\n";
        let mut failures = Vec::new();
        check_agents_skill_list(text, &actual, &mut failures);
        assert_eq!(failures.len(), 3, "{failures:?}");
        assert!(
            failures
                .iter()
                .any(|error| error.contains("duplicate skill listing \"present\""))
        );
        assert!(
            failures
                .iter()
                .any(|error| error.contains("listed skill \"stale\""))
        );
        assert!(
            failures
                .iter()
                .any(|error| error.contains("missing one-line listing for \"missing\""))
        );
        assert!(!failures.iter().any(|error| error.contains("outside")));
    }

    #[test]
    fn agents_skill_listing_requires_its_section() {
        let mut failures = Vec::new();
        check_agents_skill_list(
            "- `present` — prose, not a section\n",
            &BTreeSet::new(),
            &mut failures,
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("missing `## Skills` section"));
    }

    #[test]
    fn md_link_targets_skip_external_anchors_code_and_titles() {
        let text = "[ADR](0012-docs.md) [section](fiber.md#s1) [external](https://example.com/x.md)\n\
                    [mail](mailto:x.md) [anchor](#s1) [image](../assets/x.png)\n\
                    ```text\n[fenced](fake.md)\n```\n\
                    ~~~\n[also fenced](fake-2.md)\n~~~\n\
                    `[inline](X.md)` [real](real.md) [title](title.md \"title\")\n\
                    [angle](<two words.md>) [encoded](two%20words.md) [root](/README.md)\n";
        assert_eq!(
            find_md_link_targets(text),
            vec![
                "0012-docs.md".to_string(),
                "fiber.md".to_string(),
                "real.md".to_string(),
                "two words.md".to_string(),
            ]
        );
    }

    #[test]
    fn internal_links_check_scoped_markdown_and_resolve_from_each_file() {
        let root = temp_root("links");
        let _ = std::fs::remove_dir_all(&root);
        let references = root.join(".agents/skills/demo/references");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(&references).unwrap();
        std::fs::write(root.join("README.md"), "# Home\n").unwrap();
        std::fs::write(
            root.join("docs/guide.md"),
            "[home](../README.md)\n[missing](missing.md)\n\
              [encoded](two%20words.md) [root](/missing.md)\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".agents/skills/demo/SKILL.md"),
            "`[literal example](X.md)`\n",
        )
        .unwrap();
        std::fs::write(
            references.join("details.md"),
            "[home](../../../../README.md)\n",
        )
        .unwrap();

        let mut failures = Vec::new();
        check_internal_links(&root, &mut failures);
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("docs/guide.md: broken internal link -> missing.md"));
    }
}
