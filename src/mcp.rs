use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Mutex, OnceLock};

use crate::assessment;
use crate::config::AppConfig;
use crate::discovery;
use crate::migrator;
use crate::models::{CacheTarget, ConflictStrategy, PathState};

// In-process lock: tracks target keys currently being migrated.
// Prevents concurrent MCP calls from racing on the same cache directory.
static ACTIVE_MIGRATIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_migrations() -> &'static Mutex<HashSet<String>> {
    ACTIVE_MIGRATIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

// Server-enforced dry-run tracking: stores target keys that have received a dry-run preview.
// Rejects execute: true calls unless a dry-run preview was generated first.
static PREVIEWED_TARGETS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn previewed_targets() -> &'static Mutex<HashSet<String>> {
    PREVIEWED_TARGETS.get_or_init(|| Mutex::new(HashSet::new()))
}

// Server-enforced dry-run tracking for restore operations
static PREVIEWED_RESTORES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn previewed_restores() -> &'static Mutex<HashSet<String>> {
    PREVIEWED_RESTORES.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn run_mcp_server() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                let err_resp = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                        data: None,
                    }),
                };
                let resp_str = serde_json::to_string(&err_resp)?;
                writeln!(stdout, "{}", resp_str)?;
                stdout.flush()?;
                continue;
            }
        };

        if request.id.is_none() && request.method.starts_with("notifications/") {
            continue;
        }

        let response = handle_mcp_request(&request);
        let resp_str = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", resp_str)?;
        stdout.flush()?;
    }

    Ok(())
}

