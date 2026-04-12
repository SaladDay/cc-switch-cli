# CC Switch CLI v5.3.1

> Patch release focused on Codex official auth safety and newer database compatibility.

**[中文版本 →](release-note-v5.3.1-zh.md)**

---

## Highlights

**Release Date**: 2026-04-12

- **Codex Official Provider Safety**: Editing an official Codex provider now preserves the stored auth snapshot instead of rebuilding it like a third-party relay config.
- **Database Schema v8 Compatibility**: The backend now accepts newer schema v8 databases and upgrades older local databases through the staged v6 -> v7 -> v8 migration path.
- **Consistent Pricing Data**: Corrected pricing values are applied both during migration and when seeding a fresh database, so imported and newly initialized databases stay aligned.

## Fixed

- **Issue #102**: Prevented the official Codex provider flow from reintroducing provider-local `base_url` state and dropping official auth snapshots.
- **Issue #106**: Removed the blocking “database version too new (8)” failure by updating the supported schema version and migration chain.

## Notes

- This patch is intentionally small. It does not introduce a new feature wave; it closes compatibility gaps in the current 5.3 line.
- Existing databases still migrate in place. Newly created databases start directly with the latest schema expected by this release.

## Thanks

- Thanks `@saladday` for carrying the backend alignment and patch-release preparation work.
- Thanks `@aldev814` and `@Hatiaa` for reporting the migration and official-provider failures quickly, which helped narrow the fix scope.
- Thanks to everyone who kept testing the 5.3 line while these compatibility fixes were landing.

## Links

- [README](../README.md)
- [CHANGELOG](../CHANGELOG.md)
