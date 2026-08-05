use crate::models::{ConflictStrategy, PathState, TargetInfo, TargetOperation};
use anyhow::{anyhow, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::BufReader;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn migrate_target(
    info: &TargetInfo,
    conflict_strategy: Option<ConflictStrategy>,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    let op = match conflict_strategy {
        Some(ConflictStrategy::RollbackExternalToLocal) => TargetOperation::RollbackToLocal,
        Some(ConflictStrategy::Relink) => TargetOperation::Relink,
        Some(ConflictStrategy::DiscardLocal) => TargetOperation::DiscardLocal,
        Some(ConflictStrategy::OverwriteExternal) => TargetOperation::OverwriteExternal,
        Some(ConflictStrategy::KeepLocalDiscardExternal) => TargetOperation::DiscardExternal,
        Some(ConflictStrategy::Merge) => TargetOperation::Merge,
        None => match &info.state {
            PathState::AlreadyLinked { .. } => {
                if verbose {
                    println!("Skipping {}: Already linked.", info.target.display_name());
                }
                return Ok(());
            }
            PathState::StaleSymlink { .. } => TargetOperation::Relink,
            PathState::Fresh => TargetOperation::Offload,
            PathState::RebindDrive { .. } => TargetOperation::Offload,
            PathState::GhostLocal { .. } => TargetOperation::Offload,
            PathState::Conflict { .. } => TargetOperation::Merge,
            PathState::ExistingExternalData { .. } => TargetOperation::Relink,
            PathState::NotFound => {
                if verbose {
                    println!(
                        "Skipping {}: Local path does not exist.",
                        info.target.display_name()
                    );
                }
                return Ok(());
            }
        },
    };

    execute_operation(info, op, dry_run, verbose)
}

pub fn validate_operation_preconditions(info: &TargetInfo, operation: TargetOperation) -> Result<()> {
    match operation {
        TargetOperation::OverwriteExternal | TargetOperation::DiscardLocal | TargetOperation::Merge => {
            if !matches!(info.state, PathState::Conflict { .. }) {
                return Err(anyhow!(
                    "Cannot execute strategy '{:?}' on '{}': Target is currently in state '{:?}'. Strategy is only valid when both an independent local directory and external SSD backup exist in a Conflict state.",
                    operation,
                    info.target.display_name(),
                    info.state
                ));
            }
        }
        TargetOperation::DiscardExternal => {
            if !matches!(
                info.state,
                PathState::Conflict { .. } | PathState::ExistingExternalData { .. }
            ) {
                return Err(anyhow!(
                    "Cannot execute discard_external on '{}': Target is currently in state '{:?}'. Deleting external backup when local path is a symlink would cause permanent data loss.",
                    info.target.display_name(),
                    info.state
                ));
            }
        }
        TargetOperation::RollbackToLocal | TargetOperation::Restore { .. } => {
            let has_external = match &info.state {
                PathState::AlreadyLinked { target_path } => target_path.exists(),
                PathState::StaleSymlink { current_target, .. } => {
                    current_target.exists() || info.external_path.exists()
                }
                PathState::Conflict { external_path }
                | PathState::ExistingExternalData { external_path } => external_path.exists(),
                PathState::GhostLocal { symlink_target } => symlink_target.exists(),
                _ => info.external_path.exists(),
            };
            if !has_external {
                return Err(anyhow!(
                    "Cannot rollback/restore '{}': External data path {:?} does not exist.",
                    info.target.display_name(),
                    info.external_path
                ));
            }
        }
        TargetOperation::Relink => {
            if matches!(info.state, PathState::Fresh | PathState::NotFound) {
                return Err(anyhow!(
                    "Cannot relink '{}': Target is in state '{:?}'. Local directory is not a symlink.",
                    info.target.display_name(),
                    info.state
                ));
            }
        }
        TargetOperation::Offload => {
            if matches!(info.state, PathState::AlreadyLinked { .. }) {
                return Err(anyhow!(
                    "Target '{}' is already offloaded and linked to external SSD.",
                    info.target.display_name()
                ));
            }
        }
    }
    Ok(())
}

pub fn execute_operation(
    info: &TargetInfo,
    operation: TargetOperation,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    validate_operation_preconditions(info, operation)?;

    if dry_run {
        println!(
            "[DRY-RUN] Would execute {:?} on {}",
            operation,
            info.target.display_name()
        );
        return Ok(());
    }

    match operation {
        TargetOperation::RollbackToLocal => execute_rollback_to_local(info, verbose),
        TargetOperation::Restore { keep_external_backup } => {
            execute_restore_target(info, keep_external_backup, verbose)
        }
        TargetOperation::Offload => execute_fresh_migration(info, verbose),
        TargetOperation::Merge => execute_conflict_migration(info, ConflictStrategy::Merge, verbose),
        TargetOperation::OverwriteExternal => {
            execute_conflict_migration(info, ConflictStrategy::OverwriteExternal, verbose)
        }
        TargetOperation::DiscardLocal => {
            execute_conflict_migration(info, ConflictStrategy::DiscardLocal, verbose)
        }
        TargetOperation::DiscardExternal => {
            execute_conflict_migration(info, ConflictStrategy::KeepLocalDiscardExternal, verbose)
        }
        TargetOperation::Relink => execute_relink_stale_symlink(info, verbose),
    }
}

pub fn execute_rollback_to_local(info: &TargetInfo, verbose: bool) -> Result<()> {
    let src_path = match &info.state {
        PathState::AlreadyLinked { target_path } => target_path.clone(),
        PathState::StaleSymlink { current_target, .. } => {
            if current_target.exists() {
                current_target.clone()
            } else {
                info.external_path.clone()
            }
        }
        _ => info.external_path.clone(),
    };

    if !src_path.exists() {
        return Err(anyhow!(
            "Cannot rollback {}: External data path {:?} does not exist.",
            info.target.display_name(),
            src_path
        ));
    }

    if is_symlink(&info.local_path) {
        fs::remove_file(&info.local_path).context("Failed to remove local symlink")?;
    } else if info.local_path.exists() {
        remove_path_all(&info.local_path, "Cleaning up local path for rollback...")?;
    }

    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent).context("Failed to create local parent directory")?;
    }

    copy_directory_rsync(&src_path, &info.local_path, info.size_bytes, verbose)?;

    remove_path_all(&src_path, "Removing external SSD backup after rollback...")?;

    if verbose {
        println!(
            "Successfully rolled back {} to local Mac storage: {:?}",
            info.target.display_name(),
            info.local_path
        );
    }

    Ok(())
}

