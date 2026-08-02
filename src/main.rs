mod assessment;
mod cli;
mod config;
mod discovery;
mod mcp;
mod mcp_setup;
mod migrator;
mod models;
mod ui;

use anyhow::{anyhow, Result};
use clap::Parser;
use cli::{Cli, CliConflictStrategy, Commands, McpSubcommand};
use config::AppConfig;
use console::style;
use discovery::ExternalDrive;
use indicatif::{ProgressBar, ProgressStyle};
use models::{CacheTarget, ConflictStrategy, TargetInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use ui::{QuickRunChoice, StepResult};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => run_interactive_tui(cli.verbose),
        Some(Commands::Mcp { action }) => match action {
            None | Some(McpSubcommand::Run) => mcp::run_mcp_server(),
            Some(McpSubcommand::Setup { overwrite }) => mcp_setup::run_mcp_setup_wizard(overwrite),
            Some(McpSubcommand::Status { json }) => mcp_setup::run_mcp_status(json),
        },
        Some(Commands::Scan { json }) => run_scan_command(json),
        Some(Commands::Migrate {
            targets,
            drive,
            dry_run,
            yes,
            conflict_strategy,
        }) => run_migrate_command(targets, drive, dry_run, yes, conflict_strategy, cli.verbose),
        Some(Commands::Restore {
            targets,
            drive,
            keep_external,
            dry_run,
            yes,
        }) => run_restore_command(targets, drive, keep_external, dry_run, yes, cli.verbose),
        Some(Commands::Repair {
            drive,
            conflict_strategy,
        }) => run_repair_command(drive, conflict_strategy),
        Some(Commands::Status) => run_status_command(),
        Some(Commands::Config { reset }) => run_config_command(reset),
    }
}

fn create_spinner(msg: String) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("-=\\#")
            .template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

