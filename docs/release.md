# Cutting a release

Releases are cut from `main` by pushing a `vX.Y.Z` tag. The workflow
`.github/workflows/release.yml` builds the binaries and creates the GitHub
release. Nothing is published to crates.io automatically.

## 1. Prepare

1. Bump the version in the root `Cargo.toml`:
   - `[workspace.package] version = "X.Y.Z"`
   - the `version = "X.Y.Z"` of every internal `orq-*` entry in
     `[workspace.dependencies]` (they must match, otherwise packaging fails).
2. Run `cargo check --workspace` so `Cargo.lock` is updated, and commit it.
3. In `CHANGELOG.md`, add a `## [X.Y.Z] - YYYY-MM-DD` section. The release
   workflow uses exactly this section as the release notes and fails if it is
   missing.
4. Verify locally:
   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo package --workspace --no-verify
   ```
   `cargo package --workspace --no-verify` packages all crates together using a
   local registry overlay (cargo 1.90 or newer), so it works even though the
   internal dependencies are not on crates.io yet. The CI job `package` runs the
   same command on every push.

## 2. Tag and push

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```

## 3. Check the workflow

The `Release` workflow runs for the tag:

- `build` compiles `orq` in release for `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu` (native arm runner), `x86_64-apple-darwin` and
  `aarch64-apple-darwin`, and packs each one as
  `orq-X.Y.Z-<target>.tar.gz` with a `.sha256` file.
- `release` creates the GitHub release with the CHANGELOG notes and uploads all
  tarballs and checksums.

Download one asset and run `orq --version` to confirm.

## 4. Publish to crates.io (manual, optional)

The crate name `orq` is taken, so the install command is
`cargo install orq-cli` (it provides the `orq` binary). Publishing is a manual
step and is disabled by default.

Order matters, each crate must be on the index before its dependents:

1. `orq-core`
2. `orq-impact`, `orq-runner`, `orq-compress`, `orq-cache`
3. `orq-daemon` (needs impact, runner, cache, compress)
4. `orq-mcp` (needs daemon)
5. `orq-cli`

Two ways to do it:

- In GitHub: add the repository secret `CARGO_REGISTRY_TOKEN`, then run the
  `Release` workflow manually (Actions, Release, Run workflow) on the tag with
  `publish_crates` ticked. The `publish-crates` job publishes in the order above.
- Locally: `cargo login`, then `cargo publish -p <crate>` for each crate in the
  order above, waiting for the index to update between them.

Publishing is irreversible (versions can only be yanked), so run
`cargo package --workspace --no-verify` first.

## 5. Update the Homebrew formula

The template is `packaging/homebrew/orq.rb`. Copy it to `Formula/orq.rb` of the
tap repository and, for each release:

1. Set `version` to `X.Y.Z`.
2. For each of the four targets, take the hash from the release asset
   `orq-X.Y.Z-<target>.tar.gz.sha256` (first field) and replace the matching
   `REPLACE_WITH_SHA256_*` value.
3. Commit and push the tap. Test with `brew install --build-from-source`
   or `brew install circe07/tap/orq` followed by `brew test orq`.
