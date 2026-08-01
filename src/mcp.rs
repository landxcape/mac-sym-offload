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
                        "name": "mso_status",
                        "description": "Returns overall Mac disk telemetry, external APFS volume status, and summary counts of offloaded vs local developer target paths.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "mso_list_targets",
                        "description": "Lists all supported developer cache targets (DerivedData, .gradle, Archives, CoreSimulator, iOS DeviceSupport, .pub-cache, .npm, etc.) with byte sizes, human-readable sizes, state (Fresh, AlreadyLinked, GhostLocal, Conflict), and safety recommendations.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "category": {
                                    "type": "string",
                                    "description": "Filter by target category: 'disposable' (build outputs safe to offload), 'package_registry' (source dependencies), or 'all' (default)",
                                    "enum": ["disposable", "package_registry", "all"]
                                }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "mso_diagnose",
                        "description": "Scans for data drift, unmounted external SSD symlinks (GhostLocal), or path conflicts where both local and external copies exist, returning repair recommendations.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "mso_offload_target",
                        "description": "Previews or executes relocation of a developer cache target to the connected external APFS SSD. ALWAYS call with execute: false (default) first to preview what will happen. Only set execute: true in a follow-up call after confirming the preview with the user. Transfer duration scales with target size (e.g. ~30s for 5 GB, ~5min for 50 GB over USB-C).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "target_key": {
                                    "type": "string",
                                    "description": "Target key to offload (e.g. 'derived-data', 'gradle', 'xcode-archives', 'coresimulator', 'ios-device-support', 'pub-cache', 'npm', 'cargo', 'cocoapods', 'maven')"
                                },
                                "execute": {
                                    "type": "boolean",
                                    "description": "Set to true to actually perform the transfer and delete the local directory. Defaults to false (dry-run preview only). Always call with false first so the user can review what will happen before committing.",
                                    "default": false
                                },
                                "drive_path": {
                                    "type": "string",
                                    "description": "Optional path to external APFS SSD volume (e.g. '/Volumes/ExtremeSSD'). If omitted, uses saved drive from config."
                                },
                                "conflict_strategy": {
                                    "type": "string",
                                    "description": "Resolution strategy if data exists in both places: 'merge' (default), 'overwrite_external', or 'discard_local'",
                                    "enum": ["merge", "overwrite_external", "discard_local"]
                                }
                            },
                            "required": ["target_key"]
                        }
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
                "mso_status" => handle_tool_status(req_id),
                "mso_list_targets" => handle_tool_list_targets(req_id, &arguments),
                "mso_diagnose" => handle_tool_diagnose(req_id),
                "mso_offload_target" => handle_tool_offload_target(req_id, &arguments, progress_token),
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
                PathState::Conflict { .. } | PathState::RebindDrive { .. } => {
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
            if matches!(info.state, PathState::GhostLocal { .. } | PathState::Conflict { .. }) {
                issues.push(json!({
                    "key": target.key(),
                    "name": target.display_name(),
                    "state": format!("{:?}", info.state),
                    "local_path": info.local_path.to_string_lossy(),
                    "external_path": info.external_path.to_string_lossy(),
                    "size_bytes": info.size_bytes,
                    "size_human": format_bytes(info.size_bytes),
                    "description": match info.state {
                        PathState::GhostLocal { .. } => "External SSD is unattached or symlink is broken.",
                        PathState::Conflict { .. } => "Local directory exists AND external SSD backup exists.",
                        _ => ""
                    },
                    "available_strategies": ["merge", "overwrite_external", "discard_local", "rollback_to_local"]
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
            "warning": format!(
                "This will permanently remove '{}' ({}) from your local Mac disk after copying to the external SSD. Show this preview to the user and confirm before proceeding. Re-call with execute: true to commit.",
                info.local_path.to_string_lossy(),
                format_bytes(size_bytes)
            )
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

    // SERVER-ENFORCED GATE: Reject execute: true if no preceding dry-run preview call occurred
    {
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
            &format!("Initializing offload for {}...", target.display_name()),
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
            if let Some(token) = &progress_token {
                emit_mcp_progress(
                    token,
                    size_bytes,
                    size_bytes,
                    &format!("Finished offloading {}!", target.display_name()),
                );
            }

            let res = json!({
                "success": true,
                "target_key": target.key(),
                "target_name": target.display_name(),
                "bytes_moved": size_bytes,
                "bytes_moved_human": format_bytes(size_bytes),
                "local_path": info.local_path.to_string_lossy(),
                "external_path": info.external_path.to_string_lossy(),
                "message": format!("Successfully offloaded {} to external APFS SSD and established symlink.", target.display_name())
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
                    "drive_path": "/Volumes/MacData"
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
                    "drive_path": "/Volumes/MacData"
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
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mso_status"));
        assert!(names.contains(&"mso_list_targets"));
        assert!(names.contains(&"mso_diagnose"));
        assert!(names.contains(&"mso_offload_target"));
    }
}
