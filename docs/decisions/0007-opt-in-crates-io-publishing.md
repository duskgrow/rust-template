# 0007. crates.io publishing is opt-in; generated projects release git-only

- Status: accepted
- Date: 2026-08-31

## Context

ADR-0003 wired the release job to publish to crates.io out of the box. The
first real generated project (Cadmus, 2026-08) showed two problems with that
default:

- every push to main failed the release job until the owner completed the
  one-time Trusted Publishing setup — red CI as the very first impression of a
  fresh project;
- worse, the *first* push attempted to publish an undescribed crate
  immediately (`release_always` defaults to true), before any human gate —
  publishing to a public registry is a deliberate per-project decision, not a
  template default.

## Decision

- Generated projects run release-plz with `git_only = true, publish = false`:
  versions come from git tags, no cargo registry is ever contacted, and the
  release workflow holds no crates.io credential or OIDC permission.
- `release_always = false`: only merging the Release PR tags a release, so the
  human gate ADR-0003 describes actually holds for the first release too.
- Opting into crates.io is a documented explicit edit: drop the two config
  lines, add `id-token: write` to the release job, first manual publish + TP
  registration (CONTRIBUTING.md "Releasing").

## Consequences

- A fresh project is green from its first push: Release PR + tag + dist
  GitHub Release need only the GitHub workflow-permission toggle.
- crates.io publishing requires reading CONTRIBUTING.md once — an intentional
  speed bump before publishing to a public registry.
