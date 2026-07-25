use crate::config::AppConfig;
use crate::discovery::ExternalDrive;
use crate::models::{ConflictStrategy, PathState, TargetInfo};
use anyhow::Result;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, Table};
use console::style;
use inquire::{InquireError, Select, Text};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
    let _ = io::stdout().flush();
}

pub fn print_banner() {
    clear_screen();
    println!("{}", style("==================================================").cyan());
    println!(
        "{} {}",
        style("  mso").bold().cyan(),
        style("— macOS Developer Storage Migrator (v0.1.0)").bold()
    );
    println!("{}", style("==================================================").cyan());
    println!();
}

#[derive(Debug, PartialEq, Eq)]
pub enum QuickRunChoice {
    UsePrevious,
    Customize,
}

pub fn prompt_quick_run_or_customize(config: &AppConfig) -> Result<QuickRunChoice> {
    let last_drive = match &config.last_external_drive {
        Some(d) => d,
        None => return Ok(QuickRunChoice::Customize),
    };

    if config.remembered_targets.is_empty() {
        return Ok(QuickRunChoice::Customize);
    }

    let targets_summary = config.remembered_targets.join(", ");
    println!("Previous setup found:");
    println!("   Drive/Folder: {}", style(last_drive).cyan());
    println!("   Targets:      {}", style(&targets_summary).cyan());
    println!();

    let options = vec![
        format!("Quick Run (Recommended: Use previous setup {} -> {})", last_drive, targets_summary),
        "Customize (Select drive/folder & components manually)".to_string(),
    ];

    let ans = match Select::new("How would you like to proceed?", options).prompt() {
        Ok(a) => a,
        Err(InquireError::OperationCanceled) => return Err(anyhow::anyhow!("CANCELLED")),
        Err(e) => return Err(e.into()),
    };

    if ans.starts_with("Quick Run") {
        Ok(QuickRunChoice::UsePrevious)
    } else {
        Ok(QuickRunChoice::Customize)
    }
}

pub fn select_external_drive(drives: &[ExternalDrive], config: &AppConfig) -> Result<ExternalDrive> {
    if drives.is_empty() {
        return Err(anyhow::anyhow!(
            "No external drives found in /Volumes/. Please plug in an APFS SSD and try again."
        ));
    }

    let items: Vec<String> = drives
        .iter()
        .map(|d| {
            if d.is_apfs {
                format!("{} [APFS] ({})", d.name, d.volume_path.display())
            } else {
                format!("{} [UNSUPPORTED: {}] ({}) — Require APFS format", d.name, d.file_system, d.volume_path.display())
            }
        })
        .collect();

    let default_index = drives
        .iter()
        .position(|d| d.is_apfs && (config.last_external_drive.as_ref().map_or(false, |last| &d.name == last || last.starts_with(&d.volume_path.to_string_lossy().to_string()))))
        .unwrap_or_else(|| drives.iter().position(|d| d.is_apfs).unwrap_or(0));

    let ans = match Select::new("Step 1: Select external target partition/drive:", items)
        .with_starting_cursor(default_index)
        .with_help_message("Use [Up/Down] to navigate, [ENTER] to select, [ESC] to cancel")
        .prompt()
    {
        Ok(a) => a,
        Err(InquireError::OperationCanceled) => return Err(anyhow::anyhow!("CANCELLED")),
        Err(e) => return Err(e.into()),
    };

    let selected_drive = drives
        .iter()
        .find(|d| {
            format!("{} [APFS] ({})", d.name, d.volume_path.display()) == ans
                || format!("{} [UNSUPPORTED: {}] ({}) — Require APFS format", d.name, d.file_system, d.volume_path.display()) == ans
        })
        .ok_or_else(|| anyhow::anyhow!("Selected drive not found"))?;

    if !selected_drive.is_apfs {
        println!(
            "External drive '{}' is formatted as '{}'.",
            selected_drive.name,
            selected_drive.file_system
        );
        println!(
            "{}",
            style("Error: External drive must be formatted as APFS to preserve Unix file permissions and symlinks. exFAT/NTFS are unsupported.").yellow()
        );
        println!("Please format the partition as APFS using macOS Disk Utility and try again.");
        return Err(anyhow::anyhow!("Unsupported filesystem '{}' on drive '{}'", selected_drive.file_system, selected_drive.name));
    }

    Ok(selected_drive.clone())
}

