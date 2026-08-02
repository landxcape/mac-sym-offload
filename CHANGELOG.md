# Changelog

All notable changes to the `mac-sym-offload` (`mso`) project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