fn handle_mcp_request(req: &JsonRpcRequest) -> JsonRpcResponse {
    let req_id = req.id.clone();

    match req.method.as_str() {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "mso",
                    "version": env!("CARGO_PKG_VERSION")
                }
            });
            JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(result),
                error: None,
            }
        }
        "tools/list" => {
            let tools = json!({
                "tools": [
                    {
                        "name": "mso_get_status",
                        "description": "Returns overall Mac disk telemetry, connected APFS volume status, free space metrics, and offloaded vs local cache summaries.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_status",
                        "description": "Alias for mso_get_status. Returns overall Mac disk telemetry and cache summaries.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_list_all_targets",
                        "description": "Lists all 11 supported developer cache targets with byte sizes, human-readable sizes, state (Fresh, AlreadyLinked, GhostLocal, Conflict), and paths.",
                        "inputSchema": { "type": "object", "properties": { "drive_path": { "type": "string" } }, "required": [] }
                    },
                    {
                        "name": "mso_list_targets",
                        "description": "Lists supported developer cache targets with category filtering.",
                        "inputSchema": { "type": "object", "properties": { "category": { "type": "string", "enum": ["disposable", "package_registry", "all"] } }, "required": [] }
                    },
                    {
                        "name": "mso_list_disposable_targets",
                        "description": "Lists only build output targets safe to offload (DerivedData, .gradle, Archives, CoreSimulator, iOS DeviceSupport, Android SDK).",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_list_package_registries",
                        "description": "Lists package source registries (.pub-cache, .npm, .cargo, .m2, .cocoapods) recommended to stay local for offline development.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_diagnose_conflicts",
                        "description": "Scans for data drift, unmounted external SSD symlinks (GhostLocal), or dual-location copies (Conflict).",
                        "inputSchema": { "type": "object", "properties": { "drive_path": { "type": "string" } }, "required": [] }
                    },
                    {
                        "name": "mso_diagnose",
                        "description": "Alias for mso_diagnose_conflicts. Scans for broken symlinks and data conflicts.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_discover_ssd_caches",
                        "description": "Scans an attached SSD for pre-existing offloaded directories to auto-recover symlinks.",
                        "inputSchema": { "type": "object", "properties": { "drive_path": { "type": "string" } }, "required": [] }
                    },
                    {
                        "name": "mso_offload_target",
                        "description": "Previews or executes relocation of a built-in cache target key to external APFS SSD. Call with execute: false (default) first to preview.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string", "description": "Target key (e.g. 'derived-data', 'gradle', 'xcode-archives', 'coresimulator', 'ios-device-support', 'pub-cache', 'npm', 'cargo', 'cocoapods', 'maven')" },
                                "execute": { "type": "boolean", "default": false, "description": "Set to true to commit offload. Defaults to false (dry-run preview)." },
                                "drive_path": { "type": "string", "description": "User's manual external SSD volume path" },
                                "subfolder_path": { "type": "string", "description": "User's manual target subfolder on SSD (e.g. 'DevCaches/Offload')" },
                                "conflict_strategy": { "type": "string", "enum": ["merge", "overwrite_external", "discard_local"] }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_offload_custom_folder",
                        "description": "Offloads an arbitrary local folder path entered manually by the user (e.g. ~/Library/Caches/Docker) to external APFS SSD.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "custom_local_path": { "type": "string", "description": "Absolute or ~/ relative local directory path entered manually by user" },
                                "custom_name": { "type": "string", "description": "Optional human-readable label for custom target" },
                                "execute": { "type": "boolean", "default": false, "description": "Set to true to commit. Defaults to false (preview)." },
                                "drive_path": { "type": "string", "description": "User's manual external SSD volume path" }
                            },
                            "required": ["custom_local_path"]
                        }
                    },
                    {
                        "name": "mso_offload_recommended",
                        "description": "Auto-selects and offloads all disposable build outputs in a single batch operation.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "mso_restore_target",
                        "description": "Restores an offloaded cache target from SSD back to local Mac storage and removes symlink. Gated by 10GB disk space safety check.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string", "description": "Target key to restore back to local Mac storage" },
                                "keep_external": { "type": "boolean", "default": false, "description": "Keep copy on external SSD after restoring" },
                                "execute": { "type": "boolean", "default": false, "description": "Set to true to commit restore. Defaults to false (preview)." }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_restore_with_backup",
                        "description": "Restores an offloaded target back to Mac storage while retaining a backup copy on external SSD.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string", "description": "Target key to restore" },
                                "execute": { "type": "boolean", "default": false }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_merge",
                        "description": "Executes Safe Merge strategy (syncs missing files to SSD and frees local space).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_overwrite_external",
                        "description": "Executes Overwrite External strategy (wipes SSD copy and re-copies fresh from Mac storage).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_discard_local",
                        "description": "Executes Discard Local strategy (deletes local Mac folder and re-establishes symlink to SSD copy).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_rollback_to_local",
                        "description": "Executes Rollback SSD Data strategy (copies SSD data back to Mac disk and deletes SSD copy).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_discard_external",
                        "description": "Executes Discard External strategy (deletes SSD copy and keeps local Mac folder untouched).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_repair_relink",
                        "description": "Executes Relink strategy (removes stale/broken symlink and recreates it pointing to current expected external path).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": { "type": "string" },
                                "execute": { "type": "boolean", "default": false },
                                "drive_path": { "type": "string" }
                            },
                            "required": ["target_key"]
                        }
                    },
                    {
                        "name": "mso_get_config",
                        "description": "Reads saved configuration preferences from ~/.config/mso/config.json.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "mso_set_target_drive",
                        "description": "Updates default external APFS volume mount path and subfolder location in configuration.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "drive_path": { "type": "string", "description": "User's manual external drive path" },
                                "subfolder_path": { "type": "string", "description": "User's manual subfolder on drive" }
                            },
                            "required": ["drive_path"]
                        }
                    },
                    {
                        "name": "mso_reset_config",
                        "description": "Resets configuration file to clean default state.",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    }
                ]
            });
            JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(tools),
                error: None,
            }
        }
        "tools/call" => {
            let params = match req.params.as_ref() {
                Some(p) => p,
                None => {
                    return JsonRpcResponse {
                        jsonrpc: "2.0",
                        id: req_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Missing params".into(),
                            data: None,
                        }),
                    }
                }
            };

            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let progress_token = params.get("_meta").and_then(|m| m.get("progressToken")).cloned();

            match tool_name {
                "mso_status" | "mso_get_status" => handle_tool_status(req_id),
                "mso_list_targets" | "mso_list_all_targets" => handle_tool_list_targets(req_id, &arguments),
                "mso_list_disposable_targets" => handle_tool_list_disposable_targets(req_id),
                "mso_list_package_registries" => handle_tool_list_package_registries(req_id),
                "mso_diagnose" | "mso_diagnose_conflicts" => handle_tool_diagnose(req_id),
                "mso_discover_ssd_caches" => handle_tool_discover_ssd_caches(req_id, &arguments),
                "mso_offload_target" => handle_tool_offload_target(req_id, &arguments, progress_token),
                "mso_offload_custom_folder" => handle_tool_offload_custom_folder(req_id, &arguments, progress_token),
                "mso_offload_recommended" => handle_tool_offload_recommended(req_id, &arguments, progress_token),
                "mso_restore_target" => handle_tool_restore_target(req_id, &arguments, false, progress_token),
                "mso_restore_with_backup" => handle_tool_restore_target(req_id, &arguments, true, progress_token),
                "mso_repair_merge" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::Merge, progress_token),
                "mso_repair_overwrite_external" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::OverwriteExternal, progress_token),
                "mso_repair_discard_local" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::DiscardLocal, progress_token),
                "mso_repair_rollback_to_local" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::RollbackExternalToLocal, progress_token),
                "mso_repair_discard_external" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::KeepLocalDiscardExternal, progress_token),
                "mso_relink" | "mso_repair_relink" => handle_tool_repair_strategy(req_id, &arguments, ConflictStrategy::Relink, progress_token),
                "mso_get_config" => handle_tool_get_config(req_id),
                "mso_set_target_drive" => handle_tool_set_target_drive(req_id, &arguments),
                "mso_reset_config" => handle_tool_reset_config(req_id),
                _ => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: req_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Unknown tool: {}", tool_name),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
                data: None,
            }),
        },
    }
}

