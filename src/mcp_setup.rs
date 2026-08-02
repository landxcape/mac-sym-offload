use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiClientKind {
    Codex,
    ClaudeDesktop,
    Cursor,
    VsCodeCline,
    Antigravity,
    #[allow(dead_code)]
    Custom(PathBuf),
}

impl AiClientKind {
    pub fn display_name(&self) -> &str {
        match self {
            AiClientKind::Codex => "OpenAI Codex CLI / IDE",
            AiClientKind::ClaudeDesktop => "Claude Desktop App",
            AiClientKind::Cursor => "Cursor AI Editor",
            AiClientKind::VsCodeCline => "VS Code (Cline / Roo Code / Codeium)",
            AiClientKind::Antigravity => "Google Antigravity CLI / IDE",
            AiClientKind::Custom(p) => p.to_str().unwrap_or("Custom Config"),
        }
    }

    pub fn default_config_path(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            AiClientKind::Codex => {
                let codex_toml = home.join(".codex").join("config.toml");
                if codex_toml.exists() {
                    Some(codex_toml)
                } else {
                    let codex_dot = home.join(".codex").join("config.json");
                    if codex_dot.exists() {
                        Some(codex_dot)
                    } else {
                        Some(home.join(".config").join("codex").join("mcp.json"))
                    }
                }
            }
            AiClientKind::ClaudeDesktop => Some(
                home.join("Library")
                    .join("Application Support")
                    .join("Claude")
                    .join("claude_desktop_config.json"),
            ),
            AiClientKind::Cursor => {
                let cursor_dot = home.join(".cursor").join("mcp.json");
                if cursor_dot.exists() {
                    Some(cursor_dot)
                } else {
                    Some(
                        home.join("Library")
                            .join("Application Support")
                            .join("Cursor")
                            .join("User")
                            .join("globalStorage")
                            .join("cursor.mcp")
                            .join("mcp.json"),
                    )
                }
            }
            AiClientKind::VsCodeCline => Some(
                home.join("Library")
                    .join("Application Support")
                    .join("Code")
                    .join("User")
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev")
                    .join("settings")
                    .join("cline_mcp_settings.json"),
            ),
            AiClientKind::Antigravity => {
                Some(home.join(".gemini").join("config").join("mcp_config.json"))
            }
            AiClientKind::Custom(p) => Some(p.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiClientInfo {
    pub kind: AiClientKind,
    pub config_path: PathBuf,
    pub is_installed: bool,
    pub is_configured: bool,
    pub configured_command: Option<String>,
}

/// Scan local macOS environment for installed AI clients and check mso configuration state
pub fn scan_ai_clients() -> Vec<AiClientInfo> {
    let clients = vec![
        AiClientKind::Codex,
        AiClientKind::ClaudeDesktop,
        AiClientKind::Cursor,
        AiClientKind::VsCodeCline,
        AiClientKind::Antigravity,
    ];

    let mut results = Vec::new();
    for kind in clients {
        if let Some(config_path) = kind.default_config_path() {
            let is_installed = match &kind {
                AiClientKind::Antigravity => {
                    config_path.parent().map(|p| p.exists()).unwrap_or(false)
                        || config_path.exists()
                }
                _ => config_path.parent().map(|p| p.exists()).unwrap_or(false),
            };

            let mut is_configured = false;
            let mut configured_command = None;

            if kind == AiClientKind::Antigravity {
                if config_path.exists() && config_path.join("instructions.md").exists() {
                    is_configured = true;
                    configured_command = Some("mso mcp (Schema Directory)".into());
                }
            } else if config_path.exists() {
                if let Ok(content) = fs::read_to_string(&config_path) {
                    let is_toml = config_path.extension().and_then(|e| e.to_str()) == Some("toml")
                        || config_path.ends_with("config.toml");

                    if is_toml {
                        if content.contains("[mcp_servers.mso]") || content.contains("mcp_servers.mso") {
                            is_configured = true;
                            for line in content.lines() {
                                if line.trim().starts_with("command") && line.contains("mso") {
                                    if let Some(cmd_val) = line.split('=').nth(1) {
                                        configured_command = Some(cmd_val.trim().trim_matches('"').to_string());
                                    }
                                }
                            }
                            if configured_command.is_none() {
                                configured_command = Some("mso".to_string());
                            }
                        }
                    } else if let Ok(val) = serde_json::from_str::<Value>(&content) {
                        if let Some(mcp_servers) = val.get("mcpServers") {
                            if let Some(mso_entry) = mcp_servers.get("mso") {
                                is_configured = true;
                                if let Some(cmd) = mso_entry.get("command").and_then(|c| c.as_str()) {
                                    configured_command = Some(cmd.to_string());
                                }
                            }
                        }
                    }
                }
            }

            results.push(AiClientInfo {
                kind,
                config_path,
                is_installed,
                is_configured,
                configured_command,
            });
        }
    }

    results
}

/// Get the current mso executable path
pub fn get_mso_executable_path() -> String {
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(path_str) = current_exe.to_str() {
            return path_str.to_string();
        }
    }
    "/opt/homebrew/bin/mso".to_string()
}

/// Configure a specific AI client
pub fn configure_client(info: &AiClientInfo, overwrite: bool) -> Result<String> {
    let mso_exe = get_mso_executable_path();

    if info.kind == AiClientKind::Antigravity {
        let dir = &info.config_path;
        if !dir.exists() {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create directory {:?}", dir))?;
        }

        if info.is_configured && !overwrite {
            return Ok(format!(
                "Skipped Antigravity CLI (already configured at {:?})",
                dir
            ));
        }

        // Write instructions.md
        let instructions_content = r#"# mso (macOS Developer Storage Migrator) MCP Server Instructions

## 🤖 AI AGENT EXECUTION MANDATE
AI agents **MUST ALWAYS** use native MCP tools (`mso_get_status`, `mso_list_targets`, `mso_offload_target`, `mso_repair_*`, etc.) for all `mso` tasks instead of invoking shell/terminal commands (`mso ...`). Do NOT execute `mso` CLI shell commands unless native MCP tools are unreachable.

## ⚠️ EXPLICIT USER CONFIRMATION DIRECTIVE (MANDATORY FOR AI AGENTS)

When executing data relocation (`mso_offload_target`, `mso_offload_custom_folder`, `mso_offload_recommended`), reverse restores (`mso_restore_target`, `mso_restore_with_backup`), or conflict repairs (`mso_repair_*`), AI agents **MUST** follow this 2-step confirmation protocol:

### Step 1: Generate Dry-Run Preview
1. Call the target tool with `execute: false` (default).
2. Read the returned preview JSON containing byte sizes (`size_human`), local paths, and external SSD target paths.

### Step 2: Ask the User for Explicit Confirmation
1. Present the dry-run summary to the human user in natural text:
   - Target name & key (e.g. `DerivedData`)
   - Data size to be moved (e.g. `12.4 GB`)
   - Source local path and destination external SSD path
2. **Explicitly ask the user for confirmation**:
   > *"Would you like me to proceed with offloading 12.4 GB of DerivedData to /Volumes/ExtremeSSD/Developer/Xcode/DerivedData? Please confirm ('yes' or 'no')."*
3. **DO NOT** execute the mutation until the user explicitly responds with confirmation ("yes").
4. Once confirmed by the user, call the tool with `execute: true` to perform the operation.
"#;
        fs::write(dir.join("instructions.md"), instructions_content)?;

        // Write schema files
        let schemas = [
            ("mso_get_status", "Returns overall Mac disk telemetry and cache totals."),
            ("mso_list_all_targets", "Lists all 11 developer cache targets with byte sizes and state."),
            ("mso_list_disposable_targets", "Lists build output targets safe to offload."),
            ("mso_list_package_registries", "Lists package source registries."),
            ("mso_diagnose_conflicts", "Scans for broken symlinks and data conflicts."),
            ("mso_discover_ssd_caches", "Auto-discovers pre-existing SSD caches."),
            ("mso_offload_target", "Relocates a target key to external APFS SSD."),
            ("mso_offload_custom_folder", "Offloads custom local folder path entered by user."),
            ("mso_offload_recommended", "Batch offloads all disposable build outputs."),
            ("mso_restore_target", "Restores cache target back to Mac disk."),
            ("mso_restore_with_backup", "Restores cache target while keeping SSD copy."),
            ("mso_repair_merge", "Executes Safe Merge repair strategy."),
            ("mso_repair_overwrite_external", "Executes Overwrite External repair strategy."),
            ("mso_repair_discard_local", "Executes Discard Local repair strategy."),
            ("mso_repair_rollback_to_local", "Executes Rollback to Local repair strategy."),
            ("mso_repair_discard_external", "Executes Discard External repair strategy."),
            ("mso_repair_relink", "Executes Relink Stale Symlink repair strategy."),
            ("mso_get_config", "Reads saved CLI configuration."),
            ("mso_set_target_drive", "Updates default target drive and subfolder."),
            ("mso_reset_config", "Resets CLI configuration to default."),
        ];

        let target_dirs = if let Some(parent) = dir.parent() {
            vec![
                parent.join("mso"),
                parent.join("mac-sym-offload"),
                parent.join("MacSymOffload"),
            ]
        } else {
            vec![dir.clone()]
        };

        for target_dir in &target_dirs {
            if !target_dir.exists() {
                let _ = fs::create_dir_all(target_dir);
            }
            let _ = fs::write(target_dir.join("instructions.md"), instructions_content);

            for (name, desc) in schemas {
                let schema_json = json!({
                    "name": name,
                    "description": desc,
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                });
                let _ = fs::write(
                    target_dir.join(format!("{}.json", name)),
                    serde_json::to_string_pretty(&schema_json).unwrap_or_default(),
                );
            }
        }

        // Also update mcp_config.json for Antigravity stdio protocol
        if let Some(home) = dirs::home_dir() {
            let mcp_config_paths = vec![
                home.join(".gemini").join("config").join("mcp_config.json"),
                home.join(".gemini").join("antigravity-cli").join("mcp_config.json"),
            ];

            for mcp_config_path in mcp_config_paths {
                if let Some(parent) = mcp_config_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let doc: Value = if mcp_config_path.exists() {
                    let content = fs::read_to_string(&mcp_config_path).unwrap_or_default();
                    serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
                } else {
                    json!({})
                };

                let mut obj_map = match doc {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };

                let mcp_servers = obj_map
                    .entry("mcpServers".to_string())
                    .or_insert_with(|| json!({}));

                if !mcp_servers.is_object() {
                    *mcp_servers = json!({});
                }

                mcp_servers.as_object_mut().unwrap().insert(
                    "mso".to_string(),
                    json!({
                        "command": mso_exe.clone(),
                        "args": ["mcp"],
                        "env": {}
                    }),
                );

                let _ = fs::write(mcp_config_path, serde_json::to_string_pretty(&Value::Object(obj_map))?);
            }
        }

        return Ok(format!(
            "Successfully configured Antigravity CLI MCP schema directory & stdio config for {:?}",
            dir
        ));
    }

    let config_path = &info.config_path;
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    let is_toml = config_path.extension().and_then(|e| e.to_str()) == Some("toml")
        || config_path.ends_with("config.toml");

    if is_toml {
        let mut content = if config_path.exists() {
            fs::read_to_string(config_path)?
        } else {
            String::new()
        };

        if content.contains("[mcp_servers.mso]") {
            if !overwrite {
                return Ok(format!(
                    "Skipped {} (already configured at {:?})",
                    info.kind.display_name(),
                    config_path
                ));
            }
        } else {
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("\n[mcp_servers.mso]\ncommand = \"");
            content.push_str(&mso_exe);
            content.push_str("\"\nargs = [\"mcp\"]\n");
            fs::write(config_path, content)?;
        }

        return Ok(format!(
            "Successfully configured {} at {:?}",
            info.kind.display_name(),
            config_path
        ));
    }

    let doc: Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let mut obj_map = match doc {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    let mcp_servers = obj_map
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));

    if !mcp_servers.is_object() {
        *mcp_servers = json!({});
    }

    let mso_config = json!({
        "command": mso_exe,
        "args": ["mcp"]
    });

    mcp_servers
        .as_object_mut()
        .unwrap()
        .insert("mso".to_string(), mso_config);

    let final_doc = Value::Object(obj_map);
    fs::write(config_path, serde_json::to_string_pretty(&final_doc)?)?;

    Ok(format!(
        "Successfully configured {} at {:?}",
        info.kind.display_name(),
        config_path
    ))
}

/// Print status table or JSON output for `mso mcp status`
pub fn run_mcp_status(json_output: bool) -> Result<()> {
    let clients = scan_ai_clients();

    if json_output {
        let list: Vec<Value> = clients
            .iter()
            .map(|c| {
                json!({
                    "name": c.kind.display_name(),
                    "config_path": c.config_path.to_string_lossy(),
                    "is_installed": c.is_installed,
                    "is_configured": c.is_configured,
                    "configured_command": c.configured_command
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }

    println!("============================================================");
    println!("  mso - AI Agent MCP Configuration Status");
    println!("============================================================");
    println!();

    for c in &clients {
        let status_str = if c.is_configured {
            "[Configured]"
        } else if c.is_installed {
            "[Installed - Not Configured]"
        } else {
            "[Not Found]"
        };

        println!("• {:<36} {}", c.kind.display_name(), status_str);
        println!("  Config File: {:?}", c.config_path);
        if let Some(cmd) = &c.configured_command {
            println!("  Command:     {}", cmd);
        }
        println!();
    }

    Ok(())
}

/// Interactive TUI wizard for `mso mcp setup`
pub fn run_mcp_setup_wizard(force_overwrite: bool) -> Result<()> {
    let clients = scan_ai_clients();

    println!("============================================================");
    println!("  mso - AI Agent MCP Configuration Wizard");
    println!("============================================================");
    println!();
    println!("Scanning system for installed AI clients on your Mac...");
    println!();

    let mut options = Vec::new();
    for c in &clients {
        let status = if c.is_configured {
            "(Configured)"
        } else if c.is_installed {
            "(Installed)"
        } else {
            "(Available)"
        };
        options.push(format!("{} {}", c.kind.display_name(), status));
    }

    let selections = inquire::MultiSelect::new(
        "Select AI clients to configure for mso MCP server:",
        options.clone(),
    )
    .with_default(&[0, 1, 2, 4]) // Default to Codex, Claude, Cursor, Antigravity
    .prompt()?;

    if selections.is_empty() {
        println!("No AI clients selected. Exiting.");
        return Ok(());
    }

    for sel in selections {
        if let Some(idx) = options.iter().position(|o| o == &sel) {
            let client_info = &clients[idx];

            let mut overwrite = force_overwrite;
            if client_info.is_configured && !force_overwrite {
                let replace = inquire::Confirm::new(&format!(
                    "{} is ALREADY configured. Replace/update configuration with current binary?",
                    client_info.kind.display_name()
                ))
                .with_default(true)
                .prompt()?;

                if !replace {
                    println!("Skipping {}.", client_info.kind.display_name());
                    continue;
                }
                overwrite = true;
            }

            match configure_client(client_info, overwrite) {
                Ok(msg) => println!("✓ {}", msg),
                Err(e) => eprintln!("✗ Failed to configure {}: {}", client_info.kind.display_name(), e),
            }
        }
    }

    println!();
    println!("🎉 MCP Auto-Setup Complete!");
    println!("Restart your AI client (Codex, Claude, Cursor, Antigravity) to start using mso tools.");

    Ok(())
}
