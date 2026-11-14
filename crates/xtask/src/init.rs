//! `init <name> <owner>` — one-shot bootstrap: turns the template into a fresh
//! project (rename, strip template artifacts, git init, stage). This module is
//! template-only: a successful init deletes it (and its registration in
//! `main.rs`), so generated projects never carry their own scaffolding.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::new_crate::{MAX_CRATE_NAME_LEN, is_kebab_case, is_rust_keyword};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
/// This very file contains the placeholder literals and is deleted at the end
/// of the run — never substitute inside it.
const SELF_REL: &str = "crates/xtask/src/init.rs";
/// Never substituted: VCS/build/tooling directories.
const SKIP_DIRS: [&str; 3] = [".git", "target", ".direnv"];
const TEMPLATE_STRIP_FILES: [&str; 4] = [
    "justfile",
    "flake.nix",
    ".github/workflows/ci.yml",
    "crates/xtask/src/main.rs",
];
const TEMPLATE_RESIDUE: [&str; 4] = [
    "__project_name__",
    "__GITHUB_OWNER__",
    ">>> template-only",
    "<<< template-only",
];

const README_TEMPLATE: &str = r"# @PROJECT@

[中文文档](./README.zh-CN.md)

[![CI](https://github.com/@OWNER@/@PROJECT@/actions/workflows/ci.yml/badge.svg)](https://github.com/@OWNER@/@PROJECT@/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)

TODO: one-line description of what this project does (also update `description` in crates/@PROJECT@/Cargo.toml).

## Install

After the first release, use the installer from GitHub Releases:

    curl --proto '=https' --tlsv1.2 -LsSf https://github.com/@OWNER@/@PROJECT@/releases/latest/download/@PROJECT@-installer.sh | sh

crates.io installation becomes available after opting into registry publishing
and publishing the crate; see the Releasing section in CONTRIBUTING.md.

## Usage

See `@PROJECT@ --help`.

## Development

    direnv allow   # or: nix develop (entering the shell also arms the git hooks)
    just ci        # core local quality gate; CI adds platform and hosting checks

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

MIT OR Apache-2.0 — see [LICENSE-MIT](./LICENSE-MIT) and [LICENSE-APACHE](./LICENSE-APACHE).
";

const README_ZH_TEMPLATE: &str = r"# @PROJECT@

[English](./README.md)（如有出入以英文版为准）

TODO: 一句话说明这个项目做什么（同步更新 crates/@PROJECT@/Cargo.toml 的 description）。

## 安装

首次发版后可使用 GitHub Release 的安装脚本：

    curl --proto '=https' --tlsv1.2 -LsSf https://github.com/@OWNER@/@PROJECT@/releases/latest/download/@PROJECT@-installer.sh | sh

按 CONTRIBUTING.zh-CN.md「发布」启用 registry 发布并实际发布 crate 后，也可以从 crates.io 安装。

## 使用

见 `@PROJECT@ --help`。

## 开发

    direnv allow   # 或 nix develop（进入 devShell 会自动安装 git 钩子）
    just ci        # 本地核心质量门；CI 另跑平台矩阵与托管侧检查

详见 [CONTRIBUTING.zh-CN.md](./CONTRIBUTING.zh-CN.md)。

## License

MIT OR Apache-2.0，见 [LICENSE-MIT](./LICENSE-MIT) 与 [LICENSE-APACHE](./LICENSE-APACHE)。
";

const ADR_TEMPLATE: &str = r#"# 0001. Record architecture decisions with ADRs

- Status: accepted
- Date: @DATE@

## Context

The hardest thing to track over a project's lifetime is the _motivation_ behind
decisions; rationale scattered across PR descriptions, chat and wikis rots or
contradicts itself.

## Decision

Record architecturally significant decisions as lightweight MADR files in
`docs/decisions/`, named `NNNN-title.md`, sequentially numbered, numbers never
reused. A superseded decision is marked `superseded by ADR-NNNN`, never deleted.
Code comments, docs and PR descriptions reference the number ("see ADR-0007")
instead of restating the rationale.

## Consequences

Each decision's rationale has exactly one authoritative location; a new decision
is a new file, so there is no merge-conflict hot spot. ADRs record "why we
decided this back then"; current-state descriptions belong to architecture docs
and code.
"#;

/// Entry point: `init <project-name> <github-owner>`.
pub fn run(args: &[String]) -> ExitCode {
    run_at_with(args, Path::new(ROOT), bootstrap)
}

fn run_at_with<F>(args: &[String], root: &Path, bootstrap: F) -> ExitCode
where
    F: FnOnce(&Path, &str, &str, &str, &Path, bool) -> Result<(), String>,
{
    run_at_with_tool_probe(args, root, which, bootstrap)
}

fn run_at_with_tool_probe<F, W>(
    args: &[String],
    root: &Path,
    tool_on_path: W,
    bootstrap: F,
) -> ExitCode
where
    F: FnOnce(&Path, &str, &str, &str, &Path, bool) -> Result<(), String>,
    W: Fn(&str) -> bool,
{
    let [name, owner] = args else {
        eprintln!("usage: init <project-name> <github-owner>  (example: just init my-cli octocat)");
        return ExitCode::from(2);
    };
    // GitHub repository names often carry capitals (e.g. "Cadmus") while crate
    // names must not — normalize case instead of rejecting; other invalid
    // characters still fail the kebab-case check below.
    let name = &name.to_lowercase();
    if name.len() > MAX_CRATE_NAME_LEN {
        return fail(
            &format!(
                "project name is {} bytes; the limit is {MAX_CRATE_NAME_LEN}",
                name.len()
            ),
            "a bounded package name stays portable across filesystems and registries",
            "choose a shorter kebab-case name",
        );
    }
    if !is_kebab_case(name) {
        return fail(
            &format!("invalid project name: '{name}'"),
            "the name doubles as the crate and binary name, so it must be kebab-case",
            "lowercase letters, digits and hyphens only, e.g. my-cli",
        );
    }
    if is_rust_keyword(name) {
        return fail(
            &format!("invalid project name: '{name}'"),
            "the generated Rust source refers to the crate by its plain identifier, which cannot be a Rust keyword",
            "choose a non-keyword name such as my-cli",
        );
    }
    if !is_github_owner(owner) {
        return fail(
            &format!("invalid GitHub owner: '{owner}'"),
            "GitHub user/org names allow only letters, digits and hyphens and are at most 39 bytes",
            "e.g. octocat or my-org",
        );
    }
    let placeholder_dir = root.join("crates/__project_name__");
    if !placeholder_dir.is_dir() {
        return fail(
            "crates/__project_name__ not found",
            "this repository looks already initialized — init can only run once",
            "to start over: nix flake init -t <template-repo> into a fresh directory",
        );
    }

    let target_dir = root.join("crates").join(name);
    match std::fs::symlink_metadata(&target_dir) {
        Ok(_) => {
            return fail(
                &format!("target crate directory already exists: crates/{name}"),
                "init must not overwrite an existing crate (including the xtask tooling crate)",
                "choose another project name",
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return fail(
                "cannot inspect the target crate directory",
                &format!("{}: {err}", target_dir.display()),
                "check the path permissions before running init",
            );
        }
    }
    let has_git = match ensure_clean_existing_repo(root) {
        Ok(has_git) => has_git,
        Err(err) => {
            return fail(
                "existing git repository is not clean and readable",
                &err,
                "use a pristine clone; init refuses to overwrite or stage unrelated work",
            );
        }
    };
    for tool in ["git", "prek"] {
        if !tool_on_path(tool) {
            return fail(
                &format!("required tool not found on PATH: {tool}"),
                "init stages the tree and installs git hooks after rewriting it",
                "run through `nix develop` / `nix run`, or install the missing tool first",
            );
        }
    }
    let snake = name.replace('-', "_");
    println!("==> project: {name} (rust crate: {snake})   owner: {owner}");
    if let Err(err) = bootstrap(root, name, &snake, owner, &placeholder_dir, has_git) {
        eprintln!("error: init failed without rollback: {err}");
        return ExitCode::FAILURE;
    }

    report_success(name, owner);
    ExitCode::SUCCESS
}

fn report_success(name: &str, owner: &str) {
    println!(
        "
✅ {name} initialized — zero template residue, the whole tree staged, and the
   git hooks armed. Existing history is preserved when present; no commit was
   created for you.

Next steps:
  1. Review the staged tree        git status && git diff --cached --stat
  2. Commit the initialized tree   git commit -m \"chore: initial commit\"
     (hooks fire on it: just fmt + just lint + the commit-msg convention check)
  3. Run the core quality gate     just ci
  4. Fill in the crate description: the description field in
     crates/{name}/Cargo.toml (feeds --help and crates.io)
  5. Check the GitHub remote       git remote -v
     (if absent: gh repo create {owner}/{name} --public --source=. --push)
  6. Lock the repo down            see CONTRIBUTING.md \"Repository settings\"
     (squash-only merge + branch protection — the hard boundary that keeps
     both humans and agents from merging red code)
  7. Release setup                 see CONTRIBUTING.md \"Releasing\"
     (GitHub Actions permissions; crates.io publishing stays off until you opt in)
"
    );
}

fn bootstrap(
    root: &Path,
    name: &str,
    snake: &str,
    owner: &str,
    placeholder_dir: &Path,
    has_git: bool,
) -> Result<(), String> {
    // Validate every fenced block before the first write. A malformed marker
    // must never turn into silent deletion halfway through bootstrap.
    for rel in TEMPLATE_STRIP_FILES {
        let path = root.join(rel);
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        template_only_stripped(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    }

    // 1. Placeholder substitution: snake_case in .rs files (Rust crate
    //    references), kebab-case everywhere else.
    let mut files = Vec::new();
    walk(root, &mut files)?;
    substitute(&files, "__project_name__", snake, true)?;
    substitute(&files, "__project_name__", name, false)?;
    substitute(&files, "__GITHUB_OWNER__", owner, false)?;
    std::fs::rename(placeholder_dir, root.join("crates").join(name))
        .map_err(|err| format!("rename crate dir: {err}"))?;

    // 2. Remove template artifacts (blocks fenced by template-only markers
    //    + template-only files, including this module).
    println!("==> stripping template-only parts");
    for rel in TEMPLATE_STRIP_FILES {
        strip_template_only(&root.join(rel))?;
    }
    remove_file(&root.join("docs/engspec.md"))?;
    remove_file(&root.join(".github/workflows/init.yml"))?;
    remove_file(&root.join(SELF_REL))?;

    // 3. Project README (replaces the template README), English canonical + zh-CN.
    for (rel, template) in [
        ("README.md", README_TEMPLATE),
        ("README.zh-CN.md", README_ZH_TEMPLATE),
    ] {
        let content = template
            .replace("@PROJECT@", name)
            .replace("@OWNER@", owner);
        std::fs::write(root.join(rel), content).map_err(|err| format!("write {rel}: {err}"))?;
    }

    // 4. Reset ADRs to a fresh starting point (the template's ADRs stay with
    //    the template — they document template decisions, not yours).
    let decisions = root.join("docs/decisions");
    for entry in read_dir(&decisions)? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            std::fs::remove_file(path).map_err(|err| format!("remove ADR: {err}"))?;
        }
    }
    let today = civil_from_days(epoch_days_now());
    std::fs::write(
        decisions.join("0001-record-architecture-decisions.md"),
        ADR_TEMPLATE.replace("@DATE@", &today),
    )
    .map_err(|err| format!("write ADR: {err}"))?;

    // 5. Rebuild the lockfile, prove the generated tree is residue-free, then
    //    git init (only when absent) + stage. Deliberately NO commit: the
    //    project's history is authored by its owner.
    println!("==> regenerating Cargo.lock");
    run_cmd("cargo", &["generate-lockfile"], root)?;
    files.clear();
    walk(root, &mut files)?;
    ensure_no_template_residue(&files)?;
    if !has_git {
        run_cmd("git", &["init", "-b", "main"], root)?;
    }
    run_cmd("git", &["add", "-A"], root)?;

    // Arm the git hooks now so the creator has zero setup steps left (the
    // devShell shellHook does the same for every future contributor). `prek`
    // was checked before any write, so success here means hooks really exist.
    run_cmd(
        "prek",
        &[
            "install",
            "--hook-type",
            "pre-commit",
            "--hook-type",
            "commit-msg",
        ],
        root,
    )?;
    Ok(())
}

/// An existing clone must be pristine: bootstrap rewrites tracked files and
/// stages the whole tree, so unrelated staged, unstaged or untracked work is a
/// hard stop. Git failures are not an unborn-branch signal — they fail closed.
fn ensure_clean_existing_repo(root: &Path) -> Result<bool, String> {
    let has_git = match std::fs::symlink_metadata(root.join(".git")) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(format!("cannot inspect .git metadata: {err}")),
    };
    if !has_git {
        return Ok(false);
    }
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .map_err(|err| format!("spawn git status: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !output.stdout.is_empty() {
        return Err("staged, unstaged or untracked files are present".to_string());
    }
    Ok(true)
}

/// Three-part bootstrap failure: what happened / why / what to do.
fn fail(what: &str, why: &str, how: &str) -> ExitCode {
    eprintln!("error: {what}\n  -> why: {why}\n  -> how: {how}");
    ExitCode::FAILURE
}

/// `[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?` — GitHub user/org names.
fn is_github_owner(owner: &str) -> bool {
    let bytes = owner.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Recursive file listing, skipping symlinks and [`SKIP_DIRS`]
/// (`grep -rlI --exclude-dir` equivalent).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in read_dir(dir)? {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("file type of {}: {err}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            if !SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                walk(&path, out)?;
            }
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn read_dir(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|err| format!("read {}: {err}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("read {}: {err}", dir.display()))?;
    Ok(entries)
}

/// Replace `pattern` in every text file (binary files are skipped, like
/// `grep -I`). `only_rs` restricts to Rust sources for the `snake_case` pass.
fn substitute(
    files: &[PathBuf],
    pattern: &str,
    replacement: &str,
    only_rs: bool,
) -> Result<(), String> {
    for path in files {
        if path.ends_with(SELF_REL) {
            continue;
        }
        if only_rs && path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let bytes = std::fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if text.contains(pattern) {
            std::fs::write(path, text.replace(pattern, replacement))
                .map_err(|err| format!("write {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

/// Delete every line between `>>> template-only` and `<<< template-only`
/// markers inclusive (`sed '/>>>/,/<<</d'` equivalent; the marker works with
/// any comment prefix — `#` in justfile/nix/YAML, `//` in Rust).
fn strip_template_only(path: &Path) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let out = template_only_stripped(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    std::fs::write(path, out).map_err(|err| format!("write {}: {err}", path.display()))
}

/// Pure marker parser, separate from writing so bootstrap can validate every
/// fenced file before modifying the tree.
fn template_only_stripped(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let starts = line.contains(">>> template-only");
        let ends = line.contains("<<< template-only");
        let line_number = index + 1;
        match (starts, ends, inside) {
            (true, true, _) => {
                return Err(format!(
                    "line {line_number}: start and end template-only markers share one line"
                ));
            }
            (true, false, true) => {
                return Err(format!(
                    "line {line_number}: nested template-only start marker"
                ));
            }
            (false, true, false) => {
                return Err(format!(
                    "line {line_number}: template-only end marker has no start"
                ));
            }
            (true, false, false) => inside = true,
            (false, true, true) => inside = false,
            (false, false, false) => out.push_str(line),
            (false, false, true) => {}
        }
    }
    if inside {
        return Err("template-only start marker is not closed".to_string());
    }
    Ok(out)
}

/// Postcondition for every bootstrap path, not only the CI smoke copy.
fn ensure_no_template_residue(files: &[PathBuf]) -> Result<(), String> {
    for path in files {
        let bytes = std::fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if let Some(marker) = TEMPLATE_RESIDUE
            .iter()
            .find(|marker| text.contains(**marker))
        {
            return Err(format!(
                "template residue {marker:?} remains in {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove {}: {err}", path.display())),
    }
}

fn run_cmd(program: &str, args: &[&str], dir: &Path) -> Result<(), String> {
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|err| format!("spawn {program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// Poor-man's `which(1)`: is `program` (or `program.exe`) on `PATH`?
fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
    })
}

/// Days since 1970-01-01 (UTC), read once at the edge so the calendar math
/// stays pure and unit-testable (workspace style: time is injected).
fn epoch_days_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() / 86_400)
}

/// Calendar date (`YYYY-MM-DD`, UTC) for `days` since 1970-01-01 — Howard
/// Hinnant's civil-from-days algorithm, in std so `xtask` stays dependency-free.
fn civil_from_days(days: u64) -> String {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("xtask-init-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn entry_point_rejects_invalid_names_before_writing() {
        let root = temp_root("invalid-names");
        let placeholder = root.join("crates/__project_name__");
        std::fs::create_dir_all(&placeholder).unwrap();
        let sentinel = placeholder.join("sentinel.txt");
        std::fs::write(&sentinel, "untouched").unwrap();
        for name in ["type".to_string(), "a".repeat(MAX_CRATE_NAME_LEN + 1)] {
            let args = vec![name, "owner".to_string()];
            assert!(matches!(
                run_at_with(&args, &root, |_, _, _, _, _, _| {
                    panic!("bootstrap must not run for invalid names")
                }),
                ExitCode::FAILURE
            ));
        }
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "untouched");
        assert_eq!(std::fs::read_dir(placeholder).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dirty_existing_repo_is_rejected_without_touching_user_work() {
        let root = temp_root("dirty-repo");
        std::fs::create_dir_all(root.join("crates/__project_name__")).unwrap();
        let readme = root.join("README.md");
        std::fs::write(&readme, "user work").unwrap();
        let initialized = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(initialized.status.success());

        assert!(ensure_clean_existing_repo(&root).is_err());
        let args = vec!["dirty-app".to_string(), "owner".to_string()];
        assert!(matches!(
            run_at_with(&args, &root, |_, _, _, _, _, _| {
                panic!("bootstrap must not run for a dirty repository")
            }),
            ExitCode::FAILURE
        ));
        assert_eq!(std::fs::read_to_string(&readme).unwrap(), "user work");
        assert!(!root.join("crates/dirty-app").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_probe_distinguishes_absent_clean_and_broken_repositories() {
        let absent = temp_root("no-git");
        std::fs::create_dir_all(&absent).unwrap();
        assert!(!ensure_clean_existing_repo(&absent).unwrap());
        std::fs::remove_dir_all(absent).unwrap();

        let clean = temp_root("clean-git");
        std::fs::create_dir_all(&clean).unwrap();
        let initialized = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&clean)
            .output()
            .unwrap();
        assert!(initialized.status.success());
        assert!(ensure_clean_existing_repo(&clean).unwrap());
        std::fs::remove_dir_all(clean).unwrap();

        let broken = temp_root("broken-git");
        std::fs::create_dir_all(broken.join("crates/__project_name__")).unwrap();
        std::fs::write(broken.join(".git"), "not a gitdir").unwrap();
        assert!(ensure_clean_existing_repo(&broken).is_err());
        let args = vec!["broken-app".to_string(), "owner".to_string()];
        assert!(matches!(
            run_at_with(&args, &broken, |_, _, _, _, _, _| {
                panic!("bootstrap must not run for an unreadable repository")
            }),
            ExitCode::FAILURE
        ));
        std::fs::remove_dir_all(broken).unwrap();
    }

    #[test]
    fn successful_bootstrap_receives_the_probed_git_state() {
        for expected in [false, true] {
            let root = temp_root(if expected { "with-git" } else { "without-git" });
            std::fs::create_dir_all(root.join("crates/__project_name__")).unwrap();
            if expected {
                let initialized = std::process::Command::new("git")
                    .args(["init", "-b", "main"])
                    .current_dir(&root)
                    .output()
                    .unwrap();
                assert!(initialized.status.success());
                // Keep the fixture commit-free: ignoring the scaffold makes the
                // existing repository clean without changing init's input tree.
                std::fs::write(root.join(".git/info/exclude"), "crates/\n").unwrap();
            }
            let args = vec!["demo".to_string(), "owner".to_string()];
            assert!(matches!(
                run_at_with_tool_probe(
                    &args,
                    &root,
                    |_| true,
                    |_, _, _, _, _, actual| {
                        assert_eq!(actual, expected);
                        Ok(())
                    }
                ),
                ExitCode::SUCCESS
            ));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn residue_check_rejects_every_template_marker() {
        let root = temp_root("residue");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("sample.txt");
        for marker in [
            "__project_name__",
            "__GITHUB_OWNER__",
            ">>> template-only",
            "<<< template-only",
        ] {
            std::fs::write(&file, format!("before {marker} after")).unwrap();
            assert!(ensure_no_template_residue(std::slice::from_ref(&file)).is_err());
        }
        std::fs::write(&file, "generated project").unwrap();
        assert!(ensure_no_template_residue(std::slice::from_ref(&file)).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(20_089), "2025-01-01");
    }

    #[test]
    fn github_owner_validation() {
        assert!(is_github_owner("octocat"));
        assert!(is_github_owner("my-org"));
        assert!(is_github_owner("a"));
        assert!(!is_github_owner("-bad"));
        assert!(!is_github_owner("bad-"));
        assert!(!is_github_owner("bad_name"));
        assert!(!is_github_owner(""));
        assert!(!is_github_owner(&"a".repeat(40)));
    }

    #[test]
    fn template_only_parser_rejects_unbalanced_markers() {
        for text in [
            "keep\n# >>> template-only\ndrop\n",
            "keep\n# <<< template-only\n",
            "# >>> template-only\n# >>> template-only\n# <<< template-only\n",
            "# >>> template-only # <<< template-only\n",
        ] {
            assert!(template_only_stripped(text).is_err(), "accepted {text:?}");
        }
        assert_eq!(
            template_only_stripped("plain\ntext\n").unwrap(),
            "plain\ntext\n"
        );
    }

    #[test]
    fn template_only_strip_handles_both_comment_styles() {
        let dir = std::env::temp_dir().join("xtask-strip-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");
        std::fs::write(
            &file,
            "keep1\n# >>> template-only\ndrop\n// <<< template-only\nkeep2\n",
        )
        .unwrap();
        strip_template_only(&file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep1\nkeep2\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
