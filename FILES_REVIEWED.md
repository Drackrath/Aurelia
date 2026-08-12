# Files reviewed by human

Provenance checklist covering every file tracked in the repository: source, tests,
vendored crates, packaging, and documentation. Tick one column per file. Every file
is marked **★**, so the whole tree is in scope for the latest code review
(`git diff <file>` to see what changed).

Files we author are classified by how they were written. Vendored files are
third-party by definition, so they get their own two columns instead.

| Column | Applies to | Meaning |
|---|---|---|
| **Manual** | Authored | Written by hand, no model involved |
| **AI Assisted** | Authored | Drafted or edited with a model, then read and approved line by line |
| **Vibecoded** | Authored | Produced by a model and accepted without a full line-by-line read |
| **Vendored** | Third-party | Copied into the tree from upstream, not authored here |
| **Altered** | Third-party | Diverges from upstream, we patched it locally |

Total tracked files: **198**. Vendored files are pre-ticked as vendored.
Tick a box by replacing `⬜` with `✅`.

## Project documentation (9)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) ★ | ⬜ | ⬜ | ⬜ |
| [CONTRIBUTING.md](CONTRIBUTING.md) ★ | ⬜ | ⬜ | ⬜ |
| [FILES_REVIEWED.md](FILES_REVIEWED.md) ★ | ⬜ | ⬜ | ⬜ |
| [LICENSE](LICENSE) ★ | ⬜ | ⬜ | ⬜ |
| [README.md](README.md) ★ | ⬜ | ⬜ | ⬜ |
| [RELEASE.md](RELEASE.md) ★ | ⬜ | ⬜ | ⬜ |
| [SECURITY.md](SECURITY.md) ★ | ⬜ | ⬜ | ⬜ |
| [USAGE.md](USAGE.md) ★ | ⬜ | ⬜ | ⬜ |
| [WINDOWS_STEAM_RUNTIME.md](WINDOWS_STEAM_RUNTIME.md) ★ | ⬜ | ⬜ | ⬜ |

## Repository configuration (5)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [.gitignore](.gitignore) ★ | ⬜ | ⬜ | ⬜ |
| [.github/FUNDING.yml](.github/FUNDING.yml) ★ | ⬜ | ⬜ | ⬜ |
| [.github/ISSUE_TEMPLATE/bug_report.md](.github/ISSUE_TEMPLATE/bug_report.md) ★ | ⬜ | ⬜ | ⬜ |
| [.github/ISSUE_TEMPLATE/feature_request.md](.github/ISSUE_TEMPLATE/feature_request.md) ★ | ⬜ | ⬜ | ⬜ |
| [.github/workflows/release.yml](.github/workflows/release.yml) ★ | ⬜ | ⬜ | ⬜ |

## Build and packaging (9)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [Cargo.lock](Cargo.lock) ★ | ⬜ | ⬜ | ⬜ |
| [Cargo.toml](Cargo.toml) ★ | ⬜ | ⬜ | ⬜ |
| [build.rs](build.rs) ★ | ⬜ | ⬜ | ⬜ |
| [flake.nix](flake.nix) ★ | ⬜ | ⬜ | ⬜ |
| [assets/asciiart_banner.txt](assets/asciiart_banner.txt) ★ | ⬜ | ⬜ | ⬜ |
| [assets/aurelia_logo.png](assets/aurelia_logo.png) ★ | ⬜ | ⬜ | ⬜ |
| [assets/aurelia_logo_v2.png](assets/aurelia_logo_v2.png) ★ | ⬜ | ⬜ | ⬜ |
| [assets/aurelia_logo_v3.png](assets/aurelia_logo_v3.png) ★ | ⬜ | ⬜ | ⬜ |
| [proto/service_cloudconfigstore.proto](proto/service_cloudconfigstore.proto) ★ | ⬜ | ⬜ | ⬜ |