#[derive(Debug, PartialEq, Eq)]
pub enum StepResult<T> {
    Value(T),
    Back,
    Rescan,
    AddCustom(TargetInfo),
}

pub fn prompt_target_subfolder(volume_path: &Path) -> Result<StepResult<PathBuf>> {
    let mut existing_folders = Vec::new();
    if let Ok(entries) = fs::read_dir(volume_path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') && !name.starts_with('$') {
                        existing_folders.push(name.to_string());
                    }
                }
            }
        }
    }

    let mut options = vec![
        format!("Use Volume Root ({})", volume_path.display()),
    ];

    if !existing_folders.is_empty() {
        options.push(format!("Select Existing Folder on SSD ({} found)", existing_folders.len()));
    }

    options.push("+ Create New Subfolder (Type custom path)".to_string());
    options.push("‹ Back (Step 1: Drive Selection)".to_string());

    let choice = match Select::new("Step 2: Target root location on drive:", options)
        .with_help_message("Use [Up/Down] to navigate, [ENTER] to select, [ESC] to go back")
        .prompt()
    {
        Ok(c) => c,
        Err(InquireError::OperationCanceled) => return Ok(StepResult::Back),
        Err(e) => return Err(e.into()),
    };

    if choice.starts_with("‹ Back") {
        return Ok(StepResult::Back);
    }

    if choice.starts_with("Use Volume Root") {
        return Ok(StepResult::Value(volume_path.to_path_buf()));
    }

    if choice.starts_with("Select Existing Folder") {
        let selected = match Select::new("Select existing subfolder on SSD:", existing_folders)
            .with_help_message("Use [Up/Down] to navigate, [ENTER] to select, [ESC] to go back")
            .prompt()
        {
            Ok(s) => s,
            Err(InquireError::OperationCanceled) => return Ok(StepResult::Back),
            Err(e) => return Err(e.into()),
        };
        return Ok(StepResult::Value(volume_path.join(selected)));
    }

    // Create New Subfolder
    let input_subfolder = match Text::new("Enter subfolder path to create on SSD:")
        .with_placeholder("DevCaches/Offload")
        .with_help_message("Parent directories will be created automatically if missing")
        .prompt()
    {
        Ok(i) => i,
        Err(InquireError::OperationCanceled) => return Ok(StepResult::Back),
        Err(e) => return Err(e.into()),
    };

    let trimmed = input_subfolder.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Ok(StepResult::Value(volume_path.to_path_buf()));
    }

    let target_path = volume_path.join(trimmed);
    if !target_path.exists() {
        fs::create_dir_all(&target_path)?;
        println!(
            "Created target directory: {}",
            style(target_path.display()).cyan()
        );
    }

    Ok(StepResult::Value(target_path))
}

pub fn print_summary_table(targets: &[TargetInfo]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        Cell::new("Component").fg(Color::Cyan),
        Cell::new("Size").fg(Color::Cyan),
        Cell::new("Status").fg(Color::Cyan),
        Cell::new("Target Path").fg(Color::Cyan),
    ]);

    let mut found_existing_on_ssd = Vec::new();

    for t in targets {
        let status_cell = match &t.state {
            PathState::Fresh => Cell::new("Fresh (Local)").fg(Color::Yellow),
            PathState::AlreadyLinked { .. } => Cell::new("Linked").fg(Color::Green),
            PathState::RebindDrive { .. } => Cell::new("Drive Transfer Mode").fg(Color::Blue),
            PathState::GhostLocal { .. } => Cell::new("Ghost Local Dir").fg(Color::Red),
            PathState::Conflict { .. } => Cell::new("Data Conflict").fg(Color::Magenta),
            PathState::ExistingExternalData { .. } => {
                found_existing_on_ssd.push(t);
                Cell::new("Discovered on SSD").fg(Color::Cyan)
            }
            PathState::NotFound => Cell::new("Not Found").fg(Color::DarkGrey),
        };

        table.add_row(vec![
            Cell::new(t.target.display_name()),
            Cell::new(format_bytes(t.size_bytes)),
            status_cell,
            Cell::new(t.external_path.to_string_lossy()),
        ]);
    }

    println!("{}", table);

    if !found_existing_on_ssd.is_empty() {
        println!(
            "Config Recovery: Discovered {} pre-existing offloaded folder(s) on external drive:",
            found_existing_on_ssd.len()
        );
        for item in found_existing_on_ssd {
            println!(
                "   - {} ({}) at {}",
                style(item.target.display_name()).cyan(),
                format_bytes(item.size_bytes),
                item.external_path.display()
            );
        }
    }
    println!();
}

