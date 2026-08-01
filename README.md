# mso: macOS Developer Storage Migrator 🚀

[![Rust](https://img.shields.io/badge/rust-v1.97%2B-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](https://apple.com/macos)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

`mso` (**macOS Developer Storage Migrator**) is a fast, safe, and production-ready CLI/TUI tool written in Rust. It enables iOS, Android, Flutter, Web, and Rust developers on Mac (256GB/512GB base models) to safely offload bloated cache directories to an external APFS SSD and manage macOS symbolic links (`ln -s`).

---

## 🌟 Key Features

* **11 Pre-Configured + Custom Targets**: Offload Xcode DerivedData, Android `.gradle`, Flutter `.pub-cache`, NPM `.npm`, Cargo `.cargo/registry`, Maven `.m2/repository`, iOS CoreSimulator, CocoaPods, Xcode Archives, iOS Device Support, and **arbitrary custom local folders** (`+ Add Custom Local Folder Path`).
* **Live Dual-Line Progress Bar Engine**: Displays real-time transfer speeds (`82.3 MiB/s`), live ETAs (`9m`), byte progress counters (`1.97 GiB / 45.60 GiB`), and current relative file paths: powered by a custom byte-stream parser compatible with stock macOS `/usr/bin/rsync`.
* **5-Option Conflict Diagnostics & Rollback Engine**: When data exists on both local Mac and external SSD, `mso` calculates live sizes, detects partial backups, and offers 5 resolution strategies including **`(Recommended: Safe Merge)`**, **`Rollback SSD Data to Local`**, and **`Keep Local & Discard SSD Backup`**.
* **Atomic Failure Rollback**: If a fresh migration fails due to macOS permission locks (`Operation not permitted`), `mso` automatically cleans up partial SSD copies to leave your local Mac state clean as `Fresh (Local)`.
* **Custom SSD Target Subfolders**: Choose to offload directly to the external APFS volume root, browse existing subfolders, or create custom target subfolders (`mkdir -p`).
* **APFS Filesystem Validation**: Queries volume metadata via `diskutil info` to refuse non-APFS formatted drives (exFAT/NTFS) and preserve Unix file permissions.
* **Role-Aware Target Recommendations**: Categorizes caches into disposable **Build Outputs** (pre-checked by default for immediate space relief) vs **Package Source Registries** (unchecked by default to preserve offline Flutter, Web, and Rust development).
* **Config Crash Recovery & Auto-Discovery**: Automatically scans attached external SSDs for pre-existing offloaded directories and highlights them for instant re-linking (`[Discovered Files on SSD - Ready to Re-link]`), even if `config.json` is lost or deleted.
* **Single-Stage Interactive Menu**: Full TUI menu with in-place screen updates (`clear_screen`), cursor preservation, and instant action items (`[*] Auto-Select Recommended`, `[+] Select All`, `[-] Select None`, `+ Add Custom Path`, `↻ Force Rescan`, `‹ Back`).
* **Top-of-Screen Viewport Positioning**: Automatically clears the terminal viewport upon launch (`\x1B[2J\x1B[1;1H`) so the CLI renders cleanly at row 1, column 1 without bottom-edge terminal cramping.
* **Universal Step-Wise `ESC` Navigation**: Pressing `ESC` on any wizard step cleanly navigates back to the previous step (Step 3 Checklist -> Step 2 Subfolder -> Step 1 Drive Selection).
* **In-Memory Session Caching**: Instant (~0ms) back-and-forth wizard navigation without redundant disk I/O thrashes.
* **Safe Reverse Restore (`mso restore`)**: Brings caches back to local Mac storage with a **10GB internal disk safety gate** (`df -k /` check) to prevent Mac OS disk exhaustion.
* **Disconnection & Conflict Repair (`mso repair`)**: Fixes broken symlinks, ghost local directories, or data conflicts using saved drive configuration.
* **Dry-Run Simulation (`--dry-run`)**: Safely preview all file copy and symlink operations without touching disk files.

---

## 📊 Supported Cache Targets

| Component | Target Key | Category | Default macOS Source Path | Default Relative External SSD Path |
| :--- | :--- | :--- | :--- | :--- |
| **Xcode DerivedData** | `deriveddata` | Build Output | `~/Library/Developer/Xcode/DerivedData` | `<Drive>/Developer/Xcode/DerivedData` |
| **iOS CoreSimulator** | `coresimulator` | Build Output | `~/Library/Developer/CoreSimulator` | `<Drive>/Developer/CoreSimulator` |
| **Xcode Archives** | `archives` | Build Output | `~/Library/Developer/Xcode/Archives` | `<Drive>/Developer/Xcode/Archives` |
| **iOS Device Support** | `iosdevicesupport` | Build Output | `~/Library/Developer/Xcode/iOS DeviceSupport` | `<Drive>/Developer/Xcode/iOS DeviceSupport` |
| **Android Gradle Cache** | `gradle` | Build Output | `~/.gradle` | `<Drive>/.gradle` |
| **Android Emulator & SDK** | `android` | Build Output | `~/.android` | `<Drive>/.android` |
| **Flutter Pub Cache** | `pub-cache` | Package Source | `~/.pub-cache` | `<Drive>/.pub-cache` |
| **CocoaPods Cache** | `cocoapods` | Package Source | `~/.cocoapods` | `<Drive>/.cocoapods` |
| **NPM Package Cache** | `npm` | Package Source | `~/.npm` | `<Drive>/.npm` |
| **Rust Cargo Registry** | `cargo` | Package Source | `~/.cargo/registry` | `<Drive>/.cargo/registry` |
| **Maven Local Repository** | `m2` | Package Source | `~/.m2/repository` | `<Drive>/.m2/repository` |
| **Custom Local Folder** | `custom` | Custom | Arbitrary Local Path | `<Drive>/<Subfolder>/<Name>` |

---

## 🛠️ Installation

Users do **NOT** need Rust installed to use `mso`. Standalone pre-compiled universal binaries are published on GitHub Releases and Homebrew.

### Option 1: Homebrew (Recommended for macOS Developers)
Install directly via Homebrew from the `landxcape` tap:
```bash
brew install landxcape/tap/mso
```

### Option 2: Quick 1-Line Terminal Installer
Run this command in Terminal to automatically download and install the latest release binary for your Mac architecture:
```bash
curl -fsSL https://raw.githubusercontent.com/landxcape/mac-sym-offload/main/install.sh | bash
```

### Option 3: Download Pre-Compiled Binary from GitHub Releases
1. Download `mso-macos-arm64.tar.gz` (Apple Silicon) or `mso-macos-x86_64.tar.gz` (Intel Mac) from [GitHub Releases](https://github.com/landxcape/mac-sym-offload/releases).
2. Extract and move `mso` to `/usr/local/bin/`:
   ```bash
   tar -xzf mso-macos-arm64.tar.gz
   sudo mv mso /usr/local/bin/
   chmod +x /usr/local/bin/mso
   ```

### Option 4: Build from Source (Cargo)
For Rust developers who prefer building locally:
```bash
git clone https://github.com/landxcape/mac-sym-offload.git
cd mac-sym-offload
cargo build --release
sudo cp target/release/mso /usr/local/bin/
```

Verify installation:
```bash
mso --version
```

---

## 💻 Usage & Cheat Sheet

### 1. Interactive TUI Wizard (Default)
Simply run `mso` to launch the interactive setup:
```bash
mso
```

```text
==================================================
  mso - macOS Developer Storage Migrator (v0.1.0)
==================================================

Step 3: Toggle components to offload or trigger actions:
  [x] Android Gradle Cache (45.6 GB) - Fresh [BUILD OUTPUT - Safe to offload (Android build output & dependencies)]
  [x] Xcode DerivedData (5.7 GB) - Fresh [BUILD OUTPUT - Safe to offload (Xcode automatically rebuilds caches)]
  [ ] Xcode Archives (.xcarchive) (990.5 MB) - Fresh [BUILD OUTPUT - Safe to offload (Xcode build archives)]
  [ ] iOS CoreSimulator (577.3 MB) - Fresh [BUILD OUTPUT - Safe to offload (iOS Simulator data)]
  [ ] Flutter Pub Cache (.pub-cache) (4.6 GB) - Fresh [PACKAGE SOURCE - Keep local to preserve offline Flutter development]
  [ ] NPM Package Cache (.npm) (3.5 GB) - Fresh [PACKAGE SOURCE - Keep local to preserve offline Node/Web development]
  [ ] Maven Local Repository (.m2/repository) (1.1 GB) - Fresh [PACKAGE SOURCE - Keep local for offline Java/Maven dependencies]
  [ ] Rust Cargo Registry (.cargo/registry) (201.2 MB) - Fresh [PACKAGE SOURCE - Keep local for offline Rust crate sources]
  [ ] CocoaPods Cache (.cocoapods) (79.9 MB) - Fresh [PACKAGE SOURCE - Keep local for offline CocoaPods spec repos]
  ────────────────────────────────────────────────────────────
  ✔ Start Migration Now (2 selected - 51.3 GB)
  [*] Auto-Select Recommended (Build Outputs Only)
  [+] Select All Components
  [-] Select None (Clear All Checkboxes)
  + Add Custom Local Folder Path to Offload...
  ↻ Force Rescan Local & SSD Caches...
  ‹ Back to Subfolder Selection (Step 2)
[Use [Up/Down] & [ENTER] to toggle component or run action | [ESC] to go back]
```

### 2. Live Transfer Progress View
```text
Migrating Android Gradle Cache...
⠤ [00:01:03] [=>--------------------------------------] 1.97 GiB/45.60 GiB (82.30 MiB/s, 9m)
  Transferring: caches/8.10.2/transforms/c90f6284eea2d3bdd680ddb7ca5ed7d0/transformed/instrumented/
```

### 3. Conflict Resolution Menu
```text
Conflict Diagnostic for: iOS CoreSimulator
   Local path:    /Users/username/Library/Developer/CoreSimulator [Size: 0 B]
   External path: /Volumes/ExternalSSD/Developer/CoreSimulator [Size: 577.3 MB]
   Diagnostic Note: External SSD contains a larger backup (577.3 MB) than current Local path (0 B).

? Choose resolution strategy with recommendations:
  (Recommended: Safe Merge) Merge Local into External [Syncs missing files to SSD & frees local space]
  Discard Local and Restore Symlink [Frees 0 B local space, uses 577.3 MB SSD backup]
  Overwrite External with Local [Deletes 577.3 MB SSD backup & re-copies 0 B from Mac]
  Rollback SSD Data to Local [Copies 577.3 MB SSD data back to Mac & deletes SSD copy]
  Keep Local & Discard SSD Backup [Deletes 577.3 MB SSD copy & leaves local Mac folder unchanged]
```

### 4. View Cache Directory Status & Sizes
```bash
mso status
# Output status table as JSON for scripts:
mso scan --json
```

### 5. Repair Broken Symlinks & Resolve Data Conflicts
```bash
# Repair broken symlinks or resolve conflicts using saved SSD drive
mso repair

# Non-interactive conflict repair:
mso repair --conflict-strategy merge
```

### 6. Restore Caches Back to Local Mac Storage
```bash
# Interactive restore with disk space safety gate
mso restore

# Non-interactive restore
mso restore --targets deriveddata,gradle --keep-external
```

### 7. View & Reset Configuration
```bash
mso config
mso config --reset
```

### 8. Model Context Protocol (MCP) Server for AI Assistants
`mso` includes a native Model Context Protocol (MCP) server that communicates over `stdio` using JSON-RPC 2.0. This allows AI assistants (Claude Desktop, Antigravity, Cursor, VS Code) to inspect developer storage, scan targets, diagnose broken links, and offload build caches natively.

Run the server over stdio:
```bash
mso mcp
```

Add `mso` to your AI client configuration (e.g. `claude_desktop_config.json` or Antigravity MCP settings):
```json
{
  "mcpServers": {
    "mso": {
      "command": "mso",
      "args": ["mcp"]
    }
  }
}
```

**Advertised MCP Tools**:
* `mso_status`: Returns overall Mac disk telemetry, external APFS volume status, and summary counts of offloaded vs local cache targets.
* `mso_list_targets`: Lists all supported developer cache targets with byte sizes, human-readable sizes, state (`Fresh`, `AlreadyLinked`, `GhostLocal`, `Conflict`), and safety recommendations (`disposable` vs `package_registry`).
* `mso_diagnose`: Scans for data drift, unmounted external SSD symlinks (`GhostLocal`), or path conflicts.
* `mso_offload_target`: Safely relocates a target key to the external APFS SSD using exit-status guarded transfer and atomic failure rollback.

---

## 💡 Bonus for Flutter Developers: Parallel Workspace Cleaner (`fpclean_all`)

While `mso` manages **global system build caches** (`.gradle`, `DerivedData`, `Archives`, `CoreSimulator`), your local workspace directory containing dozens of Flutter projects can also accumulate 30GB+ of local `build/` and `.dart_tool/` artifacts.

Save this `.sh` script function (e.g. into `~/.shell/flutter.sh` and source it in your `~/.zshrc` / `~/.bashrc`):

```bash
# Parallel clean for all Flutter projects in workspace
fpclean_all() {
  find . -name pubspec.yaml -exec grep -l "flutter:" {} \; |
    xargs -I {} -P $(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo 6) \
      sh -c 'd=${1%/*}; echo "Cleaning $d"; cd "$d" && flutter clean' _ {}
}
```

Run `cd ~/Developer/work && fpclean_all` from any parent workspace directory to clean all nested Flutter project build folders in parallel!

---

## 🛡️ Safety & Architecture Principles

1. **Copy Verification before Local Delete**: `mso` executes `rsync -aP` and verifies exit status before removing local directories.
2. **Atomic Rollback on Failure**: If local directory removal fails (e.g. due to macOS permission locks), `mso` automatically cleans up partial SSD files to prevent orphaned data.
3. **Resume & Rollback Capability**: Offers `(Safe Merge)` to resume interrupted transfers or `Rollback SSD Data to Local` to bring SSD data back to Mac disk.
4. **10GB Restore Safety Buffer**: `mso restore` verifies:
   $$\text{Free Internal Space} > \text{Restore Target Size} + 10\text{ GB Buffer}$$
5. **APFS Filesystem Enforcement**: Prevents file permissions loss by blocking non-APFS drives (exFAT/NTFS) before any file operations occur.

---

## 📄 License
Distributed under the [MIT License](LICENSE).
