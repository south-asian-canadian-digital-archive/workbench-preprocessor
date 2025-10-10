# Release Checklist

Use this checklist when preparing a new release.

## Pre-Release

- [ ] All tests pass locally (`cargo test`)
- [ ] Code is properly formatted (`cargo fmt --all -- --check`)
- [ ] No clippy warnings (`cargo clippy --all-targets --all-features -- -D warnings`)
- [ ] Update version in `Cargo.toml`
- [ ] Update `CHANGELOG.md` (if you have one) with new features/fixes
- [ ] Update README.md if there are new features or breaking changes
- [ ] Commit all changes to `main` branch

## Release Process

1. **Create a version tag**:
   ```bash
   git tag v1.0.0
   git push origin v1.0.0
   ```

2. **Monitor GitHub Actions**:
   - Go to the Actions tab on GitHub
   - Watch the "Release" workflow complete
   - Verify that builds for both Linux and Windows succeed

3. **Verify the Release**:
   - Go to the Releases page
   - Verify the new release was created
   - Download and test both binaries:
     - `workbench-preprocessor-linux-x86_64.tar.gz`
     - `workbench-preprocessor-windows-x86_64.zip`

4. **Update Release Notes** (optional):
   - Edit the release on GitHub
   - Add detailed release notes if needed
   - Highlight breaking changes, new features, and bug fixes

## Post-Release

- [ ] Announce the release (if applicable)
- [ ] Update documentation website (if applicable)
- [ ] Create a new branch for the next version (if using gitflow)

## Version Numbering

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR** version (1.0.0): Incompatible API changes
- **MINOR** version (0.1.0): Add functionality in a backwards compatible manner  
- **PATCH** version (0.0.1): Backwards compatible bug fixes

## Rollback

If you need to remove a bad release:

1. Delete the tag locally and remotely:
   ```bash
   git tag -d v1.0.0
   git push origin :refs/tags/v1.0.0
   ```

2. Delete the GitHub Release through the web interface

3. Fix the issues and create a new patch version