## src/ top level (6)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/cli.rs](src/cli.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/lib.rs](src/lib.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/main.rs](src/main.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/main_tests.rs](src/main_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client.rs](src/steam_client.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client_tests.rs](src/steam_client_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/commands/ (17)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/commands/auth.rs](src/commands/auth.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/cloud.rs](src/commands/cloud.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/collections.rs](src/commands/collections.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/common.rs](src/commands/common.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/config.rs](src/commands/config.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/info.rs](src/commands/info.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/install.rs](src/commands/install.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/install_downgrade_tests.rs](src/commands/install_downgrade_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/launch.rs](src/commands/launch.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/library.rs](src/commands/library.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/market.rs](src/commands/market.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/mod.rs](src/commands/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/plugins.rs](src/commands/plugins.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/runtimes.rs](src/commands/runtimes.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/scripts.rs](src/commands/scripts.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/social.rs](src/commands/social.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/commands/workshop.rs](src/commands/workshop.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/compat/ (10)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/compat/luxtorpeda.rs](src/compat/luxtorpeda.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/luxtorpeda_tests.rs](src/compat/luxtorpeda_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/mod.rs](src/compat/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/proc_admin.rs](src/compat/proc_admin.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/proc_admin_tests.rs](src/compat/proc_admin_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/proton.rs](src/compat/proton.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/proton_tests.rs](src/compat/proton_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/running.rs](src/compat/running.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/umu.rs](src/compat/umu.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/compat/umu_tests.rs](src/compat/umu_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/core/ (11)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/core/config.rs](src/core/config.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/config_tests.rs](src/core/config_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/mod.rs](src/core/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/models.rs](src/core/models.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/net.rs](src/core/net.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/net_tests.rs](src/core/net_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/output.rs](src/core/output.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/utils.rs](src/core/utils.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/utils_resolve_runner_tests.rs](src/core/utils_resolve_runner_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/utils_runner_classification_tests.rs](src/core/utils_runner_classification_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/core/utils_save_prefix_tests.rs](src/core/utils_save_prefix_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/daemon/ (7)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/daemon/client.rs](src/daemon/client.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/client_tests.rs](src/daemon/client_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/daemon_tests.rs](src/daemon/daemon_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/mod.rs](src/daemon/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/proto.rs](src/daemon/proto.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/server.rs](src/daemon/server.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/daemon/transport.rs](src/daemon/transport.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/infra/ (14)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/infra/mod.rs](src/infra/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/cli.rs](src/infra/logging/cli.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/cli_tests.rs](src/infra/logging/cli_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/debug_utils.rs](src/infra/logging/debug_utils.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/event_log.rs](src/infra/logging/event_log.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/mod.rs](src/infra/logging/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/session.rs](src/infra/logging/session.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/tests.rs](src/infra/logging/tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/logging/wine_capture.rs](src/infra/logging/wine_capture.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/runners/luxtorpeda.rs](src/infra/runners/luxtorpeda.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/runners/mod.rs](src/infra/runners/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/runners/tests.rs](src/infra/runners/tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/runners/trait.rs](src/infra/runners/trait.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/infra/runners/wine_tkg.rs](src/infra/runners/wine_tkg.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/launch/ (28)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/launch/dll_provider_resolver.rs](src/launch/dll_provider_resolver.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/dll_provider_resolver_tests.rs](src/launch/dll_provider_resolver_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/launch_script.rs](src/launch/launch_script.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/launch_script_tests.rs](src/launch/launch_script_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/mod.rs](src/launch/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/pipeline.rs](src/launch/pipeline.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/pipeline_tests.rs](src/launch/pipeline_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/verification_tests.rs](src/launch/verification_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/fixups/fixups_tests.rs](src/launch/fixups/fixups_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/fixups/mod.rs](src/launch/fixups/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/apply_launch_script.rs](src/launch/stages/apply_launch_script.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/apply_launch_script_tests.rs](src/launch/stages/apply_launch_script_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/build_command.rs](src/launch/stages/build_command.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/build_environment.rs](src/launch/stages/build_environment.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/finalize.rs](src/launch/stages/finalize.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/mod.rs](src/launch/stages/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/preflight.rs](src/launch/stages/preflight.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/preflight_tests.rs](src/launch/stages/preflight_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/prepare_prefix.rs](src/launch/stages/prepare_prefix.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/resolve_components.rs](src/launch/stages/resolve_components.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/resolve_dll_providers.rs](src/launch/stages/resolve_dll_providers.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/resolve_game.rs](src/launch/stages/resolve_game.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/resolve_game_fixups.rs](src/launch/stages/resolve_game_fixups.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/resolve_profile.rs](src/launch/stages/resolve_profile.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/stages/spawn_process.rs](src/launch/stages/spawn_process.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/validators/invariants.rs](src/launch/validators/invariants.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/validators/mod.rs](src/launch/validators/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/launch/validators/overrides.rs](src/launch/validators/overrides.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/library/ (11)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/library/cloud_sync.rs](src/library/cloud_sync.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/cloud_sync_tests.rs](src/library/cloud_sync_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/collections.rs](src/library/collections.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/collections_tests.rs](src/library/collections_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/depot_browser.rs](src/library/depot_browser.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/library_tests.rs](src/library/library_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/local_library.rs](src/library/local_library.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/local_library_tests.rs](src/library/local_library_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/mod.rs](src/library/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/relocate.rs](src/library/relocate.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/library/relocate_tests.rs](src/library/relocate_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/steam_client/ (18)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/steam_client/chat.rs](src/steam_client/chat.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/client.rs](src/steam_client/client.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/cloudconfig.rs](src/steam_client/cloudconfig.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/collections.rs](src/steam_client/collections.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/content.rs](src/steam_client/content.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/friends.rs](src/steam_client/friends.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/friends_tests.rs](src/steam_client/friends_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/install.rs](src/steam_client/install.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/launch.rs](src/steam_client/launch.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/library.rs](src/steam_client/library.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/library_update_detection_tests.rs](src/steam_client/library_update_detection_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/manage.rs](src/steam_client/manage.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/manifests.rs](src/steam_client/manifests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/market.rs](src/steam_client/market.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/process.rs](src/steam_client/process.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/workshop.rs](src/steam_client/workshop.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/workshop_manifest.rs](src/steam_client/workshop_manifest.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/steam_client/workshop_manifest_tests.rs](src/steam_client/workshop_manifest_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## src/web/ (10)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [src/web/cm_list.rs](src/web/cm_list.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/mod.rs](src/web/mod.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/openid.rs](src/web/openid.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/openid_tests.rs](src/web/openid_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/steam_urls.rs](src/web/steam_urls.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/steam_urls_tests.rs](src/web/steam_urls_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/store.rs](src/web/store.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/web_session.rs](src/web/web_session.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/web_token.rs](src/web/web_token.rs) ★ | ⬜ | ⬜ | ⬜ |
| [src/web/web_token_tests.rs](src/web/web_token_tests.rs) ★ | ⬜ | ⬜ | ⬜ |

