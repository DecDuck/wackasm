---
title: Releasing
description: What the GitHub workflow builds and publishes, and how to cut a release.
sidebar:
  order: 3
---

`.github/workflows/release.yml` does everything.

| when | what |
|---|---|
| every push and pull request | builds the native library for every platform, runs the tests, builds the mod jar and uploads it as a workflow artifact, and builds these docs |
| a push to `master` | also publishes these docs to GitHub Pages |
| a tag `v*` | also creates a GitHub release with the jar, publishes `wackasm-opencomputers` to crates.io (unless that version is already there), and publishes the docs |

The native library is built on each platform's own runner (Windows x86_64,
Linux x86_64 and aarch64, macOS aarch64, and macOS x86_64 cross-compiled
from Apple silicon). The jar job puts them in `mc-mod/natives-extra/` before
building, so the jar runs everywhere.

## Cutting a release

1. Set the versions:
   - the mod: `mod_version` in `mc-mod/gradle.properties`. The workflow
     overrides it with the tag's version, so the jar always matches the tag;
   - the crate: `version` in `crates/wackasm-opencomputers/Cargo.toml`. It's
     published only if crates.io doesn't have that version yet, so a release
     that doesn't change the crate leaves it alone.
2. Commit, then tag and push:
   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

## One-time setup

- **crates.io:** create an API token with the `publish-new` and
  `publish-update` scopes. In the repository's settings, under
  Environments, create an environment named `release` and add the token to it
  as the secret `CARGO_REGISTRY_TOKEN`. The publish job runs in that
  environment. If you restrict which branches and tags may deploy to it, allow
  `v*` tags, because releases run from a tag. Any protection rules you add,
  such as required reviewers, then gate every crates.io publish.
- **GitHub Pages:** in the repository's settings, under Pages, set the source
  to **GitHub Actions**.
- Releases need nothing extra: the workflow uses its own `GITHUB_TOKEN`.

Only `wackasm-opencomputers` is published. The interpreter crates build
against a patched `wasmparser`, which crates.io can't carry, so they're
marked `publish = false`.