pub fn prompt_targets_to_migrate(
    targets: &[TargetInfo],
    config: &AppConfig,
    external_root: &Path,
) -> Result<StepResult<Vec<TargetInfo>>> {
    let unlinked_targets: Vec<&TargetInfo> = targets
        .iter()
        .filter(|t| !matches!(t.state, PathState::AlreadyLinked { .. }))
        .collect();

    if unlinked_targets.is_empty() {
        println!(
            "{}",
            style("All supported caches are already linked to external SSD!").green().bold()
        );
        return Ok(StepResult::Value(Vec::new()));
    }

    let mut build_outputs: Vec<&&TargetInfo> = unlinked_targets
        .iter()
        .filter(|t| t.target.is_build_output())
        .collect();
    build_outputs.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    let mut package_sources: Vec<&&TargetInfo> = unlinked_targets
        .iter()
        .filter(|t| !t.target.is_build_output())
        .collect();
    package_sources.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    let mut ordered_targets: Vec<&TargetInfo> = Vec::new();
    for t in &build_outputs {
        ordered_targets.push(**t);
    }
    for t in &package_sources {
        ordered_targets.push(**t);
    }

    // Checked state set
    let mut checked_keys: HashSet<String> = HashSet::new();
    for t in &ordered_targets {
        if matches!(t.state, PathState::ExistingExternalData { .. })
            || (t.target.is_build_output() && config.remembered_targets.is_empty())
            || config.remembered_targets.contains(&t.target.key())
        {
            checked_keys.insert(t.target.key());
        }
    }

    let mut cursor_index = 0;

    // Single-Stage In-Place Interactive Loop
    loop {
        clear_screen();
        print_summary_table(targets);

        let mut options: Vec<String> = Vec::new();

        // 1. Render Components with Checkbox State & Rich Informative Recommendations
        for t in &ordered_targets {
            let is_checked = checked_keys.contains(&t.target.key());
            let check_mark = if is_checked { "[x]" } else { "[ ]" };
            let cat_tag = match &t.state {
                PathState::ExistingExternalData { .. } => " [Discovered Files on SSD — Ready to Re-link]",
                _ => {
                    match t.target.key().as_str() {
                        "deriveddata" => " [BUILD OUTPUT — Safe to offload (Xcode automatically rebuilds caches)]",
                        "coresimulator" => " [BUILD OUTPUT — Safe to offload (iOS Simulator data)]",
                        "archives" => " [BUILD OUTPUT — Safe to offload (Xcode build archives)]",
                        "gradle" => " [BUILD OUTPUT — Safe to offload (Android build output & dependencies)]",
                        "pub-cache" => " [PACKAGE SOURCE — Keep local to preserve offline Flutter development]",
                        "npm" => " [PACKAGE SOURCE — Keep local to preserve offline Node/Web development]",
                        "m2" => " [PACKAGE SOURCE — Keep local for offline Java/Maven dependencies]",
                        "cargo" => " [PACKAGE SOURCE — Keep local for offline Rust crate sources]",
                        "cocoapods" => " [PACKAGE SOURCE — Keep local for offline CocoaPods spec repos]",
                        _ => {
                            if t.target.is_build_output() {
                                " [BUILD OUTPUT — Safe to offload]"
                            } else {
                                " [PACKAGE SOURCE — Keep local for offline work]"
                            }
                        }
                    }
                }
            };

            options.push(format!(
                "{} {} ({}) — {}{}",
                check_mark,
                t.target.display_name(),
                format_bytes(t.size_bytes),
                state_label(&t.state),
                cat_tag
            ));
        }

        let selected_count = checked_keys.len();
        let selected_bytes: u64 = ordered_targets
            .iter()
            .filter(|t| checked_keys.contains(&t.target.key()))
            .map(|t| t.size_bytes)
            .sum();

        // 2. Render Separator Line & Action Items (with Select Recommended)
        options.push("────────────────────────────────────────────────────────────".to_string());
        
        let start_action_label = if selected_count > 0 {
            format!("✔ Start Migration Now ({} selected — {})", selected_count, format_bytes(selected_bytes))
        } else {
            "✔ Start Migration Now (0 selected)".to_string()
        };
        options.push(start_action_label);
        options.push("[*] Auto-Select Recommended (Build Outputs Only)".to_string());
        options.push("[+] Select All Components".to_string());
        options.push("[-] Select None (Clear All Checkboxes)".to_string());
        options.push("+ Add Custom Local Folder Path to Offload...".to_string());
        options.push("↻ Force Rescan Local & SSD Caches...".to_string());
        options.push("‹ Back to Subfolder Selection (Step 2)".to_string());

        let help_text = "Use [Up/Down] & [ENTER] to toggle component or run action | [ESC] to go back";

        let selected_choice = match Select::new("Step 3: Toggle components to offload or trigger actions:", options)
            .with_starting_cursor(cursor_index)
            .with_page_size(24)
            .with_help_message(help_text)
            .prompt()
        {
            Ok(c) => c,
            Err(InquireError::OperationCanceled) => return Ok(StepResult::Back),
            Err(e) => return Err(e.into()),
        };

        if selected_choice.contains("───────────────────────────") {
            continue; // Ignore separator line if selected
        }

        if selected_choice.starts_with("✔ Start Migration Now") {
            let selected_targets: Vec<TargetInfo> = ordered_targets
                .into_iter()
                .filter(|t| checked_keys.contains(&t.target.key()))
                .cloned()
                .collect();
            return Ok(StepResult::Value(selected_targets));
        }

        if selected_choice.starts_with("[*] Auto-Select Recommended") {
            cursor_index = ordered_targets.len() + 2; // Keep cursor on utility action row!
            checked_keys.clear();
            for t in &ordered_targets {
                if t.target.is_build_output() || matches!(t.state, PathState::ExistingExternalData { .. }) {
                    checked_keys.insert(t.target.key());
                }
            }
            continue;
        }

        if selected_choice.starts_with("[+] Select All") {
            cursor_index = ordered_targets.len() + 3; // Keep cursor on utility action row!
            for t in &ordered_targets {
                checked_keys.insert(t.target.key());
            }
            continue;
        }

        if selected_choice.starts_with("[-] Select None") {
            cursor_index = ordered_targets.len() + 4; // Keep cursor on utility action row!
            checked_keys.clear();
            continue;
        }

        if selected_choice.starts_with("+ Add Custom") {
            if let Ok(custom_path_str) = Text::new("Enter custom local folder path to offload:")
                .with_placeholder("~/Library/Caches/Docker or /Users/name/Downloads/BigFolder")
                .with_help_message("Absolute path or ~ relative path to local directory")
                .prompt()
            {
                let expanded_str = if custom_path_str.starts_with("~/") {
                    if let Some(home) = dirs::home_dir() {
                        home.join(&custom_path_str[2..]).to_string_lossy().to_string()
                    } else {
                        custom_path_str
                    }
                } else {
                    custom_path_str
                };

                let local_p = PathBuf::from(&expanded_str);
                if local_p.exists() && local_p.is_dir() {
                    let name = local_p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("CustomFolder")
                        .to_string();

                    let custom_target = crate::models::CacheTarget::Custom {
                        name,
                        local_rel_path: local_p.clone(),
                    };

                    if let Ok(info) = crate::assessment::assess_target(&custom_target, external_root) {
                        println!(
                            "Added custom path: {} ({})",
                            local_p.display(),
                            format_bytes(info.size_bytes)
                        );
                        return Ok(StepResult::AddCustom(info));
                    }
                } else {
                    println!(
                        "Warning: Path '{}' does not exist or is not a directory.",
                        expanded_str
                    );
                }
            }
            return Ok(StepResult::Rescan);
        }

        if selected_choice.starts_with("↻ Force Rescan") {
            return Ok(StepResult::Rescan);
        }

        if selected_choice.starts_with("‹ Back") {
            return Ok(StepResult::Back);
        }

        // Handle Component Toggle!
        if let Some(pos) = ordered_targets.iter().position(|t| {
            selected_choice.contains(&t.target.display_name())
        }) {
            cursor_index = pos;
            let target_key = ordered_targets[pos].target.key();
            if checked_keys.contains(&target_key) {
                checked_keys.remove(&target_key);
            } else {
                checked_keys.insert(target_key);
            }
        }
    }
}