## tests/ (15)

| File | Manual | AI Assisted | Vibecoded |
|---|:---:|:---:|:---:|
| [tests/compat_discovery.rs](tests/compat_discovery.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/compatibility_validator_tests.rs](tests/compatibility_validator_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/config_game_steam_runtime.rs](tests/config_game_steam_runtime.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/config_load_errors.rs](tests/config_load_errors.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/dll_override_tests.rs](tests/dll_override_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/dll_resolution_report.rs](tests/dll_resolution_report.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/dxvk_evidence_tests.rs](tests/dxvk_evidence_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/launch_summary_tests.rs](tests/launch_summary_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/path_resolution.rs](tests/path_resolution.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/preflight_integration_tests.rs](tests/preflight_integration_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/runner_root_derivation.rs](tests/runner_root_derivation.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/staged_launch_failure_tests.rs](tests/staged_launch_failure_tests.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/steam_runtime_install.rs](tests/steam_runtime_install.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/steam_runtime_runner_config.rs](tests/steam_runtime_runner_config.rs) ★ | ⬜ | ⬜ | ⬜ |
| [tests/symlink_deployment.rs](tests/symlink_deployment.rs) ★ | ⬜ | ⬜ | ⬜ |

## vendor/steam-cdn/ (23)

| File | Vendored | Altered |
|---|:---:|:---:|
| [vendor/steam-cdn/Cargo.lock](vendor/steam-cdn/Cargo.lock) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/Cargo.toml](vendor/steam-cdn/Cargo.toml) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/Cargo.toml.orig](vendor/steam-cdn/Cargo.toml.orig) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/LICENSE](vendor/steam-cdn/LICENSE) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/README.md](vendor/steam-cdn/README.md) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/examples/download_manifest.rs](vendor/steam-cdn/examples/download_manifest.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/error.rs](vendor/steam-cdn/src/error.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/lib.rs](vendor/steam-cdn/src/lib.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/depot.rs](vendor/steam-cdn/src/cdn/depot.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/inner.rs](vendor/steam-cdn/src/cdn/inner.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/mod.rs](vendor/steam-cdn/src/cdn/mod.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/crypto/aes256.rs](vendor/steam-cdn/src/crypto/aes256.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/crypto/mod.rs](vendor/steam-cdn/src/crypto/mod.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/utils/base64.rs](vendor/steam-cdn/src/utils/base64.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/utils/lzma.rs](vendor/steam-cdn/src/utils/lzma.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/utils/mod.rs](vendor/steam-cdn/src/utils/mod.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/web_api/content_service.rs](vendor/steam-cdn/src/web_api/content_service.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/web_api/mod.rs](vendor/steam-cdn/src/web_api/mod.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/depot_chunk/mod.rs](vendor/steam-cdn/src/cdn/depot_chunk/mod.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/manifest/buf.rs](vendor/steam-cdn/src/cdn/manifest/buf.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/manifest/error.rs](vendor/steam-cdn/src/cdn/manifest/error.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/manifest/file.rs](vendor/steam-cdn/src/cdn/manifest/file.rs) ★ | ✅ | ⬜ |
| [vendor/steam-cdn/src/cdn/manifest/mod.rs](vendor/steam-cdn/src/cdn/manifest/mod.rs) ★ | ✅ | ⬜ |

## vendor/steam-vent-chat/ (5)

| File | Vendored | Altered |
|---|:---:|:---:|
| [vendor/steam-vent-chat/.gitignore](vendor/steam-vent-chat/.gitignore) ★ | ✅ | ⬜ |
| [vendor/steam-vent-chat/Cargo.lock](vendor/steam-vent-chat/Cargo.lock) ★ | ✅ | ⬜ |
| [vendor/steam-vent-chat/Cargo.toml](vendor/steam-vent-chat/Cargo.toml) ★ | ✅ | ⬜ |
| [vendor/steam-vent-chat/README.md](vendor/steam-vent-chat/README.md) ★ | ✅ | ⬜ |
| [vendor/steam-vent-chat/src/lib.rs](vendor/steam-vent-chat/src/lib.rs) ★ | ✅ | ⬜ |

---

**★ In scope for the latest code review.** Every tracked file carries the mark, so
nothing in the tree is exempt from a manual read-through.