fn run_interactive_tui(verbose: bool) -> Result<()> {
    ui::print_banner();
    let mut config = AppConfig::load();

    let drives = discovery::discover_external_drives()?;
    if drives.is_empty() {
        println!(
            "{}",
            style("No external APFS drive found in /Volumes/.").red().bold()
        );
        println!(
            "Please plug in an APFS-formatted external SSD and re-run {}.",
            style("mso").cyan().bold()
        );
        return Ok(());
    }

    let quick_choice = match ui::prompt_quick_run_or_customize(&config) {
        Ok(c) => c,
        Err(e) if e.to_string() == "CANCELLED" => {
            println!("{}", style("Operation cancelled. No files modified.").yellow());
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let (target_root_dir, chosen_targets) = if quick_choice == QuickRunChoice::UsePrevious {
        let last_drive_str = config.last_external_drive.as_ref().unwrap();
        let target_path = PathBuf::from(last_drive_str);

        let spinner = create_spinner("Assessing previous targets...".to_string());
        let mut target_infos = Vec::new();
        for target in CacheTarget::all() {
            if config.remembered_targets.contains(&target.key()) {
                spinner.set_message(format!("Scanning {}...", target.display_name()));
                if let Ok(info) = assessment::assess_target(&target, &target_path) {
                    target_infos.push(info);
                }
            }
        }
        spinner.finish_and_clear();
        (target_path, target_infos)
    } else {
        // Step-wise wizard loop with Session Scan Caching & Rescan Support
        let mut step = 1;
        let mut current_drive: Option<ExternalDrive> = None;
        let mut current_subfolder: Option<PathBuf> = None;
        let mut current_targets: Option<Vec<TargetInfo>> = None;
        let mut session_scan_cache: HashMap<PathBuf, Vec<TargetInfo>> = HashMap::new();

        loop {
            match step {
                1 => {
                    // Step 1: Select Drive
                    let drive = match ui::select_external_drive(&drives, &config) {
                        Ok(d) => d,
                        Err(e) if e.to_string() == "CANCELLED" => {
                            println!("{}", style("Operation cancelled. No files modified.").yellow());
                            return Ok(());
                        }
                        Err(e) => return Err(e),
                    };
                    current_drive = Some(drive);
                    step = 2;
                }
                2 => {
                    // Step 2: Select/Create Subfolder
                    let drive = current_drive.as_ref().unwrap();
                    let subfolder_res = match ui::prompt_target_subfolder(&drive.volume_path) {
                        Ok(r) => r,
                        Err(e) if e.to_string() == "CANCELLED" => {
                            println!("{}", style("Operation cancelled. No files modified.").yellow());
                            return Ok(());
                        }
                        Err(e) => return Err(e),
                    };

                    match subfolder_res {
                        StepResult::Value(path) => {
                            current_subfolder = Some(path);
                            step = 3;
                        }
                        StepResult::Back => {
                            step = 1; // Step-wise back to Step 1 Drive Selection!
                        }
                        _ => {
                            step = 2;
                        }
                    }
                }
                3 => {
                    // Step 3: Target Checklist (with Session Cache & Rescan Support)
                    let target_path = current_subfolder.as_ref().unwrap();

                    let target_infos = if let Some(cached) = session_scan_cache.get(target_path) {
                        cached.clone()
                    } else {
                        let spinner = create_spinner("Scanning local cache sizes & external SSD data...".to_string());
                        let mut infos = Vec::new();
                        for target in CacheTarget::all() {
                            spinner.set_message(format!("Scanning {}...", target.display_name()));
                            if let Ok(info) = assessment::assess_target(&target, target_path) {
                                infos.push(info);
                            }
                        }
                        spinner.finish_and_clear();
                        session_scan_cache.insert(target_path.clone(), infos.clone());
                        infos
                    };

                    ui::print_summary_table(&target_infos);
                    let targets_res = match ui::prompt_targets_to_migrate(&target_infos, &config, target_path) {
                        Ok(r) => r,
                        Err(e) if e.to_string() == "CANCELLED" => {
                            println!("{}", style("Operation cancelled. No files modified.").yellow());
                            return Ok(());
                        }
                        Err(e) => return Err(e),
                    };

                    match targets_res {
                        StepResult::Value(t) => {
                            current_targets = Some(t);
                            break (target_path.clone(), current_targets.unwrap());
                        }
                        StepResult::Back => {
                            step = 2; // Step-wise back to Step 2 Subfolder Selection!
                        }
                        StepResult::Rescan => {
                            println!("{}", style("Forcing rescan of local and external SSD caches...").cyan());
                            session_scan_cache.remove(target_path);
                            step = 3; // Trigger fresh scan loop!
                        }
                        StepResult::AddCustom(info) => {
                            if let Some(cached) = session_scan_cache.get_mut(target_path) {
                                cached.push(info);
                            }
                            step = 3; // Re-render Step 3 checklist with newly added custom target!
                        }
                    }
                }
                _ => break (current_subfolder.unwrap(), current_targets.unwrap()),
            }
        }
    };

    if chosen_targets.is_empty() {
        println!("No targets selected for migration. Exiting.");
        return Ok(());
    }

    config.last_external_drive = Some(target_root_dir.to_string_lossy().to_string());
    config.remembered_targets = chosen_targets.iter().map(|t| t.target.key()).collect();
    let _ = config.save();

    for info in &chosen_targets {
        let strategy = match info.state {
            models::PathState::Conflict { .. } | models::PathState::GhostLocal { .. } => {
                Some(ui::prompt_conflict_strategy(info)?)
            }
            _ => None,
        };

        println!(
            "Migrating {}...",
            style(info.target.display_name()).cyan().bold()
        );

        match migrator::migrate_target(info, strategy, false, verbose) {
            Ok(_) => {
                println!(
                    "{} Offloaded {} -> {}",
                    style("Done:").green().bold(),
                    info.local_path.display(),
                    info.external_path.display()
                );
            }
            Err(e) => {
                println!(
                    "{} Failed to migrate {}: {:#}",
                    style("Error:").red().bold(),
                    info.target.display_name(),
                    e
                );
            }
        }
    }

    println!();
    println!(
        "{}",
        style("Migration process completed!").green().bold()
    );
    Ok(())
}

fn run_restore_command(
    target_keys: Vec<String>,
    drive_path: Option<PathBuf>,
    keep_external: bool,
    dry_run: bool,
    _auto_confirm: bool,
    verbose: bool,
) -> Result<()> {
    ui::print_banner();

    let config = AppConfig::load();
    let drives = discovery::discover_external_drives()?;

    let drive_path = match drive_path {
        Some(d) => d,
        None => {
            if let Some(last_drive) = &config.last_external_drive {
                PathBuf::from(last_drive)
            } else {
                drives.first().map(|d| d.volume_path.clone()).unwrap_or_else(|| PathBuf::from("/Volumes/ExternalSSD"))
            }
        }
    };

    let spinner = create_spinner("Assessing targets for restore...".to_string());
    let mut target_infos = Vec::new();
    for target in CacheTarget::all() {
        spinner.set_message(format!("Checking {}...", target.display_name()));
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            target_infos.push(info);
        }
    }
    spinner.finish_and_clear();

    let targets_to_restore = if !target_keys.is_empty() {
        target_infos
            .into_iter()
            .filter(|t| target_keys.contains(&t.target.key()) || target_keys.contains(&"all".to_string()))
            .collect()
    } else {
        match ui::prompt_targets_to_restore(&target_infos) {
            Ok(t) => t,
            Err(e) if e.to_string() == "CANCELLED" => {
                println!("{}", style("Operation cancelled. No files modified.").yellow());
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    };

    if targets_to_restore.is_empty() {
        println!("No targets selected to restore. Exiting.");
        return Ok(());
    }

    for info in &targets_to_restore {
        println!(
            "Restoring {} to local Mac storage...",
            style(info.target.display_name()).cyan().bold()
        );

        match migrator::restore_target(info, keep_external, dry_run, verbose) {
            Ok(_) => {
                if !dry_run {
                    println!(
                        "{} Restored {}",
                        style("Done:").green().bold(),
                        info.target.display_name()
                    );
                }
            }
            Err(e) => {
                println!(
                    "{} Failed to restore {}: {:#}",
                    style("Error:").red().bold(),
                    info.target.display_name(),
                    e
                );
            }
        }
    }

    println!();
    println!("{}", style("Restore operation completed!").green().bold());
    Ok(())
}

fn run_scan_command(json_output: bool) -> Result<()> {
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives()?;
    
    let drive_path = if let Some(last_drive) = &config.last_external_drive {
        PathBuf::from(last_drive)
    } else {
        drives.first().map(|d| d.volume_path.clone()).unwrap_or_else(|| PathBuf::from("/Volumes/ExternalSSD"))
    };

    let spinner = if !json_output {
        Some(create_spinner("Scanning local cache sizes...".to_string()))
    } else {
        None
    };

    let mut target_infos = Vec::new();
    for target in CacheTarget::all() {
        if let Some(sp) = &spinner {
            sp.set_message(format!("Scanning {}...", target.display_name()));
        }
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            target_infos.push(info);
        }
    }

    if let Some(sp) = spinner {
        sp.finish_and_clear();
    }

    if json_output {
        println!("[");
        for (idx, t) in target_infos.iter().enumerate() {
            let comma = if idx + 1 < target_infos.len() { "," } else { "" };
            println!(
                "  {{\"component\": \"{}\", \"key\": \"{}\", \"size_bytes\": {}, \"local_path\": \"{}\"}}{}",
                t.target.display_name(),
                t.target.key(),
                t.size_bytes,
                t.local_path.display(),
                comma
            );
        }
        println!("]");
    } else {
        ui::print_banner();
        ui::print_summary_table(&target_infos);
    }
    Ok(())
}

fn run_migrate_command(
    target_keys: Vec<String>,
    drive_path: Option<PathBuf>,
    dry_run: bool,
    _auto_confirm: bool,
    conflict_strategy: Option<CliConflictStrategy>,
    verbose: bool,
) -> Result<()> {
    let config = AppConfig::load();
    let drive_path = match drive_path {
        Some(d) => d,
        None => {
            let drives = discovery::discover_external_drives()?;
            if let Some(last_drive) = &config.last_external_drive {
                PathBuf::from(last_drive)
            } else if !drives.is_empty() {
                drives[0].volume_path.clone()
            } else {
                return Err(anyhow!("No external drive found. Please specify --drive"));
            }
        }
    };

    let drive = discovery::validate_apfs_drive(&drive_path)?;

    let targets_to_process = if target_keys.contains(&"all".to_string()) || target_keys.is_empty() {
        CacheTarget::all()
    } else {
        CacheTarget::all()
            .into_iter()
            .filter(|t| target_keys.contains(&t.key()))
            .collect()
    };

    let strat = conflict_strategy.map(convert_cli_strategy);

    for target in targets_to_process {
        let info = assessment::assess_target(&target, &drive.volume_path)?;
        println!("Processing {}...", target.display_name());
        migrator::migrate_target(&info, strat, dry_run, verbose)?;
        if !dry_run {
            println!("Done for {}", target.display_name());
        }
    }

    Ok(())
}

fn run_repair_command(
    drive_path: Option<PathBuf>,
    conflict_strategy: Option<CliConflictStrategy>,
) -> Result<()> {
    ui::print_banner();
    println!("Checking for broken symlinks, ghost directories, and data conflicts...");
    println!();

    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = match drive_path {
        Some(d) => d,
        None => {
            if let Some(last_drive) = &config.last_external_drive {
                PathBuf::from(last_drive)
            } else if !drives.is_empty() {
                drives[0].volume_path.clone()
            } else {
                PathBuf::from("/Volumes/ExternalSSD")
            }
        }
    };

    let mut repaired_count = 0;
    let strat = conflict_strategy.map(convert_cli_strategy);

    for target in CacheTarget::all() {
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            match &info.state {
                models::PathState::GhostLocal { .. } => {
                    if migrator::repair_broken_links(&info)? {
                        println!(
                            "{} Repaired broken symlink for {}",
                            style("Done:").green().bold(),
                            target.display_name()
                        );
                        repaired_count += 1;
                    }
                }
                models::PathState::Conflict { .. } => {
                    let choose_strat = match strat {
                        Some(s) => s,
                        None => ui::prompt_conflict_strategy(&info)?,
                    };
                    println!("Resolving conflict for {}...", style(target.display_name()).cyan().bold());
                    migrator::migrate_target(&info, Some(choose_strat), false, false)?;
                    println!(
                        "{} Resolved conflict for {}",
                        style("Done:").green().bold(),
                        target.display_name()
                    );
                    repaired_count += 1;
                }
                _ => {}
            }
        }
    }

    if repaired_count == 0 {
        println!("No broken symlinks or data conflicts found.");
    } else {
        println!();
        println!(
            "{} Successfully repaired/resolved {} item(s).",
            style("Done:").green().bold(),
            repaired_count
        );
    }

    Ok(())
}

fn run_status_command() -> Result<()> {
    ui::print_banner();
    let config = AppConfig::load();
    let drives = discovery::discover_external_drives().unwrap_or_default();
    let drive_path = if let Some(last_drive) = &config.last_external_drive {
        PathBuf::from(last_drive)
    } else {
        drives
            .first()
            .map(|d| d.volume_path.clone())
            .unwrap_or_else(|| PathBuf::from("/Volumes/ExternalSSD"))
    };

    let spinner = create_spinner("Scanning local cache sizes...".to_string());
    let mut target_infos = Vec::new();
    for target in CacheTarget::all() {
        spinner.set_message(format!("Scanning {}...", target.display_name()));
        if let Ok(info) = assessment::assess_target(&target, &drive_path) {
            target_infos.push(info);
        }
    }
    spinner.finish_and_clear();

    ui::print_summary_table(&target_infos);
    Ok(())
}

fn run_config_command(reset: bool) -> Result<()> {
    ui::print_banner();
    let path = AppConfig::config_path().ok_or_else(|| anyhow!("Could not determine config path"))?;

    if reset {
        let empty_config = AppConfig::default();
        empty_config.save()?;
        println!("{} Reset configuration file to default.", style("Done:").green().bold());
        return Ok(());
    }

    let config = AppConfig::load();
    println!("Configuration path: {}", style(path.display()).cyan());
    println!();
    println!("Current configuration:");
    println!("  - Last External Drive: {:?}", config.last_external_drive);
    println!("  - Remembered Targets: {:?}", config.remembered_targets);
    println!("  - Default Conflict Strategy: {:?}", config.default_conflict_strategy);
    Ok(())
}

fn convert_cli_strategy(cli_strat: CliConflictStrategy) -> ConflictStrategy {
    match cli_strat {
        CliConflictStrategy::OverwriteExternal => ConflictStrategy::OverwriteExternal,
        CliConflictStrategy::Merge => ConflictStrategy::Merge,
        CliConflictStrategy::DiscardLocal => ConflictStrategy::DiscardLocal,
        CliConflictStrategy::KeepLocalDiscardExternal => ConflictStrategy::KeepLocalDiscardExternal,
        CliConflictStrategy::RollbackExternalToLocal => ConflictStrategy::RollbackExternalToLocal,
    }
}
