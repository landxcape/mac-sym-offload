# Changelog

All notable changes to the `mac-sym-offload` (`mso`) project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.7] - 2026-08-06

### Fixed
- **Custom Folder Offload Target Resolution Fix**: Fixed a bug in `handle_tool_offload_target` where custom folders passed via `mso_offload_custom_folder` failed target resolution with `Unknown target_key 'custom-...'`. Target resolution now inspects `custom_local_path` parameters and resolves `CacheTarget::Custom` dynamically.
- **Custom Folder Relative Path Handling**: Updated `CacheTarget::Custom` `default_relative_path` resolution so absolute custom paths map safely to `CustomFolders/<folder_name>` on the external SSD instead of colliding with local absolute paths.
- **Added Integration Test**: Added `test_mcp_offload_custom_folder_dry_run_and_execution` to test end-to-end dry-run preview and execution on custom folders.

## [0.4.6] - 2026-08-06

### Enhanced
- **Actionable Error Message Hints for Offload Attempts on Linked Targets**: Updated `validate_operation_preconditions` error message when `mso_offload_target` is attempted on targets that are already offloaded (`AlreadyLinked` state) to surface the active target path and suggest calling `mso_repair_rollback_to_local` to restore data back to local Mac storage.

## [0.4.5] - 2026-08-05

### Fixed
- **Fresh Target Offload Execution Fix**: Fixed a bug where `mso_offload_target` passed `Some(ConflictStrategy::Merge)` to `migrator::migrate_target` for `Fresh` state targets when no explicit conflict strategy argument was specified. When `explicit_strat` is `None`, `migrator::migrate_target` now correctly receives `None`, causing `Fresh` state targets to execute `execute_fresh_migration` cleanly instead of failing precondition validation for `Merge`.
- **Display & Warning Message Clarity**: Updated dry-run preview formatting for plain `mso_offload_target` calls against `Fresh` targets to display `conflict_strategy: "offload"` and warning `"This will offload local Mac directory ... to external APFS SSD and establish a symlink"`.
- **Added Integration Test**: Added `test_mcp_fresh_target_offload_dry_run_and_execution` verifying end-to-end dry-run preview and execution on `Fresh` state targets.

## [0.4.4] - 2026-08-05

### Fixed
- **Small Directory & Low-Byte Parity Fix**: Fixed `get_fast_dir_size_bytes` returning `0` when `du -sk` rounds down small directories (< 1024 bytes) to `0` KB. Added an automatic `fs::read_dir` metadata walk fallback so exact payload byte sizes are accurately measured and returned in both dry-run and execution paths.
- **Server Gate Order Refinement**: Moved the `if execute` dry-run preview check to execute before APFS drive validation and assessment, ensuring dry-run requirement error `-32002` is consistently returned if an execution call is attempted without prior preview.
- **Live Stdio MCP Verification**: Verified all 6 conflict strategies (`discard_local`, `merge`, `overwrite_external`, `discard_external`, `rollback_to_local`, `relink`) live over stdio MCP JSON-RPC against real disk states, confirming 100% byte parity (`would_move_bytes == bytes_moved > 0`).

## [0.4.3] - 2026-08-05

### Fixed
- **Strict Byte Parity (`would_move_bytes` == `bytes_moved`)**: Reconciled dry-run (`would_move_bytes`) and execution (`bytes_moved`) to compute `action_bytes` using a single unified definition (transfer/action bytes). When a conflict exists (local marker = 4.0 KB, external SSD = 1.3 GB), `discard_local` dry-run and execution both report 4.0 KB (local bytes discarded), guaranteeing 100% agreement.
- **Added Regression Test**: Added `test_mcp_dry_run_and_execution_bytes_match_on_conflict` verifying byte equality across dry-run and execution paths.

## [0.4.2] - 2026-08-05

### Security & Integrity Fixes (Critical Data-Loss Prevention)
- **Strict Strategy Pre-Condition Engine (`validate_operation_preconditions`)**: Rejects destructive conflict strategies (`overwrite_external`, `discard_local`, `discard_external`, `merge`) when target is in `AlreadyLinked` or non-`Conflict` state. Prevents data destruction from calling `overwrite_external` against symlinks pointing to external SSD targets.
- **`discard_external` Strategy Exposure**: Added `"discard_external"` (`keep_local_discard_external`) to `available_strategies` in `mso_diagnose` JSON output.
- **Reporting Accuracy**: Standardized dry-run (`would_move_bytes`) and execution (`bytes_moved`) to report exact operation action bytes.

## [0.4.1] - 2026-08-02

