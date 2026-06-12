# Files reviewed by human

Checklist for manually reviewing the project's source, test, and vendored files.
Tick each file once you have read and approved it. Files marked **★** were changed
in the latest code review (`git diff <file>` to see what changed).

## src/ top level

- [x] [src/lib.rs](src/lib.rs)
- [x] [src/main.rs](src/main.rs)
- [ ] [src/models.rs](src/models.rs)
- [ ] [src/config.rs](src/config.rs) ★
- [ ] [src/utils.rs](src/utils.rs) ★
- [ ] [src/output.rs](src/output.rs)
- [ ] [src/store.rs](src/store.rs)
- [ ] [src/library.rs](src/library.rs) ★
- [ ] [src/local_library.rs](src/local_library.rs)
- [ ] [src/cloud_sync.rs](src/cloud_sync.rs)
- [ ] [src/cm_list.rs](src/cm_list.rs)
- [ ] [src/depot_browser.rs](src/depot_browser.rs)
- [ ] [src/proton.rs](src/proton.rs) ★
- [ ] [src/relocate.rs](src/relocate.rs)
- [ ] [src/running.rs](src/running.rs)
- [ ] [src/proc_admin.rs](src/proc_admin.rs)
- [ ] [src/steam_client.rs](src/steam_client.rs) ★
- [x] [src/progress.rs](src/progress.rs) ★

## src/steam_client/

- [ ] [src/steam_client/client.rs](src/steam_client/client.rs)
- [ ] [src/steam_client/content.rs](src/steam_client/content.rs) ★
- [ ] [src/steam_client/install.rs](src/steam_client/install.rs) ★
- [ ] [src/steam_client/launch.rs](src/steam_client/launch.rs) ★
- [ ] [src/steam_client/library.rs](src/steam_client/library.rs) ★
- [ ] [src/steam_client/manage.rs](src/steam_client/manage.rs)
- [ ] [src/steam_client/manifests.rs](src/steam_client/manifests.rs) ★
- [ ] [src/steam_client/process.rs](src/steam_client/process.rs) ★
- [ ] [src/steam_client/workshop.rs](src/steam_client/workshop.rs)
- [ ] [src/steam_client/workshop_manifest.rs](src/steam_client/workshop_manifest.rs)

## src/daemon/

- [ ] [src/daemon/mod.rs](src/daemon/mod.rs)
- [ ] [src/daemon/client.rs](src/daemon/client.rs) ★
- [ ] [src/daemon/server.rs](src/daemon/server.rs)
- [ ] [src/daemon/proto.rs](src/daemon/proto.rs) ★
- [ ] [src/daemon/transport.rs](src/daemon/transport.rs)

## src/launch/

- [ ] [src/launch/mod.rs](src/launch/mod.rs) ★
- [ ] [src/launch/pipeline.rs](src/launch/pipeline.rs)
- [ ] [src/launch/dll_provider_resolver.rs](src/launch/dll_provider_resolver.rs)
- [ ] [src/launch/verification_tests.rs](src/launch/verification_tests.rs)
- [ ] [src/launch/stages/mod.rs](src/launch/stages/mod.rs)
- [ ] [src/launch/stages/preflight.rs](src/launch/stages/preflight.rs)
- [ ] [src/launch/stages/resolve_game.rs](src/launch/stages/resolve_game.rs)
- [ ] [src/launch/stages/resolve_profile.rs](src/launch/stages/resolve_profile.rs)
- [ ] [src/launch/stages/resolve_components.rs](src/launch/stages/resolve_components.rs)
- [ ] [src/launch/stages/resolve_dll_providers.rs](src/launch/stages/resolve_dll_providers.rs)
- [ ] [src/launch/stages/prepare_prefix.rs](src/launch/stages/prepare_prefix.rs)
- [ ] [src/launch/stages/build_environment.rs](src/launch/stages/build_environment.rs)
- [ ] [src/launch/stages/build_command.rs](src/launch/stages/build_command.rs)
- [ ] [src/launch/stages/spawn_process.rs](src/launch/stages/spawn_process.rs)
- [ ] [src/launch/stages/finalize.rs](src/launch/stages/finalize.rs)
- [ ] [src/launch/validators/mod.rs](src/launch/validators/mod.rs)
- [ ] [src/launch/validators/invariants.rs](src/launch/validators/invariants.rs)
- [ ] [src/launch/validators/overrides.rs](src/launch/validators/overrides.rs)

## src/infra/

