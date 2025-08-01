use anyhow::{Result, Context};
use clap::Parser as ClapParser;
use regex::Regex;
use std::io::{Read, Write, IsTerminal};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use chrono::{DateTime, Local, NaiveTime};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs;
use crossterm::{
    terminal::{enable_raw_mode, disable_raw_mode},
    event::{self, Event, KeyCode, KeyModifiers},
};

#[derive(ClapParser, Debug)]
#[command(author, version, about = "Automate Claude task execution from Markdown files", long_about = None)]
struct Args {
    /// Path to the Markdown file containing tasks
    md_file: PathBuf,
    
    /// Enable debug mode to see raw output
    #[arg(long, short)]
    debug: bool,
}

struct Claudia {
    md_file: PathBuf,
    output_buffer: Arc<Mutex<String>>,
    last_output_time: Arc<Mutex<Instant>>,
    continue_count: Arc<Mutex<u32>>,
    status: Arc<Mutex<String>>,
    response_history: Arc<Mutex<Vec<String>>>,
}

impl Claudia {
    fn new(md_file: PathBuf) -> Self {
        Self {
            md_file,
            output_buffer: Arc::new(Mutex::new(String::new())),
            last_output_time: Arc::new(Mutex::new(Instant::now())),
            continue_count: Arc::new(Mutex::new(0)),
            status: Arc::new(Mutex::new("Starting...".to_string())),
            response_history: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    // Helper function to safely get last N chars from a string
    fn safe_suffix(s: &str, max_chars: usize) -> &str {
        let char_count = s.chars().count();
        if char_count <= max_chars {
            return s;
        }
        
        let skip_chars = char_count - max_chars;
        let mut char_indices = s.char_indices();
        
        // Skip to the desired starting position
        for _ in 0..skip_chars {
            char_indices.next();
        }
        
        // Get the byte index of the start position
        if let Some((byte_idx, _)) = char_indices.next() {
            &s[byte_idx..]
        } else {
            s
        }
    }

    fn create_initial_prompt(&self) -> String {
        format!(
            "Please read and complete all tasks in the file: {}\n\
             The file is located at: {}\n\
             Work through each task and:\n\
             1. Complete the task as described\n\
             2. Edit the markdown file to change [ ] to [x] for each completed task",
            self.md_file.file_name().unwrap_or_default().to_string_lossy(),
            self.md_file.display()
        )
    }

    fn update_status(&self, status: &str) {
        *self.status.lock().unwrap() = status.to_string();
        self.display_status();
    }

    fn display_status(&self) {
        let status = self.status.lock().unwrap();
        let continues = self.continue_count.lock().unwrap();
        println!("\n╔════════════════════ CLAUDIA STATUS ════════════════════╗");
        println!("║ {:<54} ║", status);
        if *continues > 0 {
            println!("║ Continues sent: {:<38} ║", continues);
        }
        println!("╚════════════════════════════════════════════════════════╝\n");
    }

    fn run(&self) -> Result<()> {
        // Check if claude command exists
        if std::process::Command::new("which")
            .arg("claude")
            .output()
            .map(|output| !output.status.success())
            .unwrap_or(true) {
            anyhow::bail!("Claude command not found. Please ensure Claude CLI is installed and in PATH.");
        }
        
        // Ensure all tasks have checkboxes
        self.ensure_checkboxes()?;
        
        let initial_prompt = self.create_initial_prompt();
        
        // Get the directory of the markdown file
        let working_dir = self.md_file.parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        
        println!("Starting Claude with task file: {}", self.md_file.display());
        println!("Working directory: {}", working_dir.display());
        println!();
        
        // Create a new pty
        let pty_system = native_pty_system();
        
        // Create a new pty pair with terminal size
        let pair = pty_system.openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }).context("Failed to create PTY")?;
        
        // Build the command
        let mut cmd = CommandBuilder::new("claude");
        cmd.arg("--dangerously-skip-permissions");
        cmd.cwd(working_dir);
        
        // Spawn the command in the pty
        let mut child = pair.slave.spawn_command(cmd)
            .context("Failed to spawn Claude process")?;
        
        // Get reader/writer for the master side
        let mut reader = pair.master.try_clone_reader()
            .context("Failed to clone reader")?;
        let mut writer = pair.master.take_writer()
            .context("Failed to get writer")?;
        
        // Send initial prompt
        self.update_status("Sending initial prompt to Claude...");
        if std::env::args().any(|arg| arg == "--debug" || arg == "-d") {
            eprintln!("[DEBUG] Sending initial prompt: {:?}", initial_prompt);
        }
        // Write the text first
        write!(writer, "{}", initial_prompt)?;
        writer.flush()?;
        thread::sleep(Duration::from_millis(50));
        // Then send Enter key (carriage return)
        writer.write_all(&[0x0D])?; // CR (Enter key)
        writer.flush()?;
        thread::sleep(Duration::from_millis(100)); // Give PTY time to process
        self.update_status("Claude is working...");
        
        // Clone Arc references for the monitoring thread
        let output_buffer_clone = Arc::clone(&self.output_buffer);
        let last_output_time_clone = Arc::clone(&self.last_output_time);
        
        // Create channel for user input (now sends raw bytes)
        let (user_tx, user_rx) = mpsc::channel::<Vec<u8>>();
        
        // Setup Ctrl+C handler before enabling raw mode
        let should_exit = Arc::new(Mutex::new(false));
        let should_exit_clone = Arc::clone(&should_exit);
        
        // Only enable raw mode and start input thread if we're in a TTY
        let is_tty = std::io::stdin().is_terminal();
        
        if is_tty {
            // Enable raw mode for terminal
            enable_raw_mode().context("Failed to enable raw mode")?;
        }
        
        // Start user input thread only if in TTY
        let _input_thread = if is_tty {
            Some(thread::spawn(move || {
                loop {
                    // Check if we should exit
                    if *should_exit_clone.lock().unwrap() {
                        break;
                    }
                    
                    // Check for keyboard events with a short timeout
                    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                        if let Ok(Event::Key(key_event)) = event::read() {
                            // Check for Ctrl+C
                            if matches!(key_event.code, KeyCode::Char('c')) && 
                               key_event.modifiers.contains(KeyModifiers::CONTROL) {
                                // Exit gracefully
                                disable_raw_mode().ok();
                                println!("\n\nInterrupted by user. Exiting...");
                                std::process::exit(0);
                            }
                            
                            let bytes = match key_event.code {
                                // Arrow keys
                                KeyCode::Up => vec![0x1B, b'[', b'A'],
                                KeyCode::Down => vec![0x1B, b'[', b'B'],
                                KeyCode::Right => vec![0x1B, b'[', b'C'],
                                KeyCode::Left => vec![0x1B, b'[', b'D'],
                                // Enter key
                                KeyCode::Enter => vec![0x0D],
                                // Regular characters
                                KeyCode::Char(c) => c.to_string().into_bytes(),
                                // Backspace
                                KeyCode::Backspace => vec![0x7F],
                                // Tab
                                KeyCode::Tab => vec![0x09],
                                // Escape
                                KeyCode::Esc => vec![0x1B],
                                // Other keys - ignore for now
                                _ => vec![],
                            };
                            
                            if !bytes.is_empty() {
                                if user_tx.send(bytes).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };
        
        // Start output monitoring thread
        println!("\n════════════════════════════════════════════════════════════");
        println!("                      CLAUDE SESSION START                   ");
        println!("════════════════════════════════════════════════════════════\n");
        
        let output_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let output = String::from_utf8_lossy(&buf[..n]);
                        
                        // Print the output exactly as received
                        print!("{}", output);
                        std::io::stdout().flush().ok();
                        
                        // Update buffer
                        let mut buffer = output_buffer_clone.lock().unwrap();
                        buffer.push_str(&output);
                        
                        // Keep only recent output
                        if buffer.len() > 2000 {
                            // Use char_indices to respect UTF-8 boundaries
                            let skip_chars = buffer.chars().count().saturating_sub(2000);
                            *buffer = buffer.chars().skip(skip_chars).collect();
                        }
                        
                        *last_output_time_clone.lock().unwrap() = Instant::now();
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            eprintln!("Read error: {}", e);
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });
        
        // Main monitoring loop
        loop {
            thread::sleep(Duration::from_millis(100)); // Faster response for user input
            
            // Check for user input
            if let Ok(user_bytes) = user_rx.try_recv() {
                // User pressed a key, send raw bytes to Claude
                writer.write_all(&user_bytes)?;
                writer.flush()?;
                
                // Only reset tracking for actual character input (not just arrow keys)
                if !user_bytes.is_empty() && user_bytes[0] != 0x1B {
                    *self.last_output_time.lock().unwrap() = Instant::now();
                }
            }
            
            // Check if process is still running
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.update_status(&format!("Claude process exited with status: {:?}", status));
                    break;
                }
                Ok(None) => {
                    // Process is still running
                }
                Err(e) => {
                    self.update_status(&format!("Error checking process status: {}", e));
                    break;
                }
            }
            
            let buffer = self.output_buffer.lock().unwrap().clone();
            let time_since_output = self.last_output_time.lock().unwrap().elapsed();
            
            // Check for usage limit (time shown at bottom right)
            if let Some(wait_until) = Self::check_usage_limit(&buffer) {
                let time_str = wait_until.format("%-I:%M%p").to_string().to_lowercase();
                
                if std::env::args().any(|arg| arg == "--debug" || arg == "-d") {
                    eprintln!("[DEBUG] Usage limit detected. Wait until: {}", time_str);
                }
                
                // Clear any pending output first
                thread::sleep(Duration::from_millis(100));
                
                // Use eprintln to write to stderr which won't be overwritten by Claude's stdout
                eprintln!("\n\n");
                eprintln!("════════════════════════════════════════════════════════════");
                eprintln!("                    USAGE LIMIT DETECTED                     ");
                eprintln!("════════════════════════════════════════════════════════════");
                eprintln!();
                eprintln!("  Claude has reached its usage limit.");
                eprintln!("  Waiting until {} to continue...", time_str);
                eprintln!();
                eprintln!("  This message will remain visible during the wait.");
                eprintln!();
                eprintln!("════════════════════════════════════════════════════════════");
                eprintln!("\n");
                
                // Also print to stdout with some newlines to push Claude's output down
                println!("\n\n\n\n\n");
                
                Self::wait_for_limit_reset(wait_until)?;
                
                *self.continue_count.lock().unwrap() += 1;
                
                // Clear and show resuming message (use stderr)
                eprintln!("\n════════════════════════════════════════════════════════════");
                eprintln!("                      RESUMING SESSION                       ");
                eprintln!("════════════════════════════════════════════════════════════\n");
                
                self.update_status("Sending Continue after usage limit wait...");
                write!(writer, "Continue")?;
                writer.flush()?;
                thread::sleep(Duration::from_millis(50));
                writer.write_all(&[0x0D])?; // CR (Enter key)
                writer.flush()?;
                *self.output_buffer.lock().unwrap() = String::new();
                *self.last_output_time.lock().unwrap() = Instant::now();
                self.update_status("Claude is working...");
                continue;
            }
            
            // Check for bypass permissions prompt
            if Self::check_bypass_permissions_prompt(&buffer) {
                self.update_status("Detected bypass permissions prompt, accepting...");
                if std::env::args().any(|arg| arg == "--debug" || arg == "-d") {
                    eprintln!("[DEBUG] Bypass permissions prompt detected, sending '2' to accept");
                }
                // Send "2" to accept
                writer.write_all(b"2")?;
                writer.flush()?;
                thread::sleep(Duration::from_millis(50));
                // Send Enter key
                writer.write_all(&[0x0D])?;
                writer.flush()?;
                *self.output_buffer.lock().unwrap() = String::new();
                *self.last_output_time.lock().unwrap() = Instant::now();
                continue;
            }
            
            // Check if we need to send Continue
            // Logic: If "esc to interrupt" is NOT present (Claude has stopped) AND 
            //        we haven't had output for 60 seconds AND tasks aren't all completed
            if time_since_output > Duration::from_secs(60) && !Self::is_claude_running(&buffer) {
                // Check if all tasks are completed
                if self.check_all_tasks_completed() {
                    self.update_status("All tasks completed! Exiting...");
                    child.kill()?;
                    break;
                }
                
                // Check for repeated patterns before sending another Continue
                if self.check_repeated_pattern(&buffer) {
                    self.update_status("Detected repeated pattern. Claude may be stuck. Exiting...");
                    eprintln!("\n[ERROR] Claude appears to be stuck in a loop. Exiting to prevent infinite retries.");
                    child.kill()?;
                    break;
                }
                
                *self.continue_count.lock().unwrap() += 1;
                let count = *self.continue_count.lock().unwrap();
                
                // Also check if we've sent too many continues
                if count > 50 {
                    self.update_status("Maximum continue limit reached. Exiting...");
                    eprintln!("\n[ERROR] Sent 50 Continue commands. Something may be wrong. Exiting.");
                    child.kill()?;
                    break;
                }
                
                self.update_status(&format!("Claude stopped. Sending Continue #{}...", count));
                write!(writer, "Continue")?;
                writer.flush()?;
                thread::sleep(Duration::from_millis(50));
                writer.write_all(&[0x0D])?; // CR (Enter key)
                writer.flush()?;
                *self.output_buffer.lock().unwrap() = String::new();
                *self.last_output_time.lock().unwrap() = Instant::now();
                self.update_status("Claude is working...");
            }
            // If "esc to interrupt" is present, Claude is still working - just wait
        }
        
        // Signal input thread to exit
        *should_exit.lock().unwrap() = true;
        
        // Drop the sender to signal input thread to stop
        drop(writer);
        
        // Disable raw mode before printing final messages (only if it was enabled)
        if is_tty {
            disable_raw_mode().ok();
        }
        
        // Wait for threads to finish
        output_thread.join().ok();
        // Give input thread time to exit cleanly
        thread::sleep(Duration::from_millis(100));
        
        println!("\n════════════════════════════════════════════════════════════");
        println!("                       CLAUDE SESSION END                    ");
        println!("════════════════════════════════════════════════════════════");
        
        // Display final summary
        let continues = *self.continue_count.lock().unwrap();
        println!("\n╔═══════════════════ CLAUDIA SUMMARY ═══════════════════╗");
        println!("║ Total Continue commands sent: {:<23} ║", continues);
        println!("║ Session ended successfully                            ║");
        println!("╚═══════════════════════════════════════════════════════╝\n");
        
        Ok(())
    }


    fn check_usage_limit(buffer: &str) -> Option<DateTime<Local>> {
        // Look for specific usage limit patterns from Claude
        // Common patterns: "usage limit", "rate limit", "try again at", "please wait until"
        let recent = Self::safe_suffix(buffer, 2000);
        let recent_lower = recent.to_lowercase();
        
        // Check if this is actually a usage limit message
        if !recent_lower.contains("usage limit") && 
           !recent_lower.contains("rate limit") && 
           !recent_lower.contains("try again") &&
           !recent_lower.contains("please wait") {
            return None;
        }
        
        // Now look for time pattern near the usage limit message
        let time_pattern = Regex::new(r"(\d{1,2})([:.]?\d{0,2})\s*([ap]\.?m)").ok()?;
        
        if let Some(captures) = time_pattern.captures(&recent_lower) {
            let hour = captures.get(1)?.as_str().parse::<u32>().ok()?;
            let minutes_part = captures.get(2)?.as_str();
            let am_pm = captures.get(3)?.as_str();
            
            let minutes = if minutes_part.len() > 1 {
                minutes_part.trim_start_matches(':').trim_start_matches('.').parse::<u32>().unwrap_or(0)
            } else {
                0
            };
            
            let hour_24 = if am_pm.starts_with('p') && hour != 12 {
                hour + 12
            } else if am_pm.starts_with('a') && hour == 12 {
                0
            } else {
                hour
            };
            
            if let Some(time) = NaiveTime::from_hms_opt(hour_24, minutes, 0) {
                let now = Local::now();
                let mut wait_until = now.date_naive().and_time(time).and_local_timezone(Local).unwrap();
                
                if wait_until <= now {
                    wait_until = wait_until + chrono::Duration::days(1);
                }
                
                return Some(wait_until);
            }
        }
        
        None
    }

    fn wait_for_limit_reset(wait_until: DateTime<Local>) -> Result<()> {
        let now = Local::now();
        if wait_until > now {
            let duration = wait_until - now;
            let total_seconds = duration.num_seconds();
            
            // Show countdown every 30 seconds
            let mut remaining = total_seconds;
            while remaining > 0 {
                let mins = remaining / 60;
                let secs = remaining % 60;
                
                print!("\r  Time remaining: {:02}:{:02} ", mins, secs);
                std::io::stdout().flush().ok();
                
                let sleep_duration = std::cmp::min(remaining, 30);
                thread::sleep(Duration::from_secs(sleep_duration as u64));
                remaining -= sleep_duration;
            }
            println!("\r  Time remaining: 00:00 - Resuming now!");
        }
        Ok(())
    }

    fn is_claude_running(buffer: &str) -> bool {
        // Check the last 200 chars for "esc to interrupt"
        // If "esc to interrupt" is present, Claude is still running
        let recent = Self::safe_suffix(buffer, 200);
        recent.to_lowercase().contains("esc to interrupt")
    }
    
    fn check_bypass_permissions_prompt(buffer: &str) -> bool {
        // Check for the bypass permissions prompt
        let recent = Self::safe_suffix(buffer, 1500);
        let recent_lower = recent.to_lowercase();
        
        // Look for the characteristic prompt patterns
        if recent_lower.contains("bypass permissions mode") &&
           recent_lower.contains("1. no, exit") &&
           recent_lower.contains("2. yes, i accept") {
            return true;
        }
        
        // Also check for variations
        if recent.contains("WARNING: Claude Code running in Bypass Permissions mode") &&
           (recent.contains("1. No, exit") || recent.contains("2. Yes, I accept")) {
            return true;
        }
        
        false
    }
    
    fn check_all_tasks_completed(&self) -> bool {
        // Read the markdown file and check if all checkboxes are marked
        if let Ok(content) = fs::read_to_string(&self.md_file) {
            // Count all checkbox patterns (-, *, +)
            let unchecked = content.matches("[ ]").count();
            let checked = content.matches("[x]").count() + content.matches("[X]").count();
            
            // If there are checkboxes and all are checked, tasks are complete
            if checked > 0 && unchecked == 0 {
                return true;
            }
        }
        false
    }
    
    fn ensure_checkboxes(&self) -> Result<()> {
        // Read the markdown file
        let content = fs::read_to_string(&self.md_file)
            .context("Failed to read markdown file")?;
        
        let mut modified = false;
        let mut new_content = String::new();
        
        // Process each line
        for line in content.lines() {
            let trimmed = line.trim_start();
            
            // Check if this is a list item without a checkbox
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") || 
                trimmed.starts_with("+ ") || trimmed.starts_with(char::is_numeric) {
                
                // Check if it already has a checkbox
                if !trimmed.contains("- [ ]") && !trimmed.contains("- [x]") && 
                   !trimmed.contains("- [X]") && !trimmed.contains("* [ ]") && 
                   !trimmed.contains("* [x]") && !trimmed.contains("* [X]") {
                    
                    // Add checkbox after the list marker
                    if trimmed.starts_with("- ") {
                        new_content.push_str(&line.replace("- ", "- [ ] "));
                        modified = true;
                    } else if trimmed.starts_with("* ") {
                        new_content.push_str(&line.replace("* ", "* [ ] "));
                        modified = true;
                    } else if trimmed.starts_with("+ ") {
                        new_content.push_str(&line.replace("+ ", "+ [ ] "));
                        modified = true;
                    } else if let Some(pos) = trimmed.find(". ") {
                        // Numbered list
                        let (_num, rest) = trimmed.split_at(pos + 2);
                        new_content.push_str(&format!("{}- [ ] {}", 
                            " ".repeat(line.len() - trimmed.len()), rest));
                        modified = true;
                    } else {
                        new_content.push_str(line);
                    }
                } else {
                    new_content.push_str(line);
                }
            } else {
                new_content.push_str(line);
            }
            new_content.push('\n');
        }
        
        // Remove the last newline if the original didn't have one
        if !content.ends_with('\n') && new_content.ends_with('\n') {
            new_content.pop();
        }
        
        // Write back if modified
        if modified {
            fs::write(&self.md_file, new_content)
                .context("Failed to write updated markdown file")?;
            println!("Added checkboxes to tasks in {}", self.md_file.display());
        }
        
        Ok(())
    }
    
    fn check_repeated_pattern(&self, current_buffer: &str) -> bool {
        let mut history = self.response_history.lock().unwrap();
        
        // Create a normalized version of the current buffer (last 500 chars, trimmed)
        let normalized = Self::safe_suffix(current_buffer, 500).trim().to_string();
        
        // Skip if empty or very short
        if normalized.len() < 10 {
            history.push("EMPTY_RESPONSE".to_string());
        } else {
            history.push(normalized);
        }
        
        // Keep only last 3 responses
        if history.len() > 3 {
            history.remove(0);
        }
        
        // Check if we have 3 identical responses
        if history.len() >= 3 {
            if history[0] == history[1] && history[1] == history[2] {
                return true;
            }
            
            // Also check if all 3 are empty responses
            if history.iter().all(|h| h == "EMPTY_RESPONSE") {
                return true;
            }
        }
        
        false
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    if !args.md_file.exists() {
        anyhow::bail!("File '{}' not found", args.md_file.display());
    }

    let automator = Claudia::new(args.md_file);
    
    ctrlc::set_handler(move || {
        if std::io::stdin().is_terminal() {
            disable_raw_mode().ok();
        }
        println!("\n\nInterrupted by user. Exiting...");
        std::process::exit(0);
    }).expect("Error setting Ctrl-C handler");

    automator.run()?;
    
    Ok(())
}