### Fixed
- **Antigravity Active MCP Config Path (`~/.gemini/config/mcp_config.json`)**: Updated `setup_antigravity` to write `mso` directly into `~/.gemini/config/mcp_config.json` (the exact location Antigravity CLI uses to inject active MCP tools into AI agent sessions).

## [0.4.0] - 2026-08-02

### Fixed & Enhanced
- **Multi-Alias Antigravity Setup**: Updated `setup_antigravity` in `mso mcp setup` to automatically populate schemas across all server aliases (`~/.gemini/antigravity-cli/mcp/mso`, `mac-sym-offload`, and `MacSymOffload`) alongside `mcp_config.json`.
- **Guaranteed Stdio & Schema Tool Registration**: Ensures Antigravity CLI indexes native MCP tools cleanly regardless of tool lookup casing or alias.

## [0.3.9] - 2026-08-02

### Fixed & Enhanced
- **Absolute Binary Path Resolution in `mcp_config.json`**: Fixed issue in Antigravity CLI setup where relative `"command": "mso"` caused tool discovery failure in non-interactive subshells. Now resolves full absolute path (`/opt/homebrew/bin/mso`) in `~/.gemini/antigravity-cli/mcp_config.json`.
- **AI Agent MCP Preference Mandate**: Added explicit directive to `instructions.md` instructing AI agents (Codex, Claude, Antigravity) to use native MCP tools (`mso_get_status`, `mso_offload_target`, `mso_repair_*`, etc.) instead of falling back to terminal CLI commands.

## [0.3.8] - 2026-08-02

### Fixed
- **MCP Tool Conflict Strategy Matching**: Fixed bug in `handle_tool_offload_target` where `conflict_strategy` string parsing only recognized `overwrite_external` and `discard_local`, defaulting all other strategies (including `rollback_to_local`, `relink`, and `discard_external`) to `merge`.
- **MCP Stdio End-to-End Verification**: Verified `mso_repair_rollback_to_local` via live JSON-RPC stdio protocol. The tool now returns the correct restore warning and restores local files from external SSD.
- **Automated MCP Unit Test**: Added `test_mcp_rollback_to_local_dry_run` to prevent regression.

## [0.3.7] - 2026-08-02

### Fixed & Refactored
- **Operation-First Dispatch Refactor**: Replaced state-first matching with explicit `TargetOperation` handlers across all 8 strategies (`Offload`, `Restore`, `RollbackToLocal`, `Merge`, `OverwriteExternal`, `DiscardLocal`, `DiscardExternal`, `Relink`).
- **Fixed `mso_repair_rollback_to_local`**: Rollback now executes regardless of starting state (`AlreadyLinked`, `StaleSymlink`, `Conflict`), copying external data back to local Mac storage, removing the external SSD directory, and replacing the symlink with a real local folder.
- **Dynamic Strategy-Aware MCP Telemetry**: Updated `handle_tool_offload_target` to generate operation-specific preview warnings, dry-run messages, and verified on-disk `bytes_moved` output.
- **Automated Rollback Integration Test**: Added `test_execute_rollback_to_local_converts_symlink_to_real_dir` verifying true filesystem transformation on disk.

## [0.3.6] - 2026-08-02

### Enhanced
- **Dual Interactive TUI & MCP Parity**: Fully integrated `Relink` strategy (`ConflictStrategy::Relink`) into both the interactive CLI TUI wizard (`mso` / `mso repair`) and the stdio MCP server (`mso_repair_relink`).

## [0.3.5] - 2026-08-02

### Added & Enhanced
- **Stale Symlink Detection**: Added `PathState::StaleSymlink` to explicitly distinguish unmounted drives (`GhostLocal` / `Disconnected Drive`) from outdated/stale symlink targets created under legacy path templates.
- **Dedicated Relink Strategy**: Introduced `ConflictStrategy::Relink` and MCP tool `mso_repair_relink` (alias `mso_relink`), allowing users and AI agents to safely replace stale symlinks without data copy or manual shell work.
- **Proactive Diagnostic Telemetry**: `mso_diagnose` and `mso_diagnose_conflicts` now explicitly report stale symlinks and provide actionable `relink` recommendations.

## [0.3.4] - 2026-08-02

### Fixed
- **Xcode Target Path Unification**: Unified all three Xcode-category targets (`derived-data`, `ios-device-support`, `xcode-archives`) under the exact same nested path structure (`Developer/Developer/Xcode/...`) matching physical disk layout.
- **AlreadyLinked Size Reporting Fix**: Fixed size calculator regression where `AlreadyLinked` targets reported `0 B` due to path field disagreement; size calculation now measures the authoritative symlink target location (`target_path`) directly.
- **Path Field Agreement**: Ensured `external_path` and `state.target_path` always agree 100% in all status/list/diagnose outputs for `AlreadyLinked` targets.