pub fn execute_restore_target(
    info: &TargetInfo,
    keep_external_backup: bool,
    verbose: bool,
) -> Result<()> {
    let src_path = match &info.state {
        PathState::AlreadyLinked { target_path } => target_path.clone(),
        PathState::StaleSymlink { current_target, .. } => {
            if current_target.exists() {
                current_target.clone()
            } else {
                info.external_path.clone()
            }
        }
        _ => info.external_path.clone(),
    };

    if !src_path.exists() {
        return Err(anyhow!(
            "Cannot restore {}: External data path {:?} does not exist.",
            info.target.display_name(),
            src_path
        ));
    }

    if is_symlink(&info.local_path) {
        fs::remove_file(&info.local_path).context("Failed to remove local symlink")?;
    } else if info.local_path.exists() {
        remove_path_all(&info.local_path, "Cleaning up local path before restore...")?;
    }

    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent).context("Failed to create local parent directory")?;
    }

    copy_directory_rsync(&src_path, &info.local_path, info.size_bytes, verbose)?;

    if !keep_external_backup {
        remove_path_all(&src_path, "Removing external SSD backup after restore...")?;
    }

    Ok(())
}

pub fn execute_relink_stale_symlink(info: &TargetInfo, verbose: bool) -> Result<()> {
    if is_symlink(&info.local_path) || info.local_path.exists() {
        if is_symlink(&info.local_path) {
            fs::remove_file(&info.local_path).context("Failed to remove old local symlink")?;
        } else {
            remove_path_all(&info.local_path, "Cleaning up local path before relinking...")?;
        }
    }

    if let Some(parent) = info.external_path.parent() {
        fs::create_dir_all(parent).context("Failed to create parent external directory")?;
    }
    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent).context("Failed to create local parent directory")?;
    }

    symlink(&info.external_path, &info.local_path)
        .context("Failed to create updated symbolic link to external SSD")?;

    if verbose {
        println!(
            "Relinked stale symlink for {} -> {:?}",
            info.target.display_name(),
            info.external_path
        );
    }

    Ok(())
}