pub fn prompt_targets_to_restore(targets: &[TargetInfo]) -> Result<Vec<TargetInfo>> {
    let linked_targets: Vec<&TargetInfo> = targets
        .iter()
        .filter(|t| matches!(t.state, PathState::AlreadyLinked { .. }))
        .collect();

    if linked_targets.is_empty() {
        println!(
            "{}",
            style("No components are currently linked to external SSD.").yellow().bold()
        );
        return Ok(Vec::new());
    }

    let avail_bytes = get_mac_available_space_bytes().unwrap_or(u64::MAX);

    println!(
        "Internal Mac Disk Space Available: {}",
        style(format_bytes(avail_bytes)).cyan().bold()
    );
    println!();

    let selectable_options: Vec<String> = linked_targets
        .iter()
        .map(|t| {
            let space_rec = if t.size_bytes + 10_000_000_000 > avail_bytes {
                " [Insufficient Space on Mac Internal SSD]"
            } else if t.size_bytes < 1_000_000_000 {
                " [Safe to Restore (Low Space Impact)]"
            } else {
                " [Moderate Space Impact]"
            };

            format!(
                "{} ({}){}",
                t.target.display_name(),
                format_bytes(t.size_bytes),
                space_rec
            )
        })
        .collect();

    let default_indices: Vec<usize> = (0..selectable_options.len()).collect();

    let choices = match inquire::MultiSelect::new(
        "Select components to RESTORE back to local Mac storage:",
        selectable_options,
    )
    .with_page_size(24)
    .with_help_message("Press [SPACE] to select/deselect, [ENTER] to execute restore, [ESC] to cancel")
    .with_default(&default_indices)
    .prompt() {
        Ok(c) => c,
        Err(InquireError::OperationCanceled) => return Err(anyhow::anyhow!("CANCELLED")),
        Err(e) => return Err(e.into()),
    };

    let selected_targets: Vec<TargetInfo> = linked_targets
        .into_iter()
        .filter(|t| {
            choices.iter().any(|c| c.contains(&t.target.display_name()))
        })
        .cloned()
        .collect();

    let total_restore_bytes: u64 = selected_targets.iter().map(|t| t.size_bytes).sum();
    let required_with_buffer = total_restore_bytes + 10_000_000_000;

    if avail_bytes != u64::MAX && required_with_buffer > avail_bytes {
        println!(
            "{}",
            style("Error: Insufficient internal disk space to restore selected targets!").red().bold()
        );
        println!(
            "   Selected Targets Size: {} (+ 10GB macOS safety buffer = {})",
            format_bytes(total_restore_bytes),
            format_bytes(required_with_buffer)
        );
        println!("   Available on Mac SSD:  {}", format_bytes(avail_bytes));
        return Err(anyhow::anyhow!("Insufficient space to restore back to local Mac storage. Please free up space or select fewer targets."));
    }

    Ok(selected_targets)
}