fn handle_tool_status(req_id: Option<Value>) -> JsonRpcResponse {
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = config
        .last_external_drive
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| drives.first().map(|d| d.volume_path.clone()))
        .unwrap_or_else(|| std::path::PathBuf::from("/Volumes/ExternalSSD"));

    let drive_validation = discovery::inspect_drive_apfs(&drive_path);

    let mut local_count = 0;
    let mut offloaded_count = 0;
    let mut ghost_count = 0;
    let mut conflict_count = 0;
    let mut total_offloaded_bytes: u64 = 0;
    let mut total_local_bytes: u64 = 0;

    for target in CacheTarget::all() {
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            match info.state {
                PathState::Fresh | PathState::NotFound => {
                    local_count += 1;
                    total_local_bytes += info.size_bytes;
                }
                PathState::AlreadyLinked { .. } | PathState::ExistingExternalData { .. } => {
                    offloaded_count += 1;
                    total_offloaded_bytes += info.size_bytes;
                }
                PathState::GhostLocal { .. } => ghost_count += 1,
                PathState::StaleSymlink { .. } | PathState::Conflict { .. } | PathState::RebindDrive { .. } => {
                    conflict_count += 1;
                    total_local_bytes += info.size_bytes;
                }
            }
        }
    }

    let payload = json!({
        "config": {
            "saved_drive": config.last_external_drive,
            "remembered_targets": config.remembered_targets,
        },
        "drive": match drive_validation {
            Ok(info) => json!({
                "is_connected": true,
                "volume_name": info.name,
                "mount_path": info.volume_path.to_string_lossy(),
                "file_system": info.file_system
            }),
            Err(e) => json!({
                "is_connected": false,
                "error": e.to_string()
            })
        },
        "summary": {
            "offloaded_targets_count": offloaded_count,
            "local_targets_count": local_count,
            "ghost_links_count": ghost_count,
            "conflict_targets_count": conflict_count,
            "offloaded_bytes": total_offloaded_bytes,
            "offloaded_human": format_bytes(total_offloaded_bytes),
            "local_cache_bytes": total_local_bytes,
            "local_cache_human": format_bytes(total_local_bytes)
        }
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string_pretty(&payload).unwrap_or_default()
                }
            ]
        })),
        error: None,
    }
}

fn handle_tool_list_targets(req_id: Option<Value>, args: &Value) -> JsonRpcResponse {
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = config
        .last_external_drive
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| drives.first().map(|d| d.volume_path.clone()))
        .unwrap_or_else(|| std::path::PathBuf::from("/Volumes/ExternalSSD"));

    let category_filter = args.get("category").and_then(|v| v.as_str()).unwrap_or("all");

    let mut targets_list = Vec::new();
    for target in CacheTarget::all() {
        let is_disposable = target.is_build_output();
        if match category_filter {
            "disposable" => is_disposable,
            "package_registry" => !is_disposable,
            _ => true,
        } {
            if let Ok(info) = assessment::assess_target(&target, &drive_path) {
                targets_list.push(json!({
                    "key": target.key(),
                    "name": target.display_name(),
                    "local_path": info.local_path.to_string_lossy(),
                    "external_path": info.external_path.to_string_lossy(),
                    "state": format!("{:?}", info.state),
                    "size_bytes": info.size_bytes,
                    "size_human": format_bytes(info.size_bytes),
                    "is_disposable": is_disposable,
                    "recommendation": if is_disposable {
                        "Safe to offload (disposable build output)"
                    } else {
                        "Package source registry (recommend keeping local for offline dev)"
                    }
                }));
            }
        }
    }

    let payload = json!({
        "targets_count": targets_list.len(),
        "targets": targets_list
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string_pretty(&payload).unwrap_or_default()
                }
            ]
        })),
        error: None,
    }
}