## [0.3.3] - 2026-08-02

### Fixed
- **Subfolder-Scoped Connectivity Check**: `inspect_drive_apfs` now resolves the underlying volume mount point (`/Volumes/MacData`) when checking filesystem info for subfolder-scoped `saved_drive` paths (e.g. `/Volumes/MacData/Developer`), eliminating false-negative drive disconnection errors.
- **Xcode Target Path Deduplication**: Normalizes relative path construction when `saved_drive` includes a subfolder prefix (e.g. `/Volumes/MacData/Developer`), preventing double-nesting (`Developer/Developer`) and fixing false-positive `GhostLocal` classifications for `xcode-archives` and other Xcode targets.
- **Valid Symlink Preservation**: Symlinks pointing to valid, existing SSD directories are correctly classified as `AlreadyLinked` instead of `GhostLocal`.

## [0.3.2] - 2026-08-02

### Fixed & Enhanced
- **Codex TOML Config Support**: Added full support for Codex TOML configuration files (`~/.codex/config.toml`). `mso mcp status` and `mso mcp setup` now seamlessly detect and configure `[mcp_servers.mso]` in `config.toml`.

## [0.3.1] - 2026-08-02

### Added
- **Complete 19-Tool MCP Suite**: Expanded MCP server over stdio to achieve 100% feature parity with the native CLI and interactive TUI wizard.
- **5 Tool Groups Exposed to AI Agents**:
  1. **Telemetry & Discovery**: `mso_get_status`, `mso_list_all_targets`, `mso_list_disposable_targets`, `mso_list_package_registries`, `mso_diagnose_conflicts`, `mso_discover_ssd_caches`.
  2. **Offloading & Migration**: `mso_offload_target`, `mso_offload_custom_folder` (user manual custom local paths), `mso_offload_recommended` (disposable build outputs batch).
  3. **Reverse Restore Engine**: `mso_restore_target` (10GB disk space safety gate), `mso_restore_with_backup` (`--keep-external`).
  4. **Conflict & Repair Strategies**: `mso_repair_merge`, `mso_repair_overwrite_external`, `mso_repair_discard_local`, `mso_repair_rollback_to_local`, `mso_repair_discard_external`.
  5. **Config & Drive Management**: `mso_get_config`, `mso_set_target_drive` (user manual drive & subfolder path), `mso_reset_config`.
- **Manual User Input Parameters**: Added explicit schema parameters (`custom_local_path`, `drive_path`, `subfolder_path`) for AI agents to pass human-entered paths.

## [0.2.2] - 2026-08-01

### Added
- **Server-Enforced Dry-Run Tracking (`PREVIEWED_TARGETS`)**: The MCP server tracks previewed target keys in memory and rejects any direct `mso_offload_target(execute: true)` call with JSON-RPC error `-32002` unless a preceding `execute: false` dry-run call occurred.
- **Implementation-Defined JSON-RPC Error Codes**: Updated error code semantics according to the JSON-RPC 2.0 specification:
  - `-32001` (Resource Busy): Returned when `ACTIVE_MIGRATIONS` detects a target is mid-transfer.
  - `-32002` (Dry-Run Required): Returned when `execute: true` is called without a preceding preview.

### Security & Resilience
- **Stream Cancellation Resilience**: Documented that transfers run synchronously to completion with full atomic rollback guarantees (`rsync -aP` with exit-code verification). If a host client stream cancels mid-flight, local directories are only removed if `rsync` exits cleanly with code 0.

## [0.2.1] - 2026-08-01

### Added
- **Dry-Run Confirmation Gate**: `mso_offload_target` parameter `execute` defaults to `false`. Returns a safe Dry-Run Preview JSON with path warnings and size metrics.
- **In-Process Concurrency Locking (`ACTIVE_MIGRATIONS`)**: Prevents concurrent MCP tool calls from racing on the same target directory.

## [0.2.0] - 2026-08-01

### Added
- **Native Model Context Protocol (MCP) Server (`mso mcp`)**: JSON-RPC 2.0 stdio server enabling AI assistants (Claude Desktop, Antigravity, Cursor, VS Code) to inspect, diagnose, and offload Mac developer storage natively.
- **4 Core MCP Tools**:
  - `mso_status`: Mac disk telemetry, APFS drive health, and offloaded vs local byte summary.
  - `mso_list_targets`: Complete target breakdown with dual byte sizing (`size_bytes` + `size_human`) and category filters (`disposable` vs `package_registry`).
  - `mso_diagnose`: Data drift, broken symlink (`GhostLocal`), and conflict scanner with repair recommendations.
  - `mso_offload_target`: Non-interactive relocation with atomic failure rollbacks.