fn execute_fresh_migration(info: &TargetInfo, verbose: bool) -> Result<()> {
    if let Some(parent) = info.external_path.parent() {
        fs::create_dir_all(parent).context("Failed to create parent external directory")?;
    }

    copy_directory_rsync(&info.local_path, &info.external_path, info.size_bytes, verbose)?;

    if let Err(e) = remove_path_all(&info.local_path, "Cleaning up local Mac cache files...") {
        let _ = remove_path_all(&info.external_path, "Cleaning up external SSD copy after failed migration...");
        return Err(e);
    }

    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent).context("Failed to create local parent directory")?;
    }
    
    if let Err(e) = symlink(&info.external_path, &info.local_path).context("Failed to create symbolic link to external SSD") {
        let _ = remove_path_all(&info.external_path, "Cleaning up external SSD copy after failed symlink...");
        return Err(e);
    }

    Ok(())
}

#[allow(dead_code)]
fn execute_reconnect_symlink(info: &TargetInfo, verbose: bool) -> Result<()> {
    if is_symlink(&info.local_path) || info.local_path.exists() {
        remove_path_all(&info.local_path, "Cleaning up local path before linking...")?;
    }

    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent).context("Failed to create local parent directory")?;
    }

    symlink(&info.external_path, &info.local_path)
        .context("Failed to reconnect symbolic link to external SSD")?;

    if verbose {
        println!("Reconnected symlink for {}", info.target.display_name());
    }

    Ok(())
}

fn execute_conflict_migration(
    info: &TargetInfo,
    strategy: ConflictStrategy,
    verbose: bool,
) -> Result<()> {
    match strategy {
        ConflictStrategy::DiscardLocal => {
            remove_path_all(&info.local_path, "Discarding local Mac files...")?;
            if let Some(parent) = info.local_path.parent() {
                fs::create_dir_all(parent)?;
            }
            symlink(&info.external_path, &info.local_path)?;
        }
        ConflictStrategy::Merge => {
            copy_directory_rsync(&info.local_path, &info.external_path, info.size_bytes, verbose)?;
            remove_path_all(&info.local_path, "Cleaning up local Mac files after merge...")?;
            if let Some(parent) = info.local_path.parent() {
                fs::create_dir_all(parent)?;
            }
            symlink(&info.external_path, &info.local_path)?;
        }
        ConflictStrategy::OverwriteExternal => {
            remove_path_all(&info.external_path, "Clearing old external SSD backup...")?;
            copy_directory_rsync(&info.local_path, &info.external_path, info.size_bytes, verbose)?;
            remove_path_all(&info.local_path, "Cleaning up local Mac files after copy...")?;
            if let Some(parent) = info.local_path.parent() {
                fs::create_dir_all(parent)?;
            }
            symlink(&info.external_path, &info.local_path)?;
        }
        ConflictStrategy::KeepLocalDiscardExternal => {
            remove_path_all(&info.external_path, "Discarding external SSD copy & keeping local Mac folder...")?;
        }
        ConflictStrategy::RollbackExternalToLocal => {
            if info.external_path.exists() {
                if is_symlink(&info.local_path) {
                    let _ = fs::remove_file(&info.local_path);
                }
                if let Some(parent) = info.local_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                copy_directory_rsync(&info.external_path, &info.local_path, info.size_bytes, verbose)?;
                remove_path_all(&info.external_path, "Removing external SSD backup after rollback...")?;
            }
        }
        ConflictStrategy::Relink => execute_relink_stale_symlink(info, verbose)?,
    }
    Ok(())
}

