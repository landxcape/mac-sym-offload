use crate::models::{ConflictStrategy, PathState, TargetInfo};
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
    if dry_run {
        println!(
            "[DRY-RUN] Would migrate {} from {:?} to {:?}",
            info.target.display_name(),
            info.local_path,
            info.external_path
        );
        return Ok(());
    }

    match &info.state {
        PathState::AlreadyLinked { .. } => {
            if verbose {
                println!("Skipping {}: Already linked.", info.target.display_name());
            }
            Ok(())
        }
        PathState::StaleSymlink { .. } => execute_relink_stale_symlink(info, verbose),
        PathState::Fresh => execute_fresh_migration(info, verbose),
        PathState::RebindDrive { old_target_path } => {
            execute_ssd_to_ssd_transfer(info, old_target_path, verbose)
        }
        PathState::GhostLocal { .. } => execute_ghost_repair(info, verbose),
        PathState::Conflict { .. } => {
            let strat = conflict_strategy.unwrap_or(ConflictStrategy::Merge);
            execute_conflict_migration(info, strat, verbose)
        }
        PathState::ExistingExternalData { .. } => execute_reconnect_symlink(info, verbose),
        PathState::NotFound => {
            if verbose {
                println!(
                    "Skipping {}: Local path does not exist.",
                    info.target.display_name()
                );
            }
            Ok(())
        }
    }
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
