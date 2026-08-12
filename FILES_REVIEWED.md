# Files reviewed by human

Provenance checklist covering every file tracked in the repository: source, tests,
vendored crates, packaging, and documentation. Tick one column per file. Every file
is marked **★**, so the whole tree is in scope for the latest code review
(`git diff <file>` to see what changed).

| Column | Meaning |
|---|---|
| **Manual** | Written by hand, no model involved |
| **AI Assisted** | Drafted or edited with a model, then read and approved line by line |
| **Vibecoded** | Produced by a model and accepted without a full line-by-line read |
| **Vendored** | Third-party code copied into the tree, not authored here |

Total tracked files: **198**. Vendored files are pre-ticked. Tick a box by
replacing `&#9744;` with `&#9745;`.

## Project documentation (9)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [CONTRIBUTING.md](CONTRIBUTING.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [FILES_REVIEWED.md](FILES_REVIEWED.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [LICENSE](LICENSE) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [README.md](README.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [RELEASE.md](RELEASE.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [SECURITY.md](SECURITY.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [USAGE.md](USAGE.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [WINDOWS_STEAM_RUNTIME.md](WINDOWS_STEAM_RUNTIME.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## Repository configuration (5)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [.gitignore](.gitignore) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [.github/FUNDING.yml](.github/FUNDING.yml) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [.github/ISSUE_TEMPLATE/bug_report.md](.github/ISSUE_TEMPLATE/bug_report.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [.github/ISSUE_TEMPLATE/feature_request.md](.github/ISSUE_TEMPLATE/feature_request.md) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [.github/workflows/release.yml](.github/workflows/release.yml) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## Build and packaging (9)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [Cargo.lock](Cargo.lock) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [Cargo.toml](Cargo.toml) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [build.rs](build.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [flake.nix](flake.nix) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [assets/asciiart_banner.txt](assets/asciiart_banner.txt) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [assets/aurelia_logo.png](assets/aurelia_logo.png) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [assets/aurelia_logo_v2.png](assets/aurelia_logo_v2.png) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [assets/aurelia_logo_v3.png](assets/aurelia_logo_v3.png) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [proto/service_cloudconfigstore.proto](proto/service_cloudconfigstore.proto) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/ top level (6)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/cli.rs](src/cli.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/lib.rs](src/lib.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/main.rs](src/main.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/main_tests.rs](src/main_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client.rs](src/steam_client.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client_tests.rs](src/steam_client_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/commands/ (17)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/commands/auth.rs](src/commands/auth.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/cloud.rs](src/commands/cloud.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/collections.rs](src/commands/collections.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/common.rs](src/commands/common.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/config.rs](src/commands/config.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/info.rs](src/commands/info.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/install.rs](src/commands/install.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/install_downgrade_tests.rs](src/commands/install_downgrade_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/launch.rs](src/commands/launch.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/library.rs](src/commands/library.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/market.rs](src/commands/market.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/mod.rs](src/commands/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/plugins.rs](src/commands/plugins.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/runtimes.rs](src/commands/runtimes.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/scripts.rs](src/commands/scripts.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/social.rs](src/commands/social.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/commands/workshop.rs](src/commands/workshop.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/compat/ (10)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/compat/luxtorpeda.rs](src/compat/luxtorpeda.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/luxtorpeda_tests.rs](src/compat/luxtorpeda_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/mod.rs](src/compat/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/proc_admin.rs](src/compat/proc_admin.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/proc_admin_tests.rs](src/compat/proc_admin_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/proton.rs](src/compat/proton.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/proton_tests.rs](src/compat/proton_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/running.rs](src/compat/running.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/umu.rs](src/compat/umu.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/compat/umu_tests.rs](src/compat/umu_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/core/ (11)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/core/config.rs](src/core/config.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/config_tests.rs](src/core/config_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/mod.rs](src/core/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/models.rs](src/core/models.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/net.rs](src/core/net.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/net_tests.rs](src/core/net_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/output.rs](src/core/output.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/utils.rs](src/core/utils.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/utils_resolve_runner_tests.rs](src/core/utils_resolve_runner_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/utils_runner_classification_tests.rs](src/core/utils_runner_classification_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/core/utils_save_prefix_tests.rs](src/core/utils_save_prefix_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/daemon/ (7)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/daemon/client.rs](src/daemon/client.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/client_tests.rs](src/daemon/client_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/daemon_tests.rs](src/daemon/daemon_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/mod.rs](src/daemon/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/proto.rs](src/daemon/proto.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/server.rs](src/daemon/server.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/daemon/transport.rs](src/daemon/transport.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/infra/ (14)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/infra/mod.rs](src/infra/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/cli.rs](src/infra/logging/cli.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/cli_tests.rs](src/infra/logging/cli_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/debug_utils.rs](src/infra/logging/debug_utils.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/event_log.rs](src/infra/logging/event_log.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/mod.rs](src/infra/logging/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/session.rs](src/infra/logging/session.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/tests.rs](src/infra/logging/tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/logging/wine_capture.rs](src/infra/logging/wine_capture.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/runners/luxtorpeda.rs](src/infra/runners/luxtorpeda.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/runners/mod.rs](src/infra/runners/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/runners/tests.rs](src/infra/runners/tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/runners/trait.rs](src/infra/runners/trait.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/infra/runners/wine_tkg.rs](src/infra/runners/wine_tkg.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/launch/ (28)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/launch/dll_provider_resolver.rs](src/launch/dll_provider_resolver.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/dll_provider_resolver_tests.rs](src/launch/dll_provider_resolver_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/launch_script.rs](src/launch/launch_script.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/launch_script_tests.rs](src/launch/launch_script_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/mod.rs](src/launch/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/pipeline.rs](src/launch/pipeline.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/pipeline_tests.rs](src/launch/pipeline_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/verification_tests.rs](src/launch/verification_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/fixups/fixups_tests.rs](src/launch/fixups/fixups_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/fixups/mod.rs](src/launch/fixups/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/apply_launch_script.rs](src/launch/stages/apply_launch_script.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/apply_launch_script_tests.rs](src/launch/stages/apply_launch_script_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/build_command.rs](src/launch/stages/build_command.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/build_environment.rs](src/launch/stages/build_environment.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/finalize.rs](src/launch/stages/finalize.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/mod.rs](src/launch/stages/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/preflight.rs](src/launch/stages/preflight.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/preflight_tests.rs](src/launch/stages/preflight_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/prepare_prefix.rs](src/launch/stages/prepare_prefix.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/resolve_components.rs](src/launch/stages/resolve_components.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/resolve_dll_providers.rs](src/launch/stages/resolve_dll_providers.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/resolve_game.rs](src/launch/stages/resolve_game.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/resolve_game_fixups.rs](src/launch/stages/resolve_game_fixups.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/resolve_profile.rs](src/launch/stages/resolve_profile.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/stages/spawn_process.rs](src/launch/stages/spawn_process.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/validators/invariants.rs](src/launch/validators/invariants.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/validators/mod.rs](src/launch/validators/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/launch/validators/overrides.rs](src/launch/validators/overrides.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/library/ (11)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/library/cloud_sync.rs](src/library/cloud_sync.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/cloud_sync_tests.rs](src/library/cloud_sync_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/collections.rs](src/library/collections.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/collections_tests.rs](src/library/collections_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/depot_browser.rs](src/library/depot_browser.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/library_tests.rs](src/library/library_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/local_library.rs](src/library/local_library.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/local_library_tests.rs](src/library/local_library_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/mod.rs](src/library/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/relocate.rs](src/library/relocate.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/library/relocate_tests.rs](src/library/relocate_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/steam_client/ (18)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/steam_client/chat.rs](src/steam_client/chat.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/client.rs](src/steam_client/client.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/cloudconfig.rs](src/steam_client/cloudconfig.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/collections.rs](src/steam_client/collections.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/content.rs](src/steam_client/content.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/friends.rs](src/steam_client/friends.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/friends_tests.rs](src/steam_client/friends_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/install.rs](src/steam_client/install.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/launch.rs](src/steam_client/launch.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/library.rs](src/steam_client/library.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/library_update_detection_tests.rs](src/steam_client/library_update_detection_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/manage.rs](src/steam_client/manage.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/manifests.rs](src/steam_client/manifests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/market.rs](src/steam_client/market.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/process.rs](src/steam_client/process.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/workshop.rs](src/steam_client/workshop.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/workshop_manifest.rs](src/steam_client/workshop_manifest.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/steam_client/workshop_manifest_tests.rs](src/steam_client/workshop_manifest_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## src/web/ (10)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [src/web/cm_list.rs](src/web/cm_list.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/mod.rs](src/web/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/openid.rs](src/web/openid.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/openid_tests.rs](src/web/openid_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/steam_urls.rs](src/web/steam_urls.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/steam_urls_tests.rs](src/web/steam_urls_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/store.rs](src/web/store.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/web_session.rs](src/web/web_session.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/web_token.rs](src/web/web_token.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [src/web/web_token_tests.rs](src/web/web_token_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## tests/ (15)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [tests/compat_discovery.rs](tests/compat_discovery.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/compatibility_validator_tests.rs](tests/compatibility_validator_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/config_game_steam_runtime.rs](tests/config_game_steam_runtime.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/config_load_errors.rs](tests/config_load_errors.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/dll_override_tests.rs](tests/dll_override_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/dll_resolution_report.rs](tests/dll_resolution_report.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/dxvk_evidence_tests.rs](tests/dxvk_evidence_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/launch_summary_tests.rs](tests/launch_summary_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/path_resolution.rs](tests/path_resolution.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/preflight_integration_tests.rs](tests/preflight_integration_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/runner_root_derivation.rs](tests/runner_root_derivation.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/staged_launch_failure_tests.rs](tests/staged_launch_failure_tests.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/steam_runtime_install.rs](tests/steam_runtime_install.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/steam_runtime_runner_config.rs](tests/steam_runtime_runner_config.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |
| [tests/symlink_deployment.rs](tests/symlink_deployment.rs) ★ | &#9744; | &#9744; | &#9744; | &#9744; |