fn handle_tool_diagnose(req_id: Option<Value>) -> JsonRpcResponse {
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = config
        .last_external_drive
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| drives.first().map(|d| d.volume_path.clone()))
        .unwrap_or_else(|| std::path::PathBuf::from("/Volumes/ExternalSSD"));

    let mut issues = Vec::new();
    for target in CacheTarget::all() {
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            if matches!(info.state, PathState::GhostLocal { .. } | PathState::StaleSymlink { .. } | PathState::Conflict { .. }) {
                let (description, available_strats) = match &info.state {
                    PathState::StaleSymlink { current_target, expected_target } => (
                        format!("Stale symlink: points to '{}', but expected path template is '{}'. Recommend relinking with mso_repair_relink.", current_target.display(), expected_target.display()),
                        vec!["relink", "discard_local", "rollback_to_local"]
                    ),
                    PathState::GhostLocal { .. } => (
                        "Disconnected drive: external SSD volume is unattached or missing target directory. Reconnect drive or restore backup.".to_string(),
                        vec!["reconnect", "rollback_to_local"]
                    ),
                    PathState::Conflict { .. } => (
                        "Data conflict: local Mac directory exists AND external SSD directory exists.".to_string(),
                        vec!["merge", "overwrite_external", "discard_local", "rollback_to_local"]
                    ),
                    _ => ("".to_string(), vec![])
                };

                issues.push(json!({
                    "key": target.key(),
                    "name": target.display_name(),
                    "state": format!("{:?}", info.state),
                    "local_path": info.local_path.to_string_lossy(),
                    "external_path": info.external_path.to_string_lossy(),
                    "size_bytes": info.size_bytes,
                    "size_human": format_bytes(info.size_bytes),
                    "description": description,
                    "available_strategies": available_strats
                }));
            }
        }
    }

    let payload = json!({
        "issues_found": issues.len(),
        "issues": issues
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string_pretty(&payload).unwrap_or_default()
                }
            ]
        })),
        error: None,
    }
}

fn emit_mcp_progress(progress_token: &Value, progress_bytes: u64, total_bytes: u64, message: &str) {
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": progress_token,
            "progress": progress_bytes,
            "total": total_bytes,
            "message": message
        }
    });

    if let Ok(notif_str) = serde_json::to_string(&notification) {
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{}", notif_str);
        let _ = stdout.flush();
    }
}

