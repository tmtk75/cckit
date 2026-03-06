---
name: release-cckit
description: Release cckit by creating a git tag that matches Cargo.toml version. Use when the user says "cckit release", "cckitリリース", "cckitのタグ打って", "release cckit", or "cckitをpublish".
---

This skill guides the release process for cckit.

## Release Flow

1. Read `Cargo.toml` and extract the current `version`
2. Check if a git tag `v{version}` already exists (`git tag -l`)
3. If the tag exists, inform the user and stop
4. Show the user what will be released:
   - Version: `v{version}`
   - Current branch and latest commits (`git log --oneline -5`)
   - Uncommitted changes (`git status --short`)
5. Ask the user for confirmation before proceeding
6. Create and push the tag:
   ```
   git tag v{version}
   git push origin v{version}
   ```

## What Happens After Tagging

The `.github/workflows/release.yml` workflow will automatically:

1. **verify-version** — Confirm the tag matches `Cargo.toml` version
2. **build** — Build release binaries for `aarch64-apple-darwin` and `x86_64-apple-darwin`
3. **release** — Create a GitHub Release with `.tar.gz` artifacts and auto-generated notes

## Important

- Never bump the version in `Cargo.toml` unless the user explicitly asks
- Always confirm with the user before `git tag` and `git push`
- If there are uncommitted changes, warn the user before proceeding