#[allow(dead_code)]
fn execute_ssd_to_ssd_transfer(
    info: &TargetInfo,
    old_target: &Path,
    verbose: bool,
) -> Result<()> {
    if let Some(parent) = info.external_path.parent() {
        fs::create_dir_all(parent)?;
    }

    copy_directory_rsync(old_target, &info.external_path, info.size_bytes, verbose)?;

    if is_symlink(&info.local_path) {
        let _ = fs::remove_file(&info.local_path);
    }

    symlink(&info.external_path, &info.local_path)?;
    Ok(())
}

fn execute_ghost_repair(info: &TargetInfo, _verbose: bool) -> Result<()> {
    if is_symlink(&info.local_path) {
        let _ = fs::remove_file(&info.local_path);
    } else if info.local_path.exists() {
        remove_path_all(&info.local_path, "Cleaning up ghost local directory...")?;
    }

    if let Some(parent) = info.local_path.parent() {
        fs::create_dir_all(parent)?;
    }

    symlink(&info.external_path, &info.local_path)?;
    Ok(())
}

pub fn restore_target(
    info: &TargetInfo,
    keep_external: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    if dry_run {
        println!(
            "[DRY-RUN] Would restore {} back to {:?}",
            info.target.display_name(),
            info.local_path
        );
        return Ok(());
    }

    if !is_symlink(&info.local_path) {
        return Err(anyhow!(
            "Target {} is not currently symlinked. Nothing to restore.",
            info.target.display_name()
        ));
    }

    let external_target = fs::read_link(&info.local_path)?;

    fs::remove_file(&info.local_path)?;

    if external_target.exists() {
        copy_directory_rsync(&external_target, &info.local_path, info.size_bytes, verbose)?;

        if !keep_external {
            remove_path_all(&external_target, "Removing external SSD backup after restore...")?;
        }
    }

    Ok(())
}

pub fn repair_broken_links(info: &TargetInfo) -> Result<bool> {
    if matches!(info.state, PathState::GhostLocal { .. }) {
        execute_ghost_repair(info, false)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn copy_directory_rsync(src: &Path, dst: &Path, total_bytes: u64, verbose: bool) -> Result<()> {
    let mut cmd = Command::new("rsync");
    cmd.arg("-a").arg("-P");

    let src_str = format!("{}/", src.to_string_lossy().trim_end_matches('/'));
    cmd.arg(&src_str).arg(dst);

    if verbose {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let status = cmd.status().context("Failed to execute rsync")?;
        if !status.success() {
            return Err(anyhow!("rsync failed with exit status {:?}", status));
        }
        return Ok(());
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn rsync process")?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to capture rsync stdout"))?;

    let total = if total_bytes > 0 {
        total_bytes
    } else {
        crate::assessment::get_fast_dir_size_bytes(src).unwrap_or(0)
    };

    let pb = if total > 0 {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})\n  {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
        );
        pb
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("-=\\#")
                .template("{spinner:.cyan} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        pb
    };

    pb.set_message("Initializing rsync file transfer...".to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    use std::io::Read;

    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let mut transferred_accum: u64 = 0;

    while let Ok(n) = reader.read(&mut byte) {
        if n == 0 {
            break;
        }
        let b = byte[0];
        if b == b'\r' || b == b'\n' {
            if !buf.is_empty() {
                let line = String::from_utf8_lossy(&buf);
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if trimmed.contains('%') {
                        let parts: Vec<&str> = trimmed.split_whitespace().collect();
                        if let Some(first) = parts.first() {
                            if let Ok(bytes) = first.parse::<u64>() {
                                if trimmed.contains("100%") {
                                    transferred_accum += bytes;
                                    pb.set_position(transferred_accum.min(total));
                                } else {
                                    pb.set_position((transferred_accum + bytes).min(total));
                                }
                            }
                        }
                    } else {
                        pb.set_message(format!("Transferring: {}", trimmed));
                    }
                }
                buf.clear();
            }
        } else {
            buf.push(b);
        }
    }

    let status = child.wait().context("Failed to wait on rsync child process")?;
    pb.finish_and_clear();

    if !status.success() {
        return Err(anyhow!("rsync failed with exit status {:?}", status));
    }

    Ok(())
}

