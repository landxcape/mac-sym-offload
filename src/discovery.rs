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

/// Check filesystem type for a specific volume path using `diskutil info`
pub fn inspect_drive_apfs(volume_path: &Path) -> Result<ExternalDrive> {
    let name = volume_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown Drive")
        .to_string();

    let output = Command::new("diskutil")
        .arg("info")
        .arg(volume_path)
        .output()
        .context("Failed to execute diskutil command")?;

    if !output.status.success() {
        return Err(anyhow!("diskutil query failed for path {:?}", volume_path));
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
    if !drive_path.exists() {
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
