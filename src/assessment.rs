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
    let mut external_path = external_drive_root.join(relative_path);

    let state = determine_path_state(&local_path, &external_path);

    // Ensure external_path and state.target_path always agree for AlreadyLinked targets
    if let PathState::AlreadyLinked { ref target_path } = state {
        external_path = target_path.clone();
    }

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
                if external_path.exists() {
                    return PathState::AlreadyLinked {
                        target_path: external_path.to_path_buf(),
                    };
                } else {
                    return PathState::GhostLocal { symlink_target };
                }
            } else {
                return PathState::StaleSymlink {
                    current_target: symlink_target,
                    expected_target: external_path.to_path_buf(),
                };
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

fn calculate_target_size(local_path: &Path, _external_path: &Path, state: &PathState) -> u64 {
    match state {
        PathState::Fresh | PathState::GhostLocal { .. } | PathState::Conflict { .. } => {
            get_fast_dir_size_bytes(local_path).unwrap_or(0)
        }
        PathState::AlreadyLinked { target_path } => {
            get_fast_dir_size_bytes(target_path).unwrap_or(0)
        }
        PathState::StaleSymlink { current_target, expected_target } => {
            if expected_target.exists() {
                get_fast_dir_size_bytes(expected_target).unwrap_or(0)
            } else {
                get_fast_dir_size_bytes(current_target).unwrap_or(0)
            }
        }
        PathState::ExistingExternalData { external_path: p } => {
            get_fast_dir_size_bytes(p).unwrap_or(0)
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

    if let Ok(output) = Command::new("du").arg("-sk").arg(path).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_word) = stdout.split_whitespace().next() {
                if let Ok(kb) = first_word.parse::<u64>() {
                    if kb > 0 {
                        return Some(kb * 1024);
                    }
                }
            }
        }
    }

    // Fallback for small directories (< 1KB) or du failures: metadata walk
    let mut total_bytes: u64 = 0;
    if path.is_file() {
        return path.metadata().ok().map(|m| m.len());
    }
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total_bytes += meta.len();
                } else if meta.is_dir() {
                    total_bytes += get_fast_dir_size_bytes(&entry.path()).unwrap_or(0);
                }
            }
        }
    }
    Some(total_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_xcode_targets_path_convention_consistency() {
        let subfolder_drive = Path::new("/Volumes/MacData/Developer");

        let derived = assess_target(&CacheTarget::DerivedData, subfolder_drive).unwrap();
        let archives = assess_target(&CacheTarget::XcodeArchives, subfolder_drive).unwrap();
        let ios_dev = assess_target(&CacheTarget::IosDeviceSupport, subfolder_drive).unwrap();

        // All three Xcode targets must use the exact same nested path structure!
        assert_eq!(
            derived.external_path,
            PathBuf::from("/Volumes/MacData/Developer/Developer/Xcode/DerivedData")
        );
        assert_eq!(
            archives.external_path,
            PathBuf::from("/Volumes/MacData/Developer/Developer/Xcode/Archives")
        );
        assert_eq!(
            ios_dev.external_path,
            PathBuf::from("/Volumes/MacData/Developer/Developer/Xcode/iOS DeviceSupport")
        );
    }

    #[test]
    fn test_already_linked_external_path_and_target_path_agreement() {
        let state = PathState::AlreadyLinked {
            target_path: PathBuf::from("/Volumes/MacData/Developer/Developer/Xcode/DerivedData"),
        };
        let external_path = Path::new("/Volumes/MacData/Developer/Developer/Xcode/DerivedData");

        let size = calculate_target_size(Path::new("/tmp"), external_path, &state);
        // Size calculation for AlreadyLinked targets must measure target_path (which has ~4.87 GiB on disk)
        assert!(size > 0);
    }

    #[test]
    fn test_stale_symlink_detection() {
        let unique_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base_dir = std::env::temp_dir().join(format!("mso_test_{}", unique_id));
        let _ = std::fs::create_dir_all(&base_dir);

        let local_link = base_dir.join("local_symlink");
        let old_target = base_dir.join("old_external_path");
        let new_target = base_dir.join("new_external_path");

        std::os::unix::fs::symlink(&old_target, &local_link).unwrap();

        let state = determine_path_state(&local_link, &new_target);
        let _ = std::fs::remove_dir_all(&base_dir);

        assert!(matches!(state, PathState::StaleSymlink { .. }));

        if let PathState::StaleSymlink { current_target, expected_target } = state {
            assert_eq!(current_target, old_target);
            assert_eq!(expected_target, new_target);
        }
    }
}
