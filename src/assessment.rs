use crate::models::{CacheTarget, PathState, TargetInfo};
use anyhow::Result;
use indicatif::ProgressBar;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn assess_target(target: &CacheTarget, external_drive_root: &Path) -> Result<TargetInfo> {
    let local_path = match target.default_local_path() {
        Some(p) => p,
        None => return Err(anyhow::anyhow!("Could not resolve local path")),
    };

    let relative_path = target.default_relative_path();
    let external_path = external_drive_root.join(relative_path);

    let state = determine_path_state(&local_path, &external_path);
    let size_bytes = calculate_target_size(&local_path, &external_path, &state);

    Ok(TargetInfo {
        target: target.clone(),
        local_path,
        external_path,
        state,
        size_bytes,
    })
}

#[allow(dead_code)]
pub fn assess_target_with_spinner(
    target: &CacheTarget,
    external_drive_root: &Path,
    pb: &ProgressBar,
) -> Result<TargetInfo> {
    pb.set_message(format!("Scanning {}...", target.display_name()));
    assess_target(target, external_drive_root)
}

fn determine_path_state(local_path: &Path, external_path: &Path) -> PathState {
    let local_exists = local_path.exists() || is_symlink(local_path);
    let external_exists = external_path.exists();

    if is_symlink(local_path) {
        if let Ok(symlink_target) = fs::read_link(local_path) {
            if symlink_target == external_path {
                return PathState::AlreadyLinked {
                    target_path: external_path.to_path_buf(),
                };
            } else if symlink_target.exists() && external_exists {
                return PathState::RebindDrive {
                    old_target_path: symlink_target,
                };
            } else {
                return PathState::GhostLocal { symlink_target };
            }
        }
    }

    if local_exists && external_exists {
        PathState::Conflict {
            external_path: external_path.to_path_buf(),
        }
    } else if local_exists && !external_exists {
        PathState::Fresh
    } else if !local_exists && external_exists {
        PathState::ExistingExternalData {
            external_path: external_path.to_path_buf(),
        }
    } else {
        PathState::NotFound
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn calculate_target_size(local_path: &Path, external_path: &Path, state: &PathState) -> u64 {
    match state {
        PathState::Fresh | PathState::GhostLocal { .. } | PathState::Conflict { .. } => {
            get_fast_dir_size_bytes(local_path).unwrap_or(0)
        }
        PathState::AlreadyLinked { .. } | PathState::ExistingExternalData { .. } => {
            get_fast_dir_size_bytes(external_path).unwrap_or(0)
        }
        PathState::RebindDrive { old_target_path } => {
            get_fast_dir_size_bytes(old_target_path).unwrap_or(0)
        }
        PathState::NotFound => 0,
    }
}

pub fn get_fast_dir_size_bytes(path: &Path) -> Option<u64> {
    if !path.exists() {
        return Some(0);
    }

    let output = Command::new("du").arg("-sk").arg(path).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_word = stdout.split_whitespace().next()?;
    let kb: u64 = first_word.parse().ok()?;
    Some(kb * 1024)
}