## vendor/steam-cdn/ (23)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [vendor/steam-cdn/Cargo.lock](vendor/steam-cdn/Cargo.lock) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/Cargo.toml](vendor/steam-cdn/Cargo.toml) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/Cargo.toml.orig](vendor/steam-cdn/Cargo.toml.orig) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/LICENSE](vendor/steam-cdn/LICENSE) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/README.md](vendor/steam-cdn/README.md) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/examples/download_manifest.rs](vendor/steam-cdn/examples/download_manifest.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/error.rs](vendor/steam-cdn/src/error.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/lib.rs](vendor/steam-cdn/src/lib.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/depot.rs](vendor/steam-cdn/src/cdn/depot.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/inner.rs](vendor/steam-cdn/src/cdn/inner.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/mod.rs](vendor/steam-cdn/src/cdn/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/crypto/aes256.rs](vendor/steam-cdn/src/crypto/aes256.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/crypto/mod.rs](vendor/steam-cdn/src/crypto/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/utils/base64.rs](vendor/steam-cdn/src/utils/base64.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/utils/lzma.rs](vendor/steam-cdn/src/utils/lzma.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/utils/mod.rs](vendor/steam-cdn/src/utils/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/web_api/content_service.rs](vendor/steam-cdn/src/web_api/content_service.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/web_api/mod.rs](vendor/steam-cdn/src/web_api/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/depot_chunk/mod.rs](vendor/steam-cdn/src/cdn/depot_chunk/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/manifest/buf.rs](vendor/steam-cdn/src/cdn/manifest/buf.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/manifest/error.rs](vendor/steam-cdn/src/cdn/manifest/error.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/manifest/file.rs](vendor/steam-cdn/src/cdn/manifest/file.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-cdn/src/cdn/manifest/mod.rs](vendor/steam-cdn/src/cdn/manifest/mod.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |

## vendor/steam-vent-chat/ (5)

| File | Manual | AI Assisted | Vibecoded | Vendored |
|---|:---:|:---:|:---:|:---:|
| [vendor/steam-vent-chat/.gitignore](vendor/steam-vent-chat/.gitignore) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-vent-chat/Cargo.lock](vendor/steam-vent-chat/Cargo.lock) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-vent-chat/Cargo.toml](vendor/steam-vent-chat/Cargo.toml) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-vent-chat/README.md](vendor/steam-vent-chat/README.md) ★ | &#9744; | &#9744; | &#9744; | &#9745; |
| [vendor/steam-vent-chat/src/lib.rs](vendor/steam-vent-chat/src/lib.rs) ★ | &#9744; | &#9744; | &#9744; | &#9745; |

---

**★ In scope for the latest code review.** Every tracked file carries the mark, so
nothing in the tree is exempt from a manual read-through.
