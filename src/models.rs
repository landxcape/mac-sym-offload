use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CacheTarget {
    DerivedData,
    CoreSimulator,
    XcodeArchives,
    IosDeviceSupport,
    Gradle,
    Android,
    PubCache,
    CocoaPods,
    Npm,
    Cargo,
    Maven,
    Custom { name: String, local_rel_path: PathBuf },
}

impl CacheTarget {
    pub fn all() -> Vec<CacheTarget> {
        vec![
            CacheTarget::DerivedData,
            CacheTarget::CoreSimulator,
            CacheTarget::XcodeArchives,
            CacheTarget::IosDeviceSupport,
            CacheTarget::Gradle,
            CacheTarget::Android,
            CacheTarget::PubCache,
            CacheTarget::CocoaPods,
            CacheTarget::Npm,
            CacheTarget::Cargo,
            CacheTarget::Maven,
        ]
    }

    pub fn key(&self) -> String {
        match self {
            CacheTarget::DerivedData => "derived-data".to_string(),
            CacheTarget::CoreSimulator => "coresimulator".to_string(),
            CacheTarget::XcodeArchives => "xcode-archives".to_string(),
            CacheTarget::IosDeviceSupport => "ios-device-support".to_string(),
            CacheTarget::Gradle => "gradle".to_string(),
            CacheTarget::Android => "android".to_string(),
            CacheTarget::PubCache => "pub-cache".to_string(),
            CacheTarget::CocoaPods => "cocoapods".to_string(),
            CacheTarget::Npm => "npm".to_string(),
            CacheTarget::Cargo => "cargo".to_string(),
            CacheTarget::Maven => "maven".to_string(),
            CacheTarget::Custom { name, .. } => format!("custom-{}", name.to_lowercase().replace(' ', "-")),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            CacheTarget::DerivedData => "Xcode DerivedData".to_string(),
            CacheTarget::CoreSimulator => "iOS CoreSimulator".to_string(),
            CacheTarget::XcodeArchives => "Xcode Archives (.xcarchive)".to_string(),
            CacheTarget::IosDeviceSupport => "iOS Device Support Symbols".to_string(),
            CacheTarget::Gradle => "Android Gradle Cache".to_string(),
            CacheTarget::Android => "Android Emulator & SDK (.android)".to_string(),
            CacheTarget::PubCache => "Flutter Pub Cache (.pub-cache)".to_string(),
            CacheTarget::CocoaPods => "CocoaPods Cache (.cocoapods)".to_string(),
            CacheTarget::Npm => "NPM Package Cache (.npm)".to_string(),
            CacheTarget::Cargo => "Rust Cargo Registry (.cargo/registry)".to_string(),
            CacheTarget::Maven => "Maven Local Repository (.m2/repository)".to_string(),
            CacheTarget::Custom { name, .. } => format!("Custom Path ({})", name),
        }
    }

    pub fn is_build_output(&self) -> bool {
        match self {
            CacheTarget::DerivedData
            | CacheTarget::CoreSimulator
            | CacheTarget::XcodeArchives
            | CacheTarget::IosDeviceSupport
            | CacheTarget::Gradle
            | CacheTarget::Android => true,
            CacheTarget::PubCache
            | CacheTarget::CocoaPods
            | CacheTarget::Npm
            | CacheTarget::Cargo
            | CacheTarget::Maven
            | CacheTarget::Custom { .. } => false,
        }
    }

    pub fn default_relative_path(&self) -> PathBuf {
        match self {
            CacheTarget::DerivedData => PathBuf::from("Developer/Xcode/DerivedData"),
            CacheTarget::CoreSimulator => PathBuf::from("Developer/CoreSimulator"),
            CacheTarget::XcodeArchives => PathBuf::from("Developer/Xcode/Archives"),
            CacheTarget::IosDeviceSupport => PathBuf::from("Developer/Xcode/iOS DeviceSupport"),
            CacheTarget::Gradle => PathBuf::from(".gradle"),
            CacheTarget::Android => PathBuf::from(".android"),
            CacheTarget::PubCache => PathBuf::from(".pub-cache"),
            CacheTarget::CocoaPods => PathBuf::from(".cocoapods"),
            CacheTarget::Npm => PathBuf::from(".npm"),
            CacheTarget::Cargo => PathBuf::from(".cargo/registry"),
            CacheTarget::Maven => PathBuf::from(".m2/repository"),
            CacheTarget::Custom { local_rel_path, .. } => local_rel_path.clone(),
        }
    }

    pub fn default_local_path(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        Some(match self {
            CacheTarget::DerivedData => home.join("Library/Developer/Xcode/DerivedData"),
            CacheTarget::CoreSimulator => home.join("Library/Developer/CoreSimulator"),
            CacheTarget::XcodeArchives => home.join("Library/Developer/Xcode/Archives"),
            CacheTarget::IosDeviceSupport => home.join("Library/Developer/Xcode/iOS DeviceSupport"),
            CacheTarget::Gradle => home.join(".gradle"),
            CacheTarget::Android => home.join(".android"),
            CacheTarget::PubCache => home.join(".pub-cache"),
            CacheTarget::CocoaPods => home.join(".cocoapods"),
            CacheTarget::Npm => home.join(".npm"),
            CacheTarget::Cargo => home.join(".cargo/registry"),
            CacheTarget::Maven => home.join(".m2/repository"),
            CacheTarget::Custom { local_rel_path, .. } => {
                if local_rel_path.is_absolute() {
                    local_rel_path.clone()
                } else {
                    home.join(local_rel_path)
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathState {
    /// Local directory exists, no external directory yet
    Fresh,
    /// Local directory is a valid symlink pointing to external SSD
    AlreadyLinked { target_path: PathBuf },
    /// Symlink points to an outdated path template or previous location
    StaleSymlink { current_target: PathBuf, expected_target: PathBuf },
    /// Local directory is currently symlinked to a different external SSD (Drive Transfer mode)
    #[allow(dead_code)]
    RebindDrive { old_target_path: PathBuf },
    /// Broken symlink or dummy local folder regenerated over missing SSD mount
    GhostLocal { symlink_target: PathBuf },
    /// Local directory exists AND external directory exists
    Conflict { external_path: PathBuf },
    /// Existing offloaded folder discovered on external drive (even if config missing)
    ExistingExternalData { external_path: PathBuf },
    /// Local path does not exist at all
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    OverwriteExternal,
    Merge,
    DiscardLocal,
    KeepLocalDiscardExternal,
    RollbackExternalToLocal,
    Relink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub target: CacheTarget,
    pub local_path: PathBuf,
    pub external_path: PathBuf,
    pub state: PathState,
    pub size_bytes: u64,
}