- [ ] [src/infra/mod.rs](src/infra/mod.rs)
- [ ] [src/infra/runners/mod.rs](src/infra/runners/mod.rs)
- [ ] [src/infra/runners/trait.rs](src/infra/runners/trait.rs)
- [ ] [src/infra/runners/wine_tkg.rs](src/infra/runners/wine_tkg.rs) ★
- [ ] [src/infra/runners/tests.rs](src/infra/runners/tests.rs)
- [ ] [src/infra/logging/mod.rs](src/infra/logging/mod.rs) ★
- [ ] [src/infra/logging/cli.rs](src/infra/logging/cli.rs)
- [ ] [src/infra/logging/session.rs](src/infra/logging/session.rs) ★
- [ ] [src/infra/logging/event_log.rs](src/infra/logging/event_log.rs) ★
- [ ] [src/infra/logging/wine_capture.rs](src/infra/logging/wine_capture.rs)
- [ ] [src/infra/logging/debug_utils.rs](src/infra/logging/debug_utils.rs)
- [ ] [src/infra/logging/tests.rs](src/infra/logging/tests.rs)

## tests/

- [ ] [tests/compat_discovery.rs](tests/compat_discovery.rs)
- [ ] [tests/compatibility_validator_tests.rs](tests/compatibility_validator_tests.rs)
- [ ] [tests/dll_override_tests.rs](tests/dll_override_tests.rs)
- [ ] [tests/dll_resolution_report.rs](tests/dll_resolution_report.rs)
- [ ] [tests/dxvk_evidence_tests.rs](tests/dxvk_evidence_tests.rs)
- [ ] [tests/launch_summary_tests.rs](tests/launch_summary_tests.rs)
- [ ] [tests/path_resolution.rs](tests/path_resolution.rs)
- [ ] [tests/preflight_integration_tests.rs](tests/preflight_integration_tests.rs)
- [ ] [tests/runner_root_derivation.rs](tests/runner_root_derivation.rs)
- [ ] [tests/staged_launch_failure_tests.rs](tests/staged_launch_failure_tests.rs)
- [ ] [tests/symlink_deployment.rs](tests/symlink_deployment.rs)

## vendor/steam-cdn/

- [ ] [vendor/steam-cdn/src/lib.rs](vendor/steam-cdn/src/lib.rs)
- [ ] [vendor/steam-cdn/src/error.rs](vendor/steam-cdn/src/error.rs)
- [ ] [vendor/steam-cdn/examples/download_manifest.rs](vendor/steam-cdn/examples/download_manifest.rs)
- [ ] [vendor/steam-cdn/src/cdn/mod.rs](vendor/steam-cdn/src/cdn/mod.rs)
- [ ] [vendor/steam-cdn/src/cdn/depot.rs](vendor/steam-cdn/src/cdn/depot.rs)
- [ ] [vendor/steam-cdn/src/cdn/inner.rs](vendor/steam-cdn/src/cdn/inner.rs)
- [ ] [vendor/steam-cdn/src/cdn/depot_chunk/mod.rs](vendor/steam-cdn/src/cdn/depot_chunk/mod.rs)
- [ ] [vendor/steam-cdn/src/cdn/manifest/mod.rs](vendor/steam-cdn/src/cdn/manifest/mod.rs)
- [ ] [vendor/steam-cdn/src/cdn/manifest/buf.rs](vendor/steam-cdn/src/cdn/manifest/buf.rs)
- [ ] [vendor/steam-cdn/src/cdn/manifest/error.rs](vendor/steam-cdn/src/cdn/manifest/error.rs)
- [ ] [vendor/steam-cdn/src/cdn/manifest/file.rs](vendor/steam-cdn/src/cdn/manifest/file.rs)
- [ ] [vendor/steam-cdn/src/crypto/mod.rs](vendor/steam-cdn/src/crypto/mod.rs)
- [ ] [vendor/steam-cdn/src/crypto/aes256.rs](vendor/steam-cdn/src/crypto/aes256.rs)
- [ ] [vendor/steam-cdn/src/utils/mod.rs](vendor/steam-cdn/src/utils/mod.rs)
- [ ] [vendor/steam-cdn/src/utils/base64.rs](vendor/steam-cdn/src/utils/base64.rs)
- [ ] [vendor/steam-cdn/src/utils/lzma.rs](vendor/steam-cdn/src/utils/lzma.rs)
- [ ] [vendor/steam-cdn/src/web_api/mod.rs](vendor/steam-cdn/src/web_api/mod.rs)
- [ ] [vendor/steam-cdn/src/web_api/content_service.rs](vendor/steam-cdn/src/web_api/content_service.rs)

---

**★ Changed in the latest code review** (see [README.md](README.md) and `git diff`):
security hardening (zip-slip guard, `session.json` perms, daemon frame cap, appmanifest
sanitization, unified log redaction, unsafe-fn fix) plus deduplication, regex caching, and
`tracing` cleanups.

**Verification status:** `cargo build` clean; 121 tests pass on Linux and 119 on Windows.