fn handle_tool_offload_target(
    req_id: Option<Value>,
    args: &Value,
    progress_token: Option<Value>,
) -> JsonRpcResponse {
    // Step 1: Validate user-supplied params first (-32602 for all param errors)
    let target_key = match args.get("target_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Missing target_key argument".into(),
                    data: None,
                }),
            };
        }
    };

    // Dry-run gate: execute defaults to false.
    // Agents MUST call with execute: false first (returns a preview),
    // then make a second explicit call with execute: true to commit.
    // This prevents any agent from autonomously moving/deleting data in a single call.
    let execute = args.get("execute").and_then(|v| v.as_bool()).unwrap_or(false);

    // Step 2: Resolve target by key before touching the filesystem
    let target = match CacheTarget::all().into_iter().find(|t| t.key() == target_key) {
        Some(t) => t,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!(
                        "Unknown target_key '{}'. Run mso_list_targets to see valid keys.",
                        target_key
                    ),
                    data: None,
                }),
            };
        }
    };

    let strat_str = args.get("conflict_strategy").and_then(|v| v.as_str()).unwrap_or("merge");
    let conflict_strat = match strat_str {
        "overwrite_external" => ConflictStrategy::OverwriteExternal,
        "discard_local" => ConflictStrategy::DiscardLocal,
        "rollback_to_local" | "rollback_external_to_local" => ConflictStrategy::RollbackExternalToLocal,
        "relink" => ConflictStrategy::Relink,
        "discard_external" | "keep_local_discard_external" => ConflictStrategy::KeepLocalDiscardExternal,
        _ => ConflictStrategy::Merge,
    };

    // Step 3: Resolve drive path (config fallback chain)
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = args
        .get("drive_path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .or_else(|| config.last_external_drive.as_ref().map(std::path::PathBuf::from))
        .or_else(|| drives.first().map(|d| d.volume_path.clone()));

    let drive_path = match drive_path {
        Some(p) => p,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "No external APFS drive specified in arguments or saved config".into(),
                    data: None,
                }),
            };
        }
    };

    // SERVER-ENFORCED GATE: Reject execute: true if no preceding dry-run preview call occurred
    if execute {
        let previewed = previewed_targets().lock().unwrap_or_else(|e| e.into_inner());
        if !previewed.contains(target_key) {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32002, // Implementation-defined error: Dry-run required
                    message: format!(
                        "Dry-run preview required prior to execution. Call mso_offload_target for '{}' with execute: false first.",
                        target_key
                    ),
                    data: None,
                }),
            };
        }
    }

    // Step 4: Validate APFS filesystem (-32603 for system-level failures)
    if let Err(e) = discovery::validate_apfs_drive(&drive_path) {
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603,
                message: format!("Drive validation failed: {}", e),
                data: None,
            }),
        };
    }

    let info = match assessment::assess_target(&target, &drive_path) {
        Ok(i) => i,
        Err(e) => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: format!("Assessment failed: {}", e),
                    data: None,
                }),
            };
        }
    };

    let size_bytes = info.size_bytes;

    // Dry-run gate: execute defaults to false.
    // Server enforces that execute: true calls MUST be preceded by a call with execute: false.
    if !execute {
        // Record that a dry-run preview was generated for this target key
        {
            let mut previewed = previewed_targets().lock().unwrap_or_else(|e| e.into_inner());
            previewed.insert(target_key.to_string());
        }

        let (warning_msg, _action_msg) = match conflict_strat {
            ConflictStrategy::RollbackExternalToLocal => (
                format!(
                    "This will restore '{}' ({}) from external SSD back to local Mac storage and delete the external backup. Re-call with execute: true to commit.",
                    info.local_path.to_string_lossy(),
                    format_bytes(size_bytes)
                ),
                format!("Successfully rolled back {} from external APFS SSD to local Mac storage.", target.display_name())
            ),
            ConflictStrategy::Relink => (
                format!(
                    "This will replace local symlink '{}' with an updated symlink pointing to '{}'. Re-call with execute: true to commit.",
                    info.local_path.to_string_lossy(),
                    info.external_path.to_string_lossy()
                ),
                format!("Successfully updated symlink for {}.", target.display_name())
            ),
            ConflictStrategy::DiscardLocal => (
                format!(
                    "This will permanently delete local Mac directory '{}' ({}) and establish a symlink to existing external SSD backup. Re-call with execute: true to commit.",
                    info.local_path.to_string_lossy(),
                    format_bytes(size_bytes)
                ),
                format!("Successfully discarded local copy and established symlink for {}.", target.display_name())
            ),
            ConflictStrategy::KeepLocalDiscardExternal => (
                format!(
                    "This will permanently delete external SSD backup '{}' ({}) and leave local Mac directory untouched. Re-call with execute: true to commit.",
                    info.external_path.to_string_lossy(),
                    format_bytes(size_bytes)
                ),
                format!("Successfully discarded external SSD backup for {}.", target.display_name())
            ),
            ConflictStrategy::Merge => (
                format!(
                    "This will merge local Mac directory '{}' ({}) into external SSD backup and establish a symlink. Re-call with execute: true to commit.",
                    info.local_path.to_string_lossy(),
                    format_bytes(size_bytes)
                ),
                format!("Successfully merged {} into external APFS SSD and established symlink.", target.display_name())
            ),
            ConflictStrategy::OverwriteExternal => (
                format!(
                    "This will overwrite external SSD backup with local Mac directory '{}' ({}) and establish a symlink. Re-call with execute: true to commit.",
                    info.local_path.to_string_lossy(),
                    format_bytes(size_bytes)
                ),
                format!("Successfully overwrote external SSD backup and established symlink for {}.", target.display_name())
            ),
        };

        let preview = json!({
            "dry_run": true,
            "target_key": target.key(),
            "target_name": target.display_name(),
            "current_state": format!("{:?}", info.state),
            "would_move_bytes": size_bytes,
            "would_move_human": format_bytes(size_bytes),
            "local_path": info.local_path.to_string_lossy(),
            "external_path": info.external_path.to_string_lossy(),
            "conflict_strategy": strat_str,
            "warning": warning_msg
        });
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: Some(json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&preview).unwrap_or_default()
                    }
                ]
            })),
            error: None,
        };
    }

    // Concurrency guard: reject if this target is already being migrated
    // by another concurrent MCP tool call in this server process.
    {
        let mut active = active_migrations().lock().unwrap_or_else(|e| e.into_inner());
        if active.contains(target_key) {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32001, // Implementation-defined error: Resource Busy
                    message: format!(
                        "Target '{}' is already being migrated. Wait for the active transfer to complete.",
                        target_key
                    ),
                    data: None,
                }),
            };
        }
        active.insert(target_key.to_string());
    }

    if let Some(token) = &progress_token {
        emit_mcp_progress(
            token,
            0,
            size_bytes,
            &format!("Initializing operation for {}...", target.display_name()),
        );
    }

    // Execute non-interactive migration
    let result = migrator::migrate_target(&info, Some(conflict_strat), false, false);

    // Always release the concurrency lock, even on failure
    {
        let mut active = active_migrations().lock().unwrap_or_else(|e| e.into_inner());
        active.remove(target_key);
    }

    // On completion, clear dry-run preview state so future operations require a fresh preview
    {
        let mut previewed = previewed_targets().lock().unwrap_or_else(|e| e.into_inner());
        previewed.remove(target_key);
    }

    match result {
        Ok(_) => {
            // Re-assess target to verify actual disk result and byte sizes
            let post_info = assessment::assess_target(&target, &drive_path).ok();
            let actual_bytes = post_info.as_ref().map(|i| i.size_bytes).unwrap_or(size_bytes);

            let (exec_warning_msg, exec_action_msg) = match conflict_strat {
                ConflictStrategy::RollbackExternalToLocal => (
                    format!("Restored {} back to local Mac disk.", target.display_name()),
                    format!("Successfully rolled back {} from external APFS SSD to local Mac storage.", target.display_name())
                ),
                ConflictStrategy::Relink => (
                    format!("Updated symlink for {}.", target.display_name()),
                    format!("Successfully updated symlink for {}.", target.display_name())
                ),
                _ => (
                    format!("Offloaded {} to external SSD.", target.display_name()),
                    format!("Successfully offloaded {} to external APFS SSD and established symlink.", target.display_name())
                ),
            };

            if let Some(token) = &progress_token {
                emit_mcp_progress(
                    token,
                    actual_bytes,
                    actual_bytes,
                    &exec_warning_msg,
                );
            }

            let res = json!({
                "success": true,
                "target_key": target.key(),
                "target_name": target.display_name(),
                "bytes_moved": actual_bytes,
                "bytes_moved_human": format_bytes(actual_bytes),
                "local_path": info.local_path.to_string_lossy(),
                "external_path": info.external_path.to_string_lossy(),
                "message": exec_action_msg
            });
            JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(json!({
                    "content": [
                        {
                            "type": "text",
                            "text": serde_json::to_string_pretty(&res).unwrap_or_default()
                        }
                    ]
                })),
                error: None,
            }
        }
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603,
                message: format!("Migration failed for '{}': {}", target.key(), e),
                data: None,
            }),
        },
    }
}

