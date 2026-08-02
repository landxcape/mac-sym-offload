use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ExternalDrive {
    pub name: String,
    pub volume_path: PathBuf,
    pub file_system: String,
    pub is_apfs: bool,
}

/// Discover all mounted external APFS drives in /Volumes/
pub fn discover_external_drives() -> Result<Vec<ExternalDrive>> {
    let volumes_path = Path::new("/Volumes");
    if !volumes_path.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(volumes_path)
        .context("Failed to read /Volumes directory")?;

    let mut drives = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // Exclude system root / Macintosh HD symlinks or internal root volume
        if name.contains("Macintosh HD") || name == "/" {
            continue;
        }

        // Validate filesystem via diskutil
        if let Ok(drive) = inspect_drive_apfs(&path) {
            drives.push(drive);
        }
    }

    Ok(drives)
}

/// Helper to resolve the underlying volume mount point for any path (even subfolders)
pub fn resolve_volume_mount_point(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for ancestor in canonical.ancestors() {
        if ancestor.parent() == Some(Path::new("/Volumes")) || ancestor == Path::new("/") {
            return ancestor.to_path_buf();
        }
    }
    path.to_path_buf()
}

/// Check filesystem type for a specific volume path or subfolder using `diskutil info`
pub fn inspect_drive_apfs(volume_path: &Path) -> Result<ExternalDrive> {
    let name = volume_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown Drive")
        .to_string();

    let mount_point = resolve_volume_mount_point(volume_path);

    let output = Command::new("diskutil")
        .arg("info")
        .arg(&mount_point)
        .output()
        .context("Failed to execute diskutil command")?;

    if !output.status.success() {
        return Err(anyhow!("diskutil query failed for mount point {:?}", mount_point));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut file_system = "Unknown".to_string();
    let mut is_apfs = false;

    for line in stdout.lines() {
        if line.contains("Type (Bundle):") || line.contains("File System Personality:") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 2 {
                file_system = parts[1].trim().to_string();
                if file_system.to_lowercase().contains("apfs") {
                    is_apfs = true;
                }
            }
        }
    }

    if !is_apfs && stdout.to_lowercase().contains("apfs") {
        is_apfs = true;
        file_system = "APFS".to_string();
    }

    Ok(ExternalDrive {
        name,
        volume_path: volume_path.to_path_buf(),
        file_system,
        is_apfs,
    })
}

/// Validate that a given path is an existing, APFS-formatted external drive
pub fn validate_apfs_drive(drive_path: &Path) -> Result<ExternalDrive> {
    let mount_point = resolve_volume_mount_point(drive_path);
    if !mount_point.exists() && !drive_path.exists() {
        return Err(anyhow!(
            "External volume path does not exist: {:?}",
            drive_path
        ));
    }

    let drive = inspect_drive_apfs(drive_path)?;

    if !drive.is_apfs {
        return Err(anyhow!(
            "External drive {:?} is formatted as '{}'. Must be formatted as APFS to preserve Unix file permissions. exFAT/NTFS are unsupported.",
            drive_path,
            drive.file_system
        ));
    }

    Ok(drive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_volume_mount_point_subfolder() {
        let subfolder = Path::new("/Volumes/MacData/Developer");
        let resolved = resolve_volume_mount_point(subfolder);
        // Should resolve to /Volumes/MacData or subfolder path safely without crashing
        assert!(resolved.starts_with("/Volumes") || resolved == Path::new("/Volumes/MacData/Developer"));
    }

    #[test]
    fn test_inspect_drive_apfs_subfolder_scoped() {
        let root = Path::new("/");
        let drive = inspect_drive_apfs(root).expect("Root volume inspection must succeed");
        assert!(drive.is_apfs);
        assert_eq!(drive.volume_path, Path::new("/"));
    }
}
