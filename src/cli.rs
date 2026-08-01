use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mso",
    author = "MacSymOffload Contributors",
    version,
    about = "Safely offload bloated macOS developer caches to an external APFS SSD using symlinks",
    long_about = None
)]
pub struct Cli {
    /// Enable verbose logging output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Scan and display local cache directory sizes and symlink states
    Scan {
        /// Output summary in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Migrate selected cache targets to external SSD
    Migrate {
        /// Target cache keys to migrate (e.g. derived-data, gradle, pub-cache, or all)
        #[arg(short, long, value_delimiter = ',')]
        targets: Vec<String>,

        /// Path to external APFS drive volume (e.g., /Volumes/ExtremeSSD)
        #[arg(short, long)]
        drive: Option<PathBuf>,

        /// Simulate migration without modifying files
        #[arg(long)]
        dry_run: bool,

        /// Auto-confirm prompts without asking in non-interactive mode
        #[arg(short = 'y', long)]
        yes: bool,

        /// Resolution strategy for conflicts (overwrite-external, merge, discard-local)
        #[arg(long, value_enum)]
        conflict_strategy: Option<CliConflictStrategy>,
    },
    /// Restore linked cache targets from external SSD back to local Mac storage
    Restore {
        /// Target cache keys to restore back to local storage (e.g. derived-data, gradle, or all)
        #[arg(short, long, value_delimiter = ',')]
        targets: Vec<String>,

        /// Path to external APFS drive volume
        #[arg(short, long)]
        drive: Option<PathBuf>,

        /// Keep a backup copy on external SSD after restoring to local
        #[arg(long)]
        keep_external: bool,

        /// Simulate restore without moving files or deleting symlinks
        #[arg(long)]
        dry_run: bool,

        /// Auto-confirm prompts without asking
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Repair broken symlinks or ghost directories after SSD reconnection
    Repair {
        /// Path to external APFS drive volume
        #[arg(short, long)]
        drive: Option<PathBuf>,

        /// Resolution strategy for ghost local directories
        #[arg(long, value_enum)]
        conflict_strategy: Option<CliConflictStrategy>,
    },
    /// Show current symlink routing status for all supported cache targets
    Status,
    /// View or edit saved CLI configuration (~/.config/mso/config.json)
    Config {
        /// Reset configuration file to default
        #[arg(long)]
        reset: bool,
    },
    /// Start Model Context Protocol (MCP) server over stdio for AI integration
    Mcp,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliConflictStrategy {
    OverwriteExternal,
    Merge,
    DiscardLocal,
    KeepLocalDiscardExternal,
    RollbackExternalToLocal,
}