fn handle_tool_list_disposable_targets(req_id: Option<Value>) -> JsonRpcResponse {
    let args = json!({ "category": "disposable" });
    handle_tool_list_targets(req_id, &args)
}

fn handle_tool_list_package_registries(req_id: Option<Value>) -> JsonRpcResponse {
    let args = json!({ "category": "package_registry" });
    handle_tool_list_targets(req_id, &args)
}

fn handle_tool_discover_ssd_caches(req_id: Option<Value>, args: &Value) -> JsonRpcResponse {
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = args
        .get("drive_path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .or_else(|| config.last_external_drive.as_ref().map(std::path::PathBuf::from))
        .or_else(|| drives.first().map(|d| d.volume_path.clone()));

    let drive_path = match drive_path {
        Some(p) => p,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "No external APFS drive specified".into(),
                    data: None,
                }),
            };
        }
    };

    let mut discovered = Vec::new();
    for target in CacheTarget::all() {
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            if matches!(info.state, PathState::ExistingExternalData { .. }) {
                discovered.push(json!({
                    "key": target.key(),
                    "name": target.display_name(),
                    "external_path": info.external_path.to_string_lossy(),
                    "size_bytes": info.size_bytes,
                    "size_human": format_bytes(info.size_bytes),
                    "action_recommendation": "Discovered on SSD! Ready to re-bind symlink."
                }));
            }
        }
    }

    let payload = json!({
        "discovered_count": discovered.len(),
        "discovered_caches": discovered
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]
        })),
        error: None,
    }
}

fn handle_tool_offload_custom_folder(
    req_id: Option<Value>,
    args: &Value,
    progress_token: Option<Value>,
) -> JsonRpcResponse {
    let custom_path_str = match args.get("custom_local_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Missing custom_local_path argument".into(),
                    data: None,
                }),
            };
        }
    };

    let expanded_path = if custom_path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(&custom_path_str[2..])
        } else {
            std::path::PathBuf::from(custom_path_str)
        }
    } else {
        std::path::PathBuf::from(custom_path_str)
    };

    if !expanded_path.exists() || !expanded_path.is_dir() {
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Custom path '{}' does not exist or is not a directory.", expanded_path.display()),
                data: None,
            }),
        };
    }

    let name = args
        .get("custom_name")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            expanded_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("CustomFolder")
        })
        .to_string();

    let custom_target = CacheTarget::Custom {
        name: name.clone(),
        local_rel_path: expanded_path.clone(),
    };

    let target_key = custom_target.key();
    let mut modified_args = args.clone();
    if let Some(obj) = modified_args.as_object_mut() {
        obj.insert("target_key".to_string(), json!(target_key));
    }

    handle_tool_offload_target(req_id, &modified_args, progress_token)
}

fn handle_tool_offload_recommended(
    req_id: Option<Value>,
    args: &Value,
    progress_token: Option<Value>,
) -> JsonRpcResponse {
    let execute = args.get("execute").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut offload_results = Vec::new();

    for target in CacheTarget::all().into_iter().filter(|t| t.is_build_output()) {
        let mut target_args = args.clone();
        if let Some(obj) = target_args.as_object_mut() {
            obj.insert("target_key".to_string(), json!(target.key()));
            obj.insert("execute".to_string(), json!(execute));
        }

        let resp = handle_tool_offload_target(req_id.clone(), &target_args, progress_token.clone());
        if resp.error.is_none() {
            offload_results.push(resp.result);
        }
    }

    let payload = json!({
        "batch_offload": true,
        "execute": execute,
        "processed_count": offload_results.len(),
        "results": offload_results
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]
        })),
        error: None,
    }
}