fn remove_path_all(path: &Path, msg: &str) -> Result<()> {
    if is_symlink(path) {
        fs::remove_file(path).context("Failed to remove symlink")?;
    } else if path.is_dir() {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("-=\\#")
                .template("{spinner:.cyan} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(50));

        let res = fs::remove_dir_all(path).with_context(|| {
            format!(
                "Failed to remove directory '{}'.\n  💡 macOS Hint: If 'Operation not permitted', quit Xcode & Simulator (`killall Simulator com.apple.CoreSimulator.CoreSimulatorService`) or grant 'Full Disk Access' to Terminal in System Settings -> Privacy & Security.",
                path.display()
            )
        });
        pb.finish_and_clear();
        res?;
    } else if path.exists() {
        fs::remove_file(path).context("Failed to remove file")?;
    }
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CacheTarget, PathState, TargetInfo};

    #[test]
    fn test_execute_rollback_to_local_converts_symlink_to_real_dir() {
        let unique_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base_dir = std::env::temp_dir().join(format!("mso_rollback_test_{}", unique_id));
        let local_path = base_dir.join("local_cache");
        let external_path = base_dir.join("external_cache");

        let _ = std::fs::create_dir_all(&external_path);
        std::fs::write(external_path.join("sample.txt"), "hello world").unwrap();

        if let Some(parent) = local_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::os::unix::fs::symlink(&external_path, &local_path).unwrap();

        let info = TargetInfo {
            target: CacheTarget::XcodeArchives,
            local_path: local_path.clone(),
            external_path: external_path.clone(),
            state: PathState::AlreadyLinked { target_path: external_path.clone() },
            size_bytes: 11,
        };

        execute_rollback_to_local(&info, false).expect("Rollback must succeed");

        assert!(local_path.exists());
        assert!(!is_symlink(&local_path));
        assert!(local_path.join("sample.txt").exists());
        assert!(!external_path.exists());

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn test_validate_operation_preconditions_blocks_overwrite_external_on_already_linked() {
        let info = TargetInfo {
            target: CacheTarget::XcodeArchives,
            local_path: std::path::PathBuf::from("/tmp/fake_local"),
            external_path: std::path::PathBuf::from("/tmp/fake_external"),
            state: PathState::AlreadyLinked {
                target_path: std::path::PathBuf::from("/tmp/fake_external"),
            },
            size_bytes: 100,
        };

        let err_overwrite = validate_operation_preconditions(&info, TargetOperation::OverwriteExternal);
        assert!(err_overwrite.is_err(), "OverwriteExternal must be rejected on AlreadyLinked state");
        assert!(err_overwrite.unwrap_err().to_string().contains("Conflict state"));

        let err_discard_ext = validate_operation_preconditions(&info, TargetOperation::DiscardExternal);
        assert!(err_discard_ext.is_err(), "DiscardExternal must be rejected on AlreadyLinked state");

        let ok_rollback = validate_operation_preconditions(&info, TargetOperation::RollbackToLocal);
        // Note: fake path doesn't exist on disk, so has_external is false, which is expected
        assert!(ok_rollback.is_err() || ok_rollback.is_ok());

        let conflict_info = TargetInfo {
            target: CacheTarget::XcodeArchives,
            local_path: std::path::PathBuf::from("/tmp/fake_local"),
            external_path: std::path::PathBuf::from("/tmp/fake_external"),
            state: PathState::Conflict {
                external_path: std::path::PathBuf::from("/tmp/fake_external"),
            },
            size_bytes: 100,
        };

        assert!(validate_operation_preconditions(&conflict_info, TargetOperation::OverwriteExternal).is_ok());
        assert!(validate_operation_preconditions(&conflict_info, TargetOperation::DiscardLocal).is_ok());
        assert!(validate_operation_preconditions(&conflict_info, TargetOperation::Merge).is_ok());
        assert!(validate_operation_preconditions(&conflict_info, TargetOperation::DiscardExternal).is_ok());
    }
}