- **Zero-Token Progress Notifications (`notifications/progress`)**: Emits side-channel JSON-RPC progress events over stdio so AI client UIs render live progress bars without consuming LLM API tokens.
- **Strict 4-Step Input Validation**: Reordered input evaluation to return JSON-RPC `-32602` (Invalid Params) for unknown keys before filesystem I/O.

## [0.1.0] - 2026-07-25

### Added
- **11 Pre-Configured Developer Cache Targets**: Support for Xcode DerivedData, Android Gradle Cache, Flutter Pub Cache, NPM, Maven, Cargo Registry, CocoaPods, Xcode Archives, iOS Device Support, CoreSimulator, and Android SDK/Emulators.
- **5-Option Conflict Resolution Engine**: Includes `(Recommended: Safe Merge)`, `Discard Local`, `Overwrite External`, `Rollback SSD Data to Local`, and `Keep Local & Discard SSD Backup`.
- **Atomic Failure Rollback Engine**: Automatic cleanup of partial SSD copies if local directory deletion fails due to macOS permission locks.
- **Live Dual-Line Progress Bar Engine**: Real-time progress bar with live transfer throughput (`82.3 MiB/s`), live ETAs (`9m`), cumulative byte counters (`1.97 GiB / 45.60 GiB`), and active relative file paths.
- **Universal macOS `rsync -aP` Byte-Stream Parser**: Custom stream parser splitting on `\r` and `\n` to support macOS stock `/usr/bin/rsync` without `rsync 3.x` dependencies.
- **Conflict Size & Diagnostics Engine**: Calculates live local vs external path sizes, detects interrupted/partial SSD backups, and explicitly displays exact gigabytes deleted or freed per option.
- **Enhanced Permission Error Hints**: Explains macOS Full Disk Access (FDA) and active Simulator process locks (`killall Simulator com.apple.CoreSimulator.CoreSimulatorService`).
- **Direct Conflict Repair (`mso repair`)**: `mso repair` loads saved drive settings and directly resolves data conflicts or broken symlinks.
- **Custom Local Folder Offloading**: Support for entering arbitrary custom local folder paths (`+ Add Custom Local Folder Path to Offload...`).
- **Custom Target Subfolder Picker**: Options to target APFS volume root, browse existing subfolders, or create custom target subfolders on external SSDs (`mkdir -p`).
- **APFS Validation Pipeline**: `diskutil info` verification to ensure external volume formatted as APFS before migration.
- **Dual Execution Modes**: Interactive TUI wizard (`mso`) and non-interactive CLI commands (`scan`, `migrate`, `restore`, `repair`, `status`, `config`).
- **Configuration Persistence**: Automatic JSON configuration storage in `~/.config/mso/config.json`.
- **Config Recovery & SSD Auto-Discovery**: Automatic scanning of attached external SSDs for pre-existing offloaded directories (`[Discovered Files on SSD - Ready to Re-link]`).
- **Role-Aware Target Recommendations**: Categorized checklist separating disposable **Build Outputs** (pre-checked by default) vs **Package Sources** (unchecked by default to preserve offline Flutter/Web development).
- **Single-Stage In-Place Interactive TUI**: Smooth menu rendering with `clear_screen` in-place updates, cursor preservation, and instant action items (`[*] Auto-Select Recommended`, `[+] Select All`, `[-] Select None`, `+ Add Custom`, `↻ Force Rescan`, `‹ Back`).
- **Top-of-Screen Viewport Positioning**: ANSI screen clearing (`\x1B[2J\x1B[1;1H`) for full-screen CLI rendering at row 1, column 1.
- **Universal Step-Wise `ESC` Navigation**: Pressing `ESC` at any wizard step navigates back to the preceding step.
- **In-Memory Session Scan Caching**: Fast (~0ms) wizard step transitions with real-time APFS scanning on fresh launches.
- **Dry-Run Simulation**: `--dry-run` flag across commands for safe pre-execution preview.
- **Reverse Restore Engine (`mso restore`)**: Brings caches back to local Mac storage with a **10GB internal disk safety gate** (`df -k /` check).
- **Disconnection Recovery (`mso repair`)**: Fix-it tool for repairing broken symlinks or ghost local directories when an external SSD is unmounted.