pub fn prompt_conflict_strategy(target: &TargetInfo) -> Result<ConflictStrategy> {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_chars("-=\\#")
            .template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner()),
    );
    pb.set_message(format!("Evaluating path sizes for {}...", target.target.display_name()));
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let local_bytes = crate::assessment::get_fast_dir_size_bytes(&target.local_path).unwrap_or(0);
    let external_bytes = crate::assessment::get_fast_dir_size_bytes(&target.external_path).unwrap_or(0);

    pb.finish_and_clear();

    let local_size_str = format_bytes(local_bytes);
    let external_size_str = format_bytes(external_bytes);

    println!(
        "Conflict Diagnostic for: {}",
        style(target.target.display_name()).cyan().bold()
    );
    println!(
        "   Local path:    {} [Size: {}]",
        target.local_path.display(),
        style(&local_size_str).yellow().bold()
    );
    println!(
        "   External path: {} [Size: {}]",
        target.external_path.display(),
        style(&external_size_str).cyan().bold()
    );

    let diag_note = if external_bytes < local_bytes && external_bytes > 0 {
        format!(
            "Diagnostic Note: External SSD contains a partial backup ({}) vs Local ({}) — transfer was likely interrupted mid-execution.",
            external_size_str, local_size_str
        )
    } else if external_bytes == local_bytes && external_bytes > 0 {
        format!(
            "Diagnostic Note: External SSD and Local Mac both contain identical dataset sizes ({}).",
            local_size_str
        )
    } else if external_bytes > local_bytes {
        format!(
            "Diagnostic Note: External SSD contains a larger backup ({}) than current Local path ({}).",
            external_size_str, local_size_str
        )
    } else {
        "Diagnostic Note: Data detected on both local Mac storage and external APFS SSD.".to_string()
    };

    println!("   {}", style(diag_note).dim());
    println!();

    let options = vec![
        format!("(Recommended: Safe Merge) Merge Local into External [Syncs missing files to SSD & frees local space]"),
        format!("Discard Local and Restore Symlink [Frees {} local space, uses {} SSD backup]", local_size_str, external_size_str),
        format!("Overwrite External with Local [Deletes {} SSD backup & re-copies {} from Mac]", external_size_str, local_size_str),
        format!("Rollback SSD Data to Local [Copies {} SSD data back to Mac & deletes SSD copy]", external_size_str),
        format!("Keep Local & Discard SSD Backup [Deletes {} SSD copy & leaves local Mac folder unchanged]", external_size_str),
    ];

    let ans = match Select::new("Choose resolution strategy with recommendations:", options)
        .with_help_message("Use [Up/Down] to navigate, [ENTER] to select strategy")
        .prompt()
    {
        Ok(a) => a,
        Err(InquireError::OperationCanceled) => return Ok(ConflictStrategy::Merge),
        Err(e) => return Err(e.into()),
    };

    Ok(match ans {
        s if s.starts_with("Discard Local") => ConflictStrategy::DiscardLocal,
        s if s.starts_with("Overwrite External") => ConflictStrategy::OverwriteExternal,
        s if s.starts_with("Rollback SSD Data") => ConflictStrategy::RollbackExternalToLocal,
        s if s.starts_with("Keep Local") => ConflictStrategy::KeepLocalDiscardExternal,
        _ => ConflictStrategy::Merge,
    })
}

pub fn get_mac_available_space_bytes() -> Option<u64> {
    let output = Command::new("df").arg("-k").arg("/").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().nth(1)?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 4 {
        let avail_kb: u64 = parts[3].parse().ok()?;
        Some(avail_kb * 1024)
    } else {
        None
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let i = (bytes as f64).log(1024.0).floor() as usize;
    let p = 1024.0_f64.powi(i as i32);
    let s = (bytes as f64) / p;
    format!("{:.1} {}", s, units[i.min(units.len() - 1)])
}

fn state_label(state: &PathState) -> &'static str {
    match state {
        PathState::Fresh => "Fresh",
        PathState::AlreadyLinked { .. } => "Linked",
        PathState::RebindDrive { .. } => "Drive Transfer Mode",
        PathState::GhostLocal { .. } => "Ghost Local",
        PathState::Conflict { .. } => "Conflict",
        PathState::ExistingExternalData { .. } => "Discovered on SSD",
        PathState::NotFound => "Empty",
    }
}