fn handle_tool_restore_target(
    req_id: Option<Value>,
    args: &Value,
    force_keep_external: bool,
    _progress_token: Option<Value>,
) -> JsonRpcResponse {
    let target_key = match args.get("target_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Missing target_key argument".into(),
                    data: None,
                }),
            };
        }
    };

    let execute = args.get("execute").and_then(|v| v.as_bool()).unwrap_or(false);
    let keep_external = force_keep_external || args.get("keep_external").and_then(|v| v.as_bool()).unwrap_or(false);

    let target = match CacheTarget::all().into_iter().find(|t| t.key() == target_key) {
        Some(t) => t,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Unknown target_key '{}'", target_key),
                    data: None,
                }),
            };
        }
    };

    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = config
        .last_external_drive
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| drives.first().map(|d| d.volume_path.clone()))
        .unwrap_or_else(|| std::path::PathBuf::from("/Volumes/ExternalSSD"));

    let info = match assessment::assess_target(&target, &drive_path) {
        Ok(i) => i,
        Err(e) => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: format!("Assessment failed: {}", e),
                    data: None,
                }),
            };
        }
    };

    // Evaluate 10GB disk space safety buffer
    let avail_bytes = crate::ui::get_mac_available_space_bytes().unwrap_or(u64::MAX);
    let required_with_buffer = info.size_bytes + 10_000_000_000;
    if avail_bytes != u64::MAX && required_with_buffer > avail_bytes {
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32003, // Implementation-defined error: Insufficient space
                message: format!(
                    "Insufficient local Mac space to restore '{}'. Required: {} (target size {}) + 10GB safety buffer, Available: {}.",
                    target.display_name(),
                    format_bytes(required_with_buffer),
                    format_bytes(info.size_bytes),
                    format_bytes(avail_bytes)
                ),
                data: None,
            }),
        };
    }

    if !execute {
        {
            let mut previewed = previewed_restores().lock().unwrap_or_else(|e| e.into_inner());
            previewed.insert(target_key.to_string());
        }

        let preview = json!({
            "dry_run": true,
            "target_key": target.key(),
            "target_name": target.display_name(),
            "restore_bytes": info.size_bytes,
            "restore_human": format_bytes(info.size_bytes),
            "mac_available_bytes": avail_bytes,
            "mac_available_human": format_bytes(avail_bytes),
            "keep_external_copy": keep_external,
            "warning": format!(
                "This will copy {} ({}) from external SSD back to local Mac storage and remove the symlink. Re-call with execute: true to commit.",
                target.display_name(),
                format_bytes(info.size_bytes)
            )
        });
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: Some(json!({
                "content": [{"type": "text", "text": serde_json::to_string_pretty(&preview).unwrap_or_default()}]
            })),
            error: None,
        };
    }

    {
        let previewed = previewed_restores().lock().unwrap_or_else(|e| e.into_inner());
        if !previewed.contains(target_key) {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32002,
                    message: format!("Dry-run preview required prior to restore execution for '{}'.", target_key),
                    data: None,
                }),
            };
        }
    }

    let result = migrator::restore_target(&info, keep_external, false, false);

    {
        let mut previewed = previewed_restores().lock().unwrap_or_else(|e| e.into_inner());
        previewed.remove(target_key);
    }

    match result {
        Ok(_) => {
            let res = json!({
                "success": true,
                "target_key": target.key(),
                "target_name": target.display_name(),
                "restored_bytes": info.size_bytes,
                "restored_human": format_bytes(info.size_bytes),
                "keep_external": keep_external,
                "message": format!("Successfully restored {} back to local Mac storage.", target.display_name())
            });
            JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&res).unwrap_or_default()}]
                })),
                error: None,
            }
        }
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603,
                message: format!("Restore failed for '{}': {}", target.key(), e),
                data: None,
            }),
        },
    }
}

fn handle_tool_repair_strategy(
    req_id: Option<Value>,
    args: &Value,
    strategy: ConflictStrategy,
    progress_token: Option<Value>,
) -> JsonRpcResponse {
    let mut modified_args = args.clone();
    let strat_name = match strategy {
        ConflictStrategy::Merge => "merge",
        ConflictStrategy::OverwriteExternal => "overwrite_external",
        ConflictStrategy::DiscardLocal => "discard_local",
        ConflictStrategy::KeepLocalDiscardExternal => "discard_external",
        ConflictStrategy::RollbackExternalToLocal => "rollback_to_local",
        ConflictStrategy::Relink => "relink",
    };

    if let Some(obj) = modified_args.as_object_mut() {
        obj.insert("conflict_strategy".to_string(), json!(strat_name));
    }

    handle_tool_offload_target(req_id, &modified_args, progress_token)
}

fn handle_tool_get_config(req_id: Option<Value>) -> JsonRpcResponse {
    let config = AppConfig::load();
    let payload = json!({
        "config_path": "~/.config/mso/config.json",
        "last_external_drive": config.last_external_drive,
        "remembered_targets": config.remembered_targets
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]
        })),
        error: None,
    }
}

fn handle_tool_set_target_drive(req_id: Option<Value>, args: &Value) -> JsonRpcResponse {
    let drive_path = match args.get("drive_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Missing drive_path argument".into(),
                    data: None,
                }),
            };
        }
    };

    let mut config = AppConfig::load();
    config.last_external_drive = Some(drive_path.to_string());
    let _ = config.save();

    let payload = json!({
        "success": true,
        "saved_drive": drive_path,
        "message": format!("Successfully saved target drive path to configuration: {}", drive_path)
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]
        })),
        error: None,
    }
}

fn handle_tool_reset_config(req_id: Option<Value>) -> JsonRpcResponse {
    let config_path = match dirs::config_dir() {
        Some(dir) => dir.join("mso").join("config.json"),
        None => std::path::PathBuf::from("~/.config/mso/config.json"),
    };

    if config_path.exists() {
        let _ = std::fs::remove_file(&config_path);
    }

    let payload = json!({
        "success": true,
        "message": "Successfully reset mso configuration to default."
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id: req_id,
        result: Some(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default()}]
        })),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(1048576 * 50), "50.0 MB");
        assert_eq!(format_bytes(1073741824 * 52), "52.0 GB");
    }

    #[test]
    fn test_mcp_initialize() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: None,
        };

        let resp = handle_mcp_request(&req);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.error.is_none());

        let res = resp.result.unwrap();
        assert_eq!(res["protocolVersion"], "2024-11-05");
        assert_eq!(res["serverInfo"]["name"], "mso");
    }

    #[test]
    fn test_mcp_offload_dry_run_default() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "mso_offload_target",
                "arguments": {
                    "target_key": "derived-data",
                    "drive_path": "/"
                }
            })),
        };

        let resp = handle_mcp_request(&req);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(3)));
        assert!(resp.error.is_none(), "Unexpected error: {:?}", resp.error);

        let res = resp.result.unwrap();
        let text = res["content"][0]["text"].as_str().unwrap();
        let preview: Value = serde_json::from_str(text).unwrap();

        assert_eq!(preview["dry_run"], true);
        assert_eq!(preview["target_key"], "derived-data");
        assert!(preview["warning"].as_str().unwrap().contains("execute: true"));
    }

    #[test]
    fn test_mcp_execute_without_preview_fails() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(4)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "mso_offload_target",
                "arguments": {
                    "target_key": "coresimulator",
                    "execute": true,
                    "drive_path": "/"
                }
            })),
        };

        let resp = handle_mcp_request(&req);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(4)));
        let err = resp.error.expect("Direct execute: true without preview must fail");
        assert_eq!(err.code, -32002);
        assert!(err.message.contains("Dry-run preview required prior to execution"));
    }

    #[test]
    fn test_mcp_tools_list() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/list".into(),
            params: None,
        };

        let resp = handle_mcp_request(&req);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(2)));

        let res = resp.result.unwrap();
        let tools = res["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 23);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mso_get_status"));
        assert!(names.contains(&"mso_repair_relink"));
        assert!(names.contains(&"mso_list_all_targets"));
        assert!(names.contains(&"mso_list_disposable_targets"));
        assert!(names.contains(&"mso_list_package_registries"));
        assert!(names.contains(&"mso_diagnose_conflicts"));
        assert!(names.contains(&"mso_discover_ssd_caches"));
        assert!(names.contains(&"mso_offload_target"));
        assert!(names.contains(&"mso_offload_custom_folder"));
        assert!(names.contains(&"mso_offload_recommended"));
        assert!(names.contains(&"mso_restore_target"));
        assert!(names.contains(&"mso_restore_with_backup"));
        assert!(names.contains(&"mso_repair_merge"));
        assert!(names.contains(&"mso_repair_overwrite_external"));
        assert!(names.contains(&"mso_repair_discard_local"));
        assert!(names.contains(&"mso_repair_rollback_to_local"));
        assert!(names.contains(&"mso_repair_discard_external"));
        assert!(names.contains(&"mso_get_config"));
        assert!(names.contains(&"mso_set_target_drive"));
        assert!(names.contains(&"mso_reset_config"));
    }

    #[test]
    fn test_mcp_rollback_to_local_dry_run() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(5)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "mso_repair_rollback_to_local",
                "arguments": {
                    "target_key": "coresimulator",
                    "execute": false,
                    "drive_path": "/"
                }
            })),
        };

        let resp = handle_mcp_request(&req);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(5)));

        let res = resp.result.unwrap();
        let text = res["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("rollback_to_local"));
        assert!(text.contains("restore"));
        assert!(text.contains("from external SSD back to local Mac storage"));
    }
}
