# Claudia

A fast, standalone Rust executable that acts as a wrapper and allows Claude Code to be run 24/7 while you are sleeping. It automates Claude interactions by passing it a Markdown file with tasks to complete and executes all tasks continuously until all tasks have been completed, irresepective of the number of tasks specified.

## Features

- 🚀 Fast native executable - no runtime dependencies
- 📝 Passes Markdown files directly to Claude for task completion
- 🔄 Automatically spawns Claude with `--dangerously-skip-permissions` flag
- ⏸️ Auto-continues when Claude stops (with smart loop detection)
- ⏰ Detects usage limits and waits with visible countdown
- ✅ Auto-adds checkboxes to tasks and tracks completion
- 🛑 Handles Ctrl+C interruption gracefully
- 🖥️ Interactive terminal support with arrow keys and user input passthrough
- 📊 Displays session statistics and status updates

## Installation

### From Source

1. Install Rust if you haven't already:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Clone and build the project:
```bash
cargo build --release
```

3. The executable will be at `target/release/claudia`

4. Install it system-wide:
```bash
sudo cp target/release/claudia /usr/local/bin/
# Or for user-only installation:
cp target/release/claudia ~/.local/bin/
```

### Quick Install

```bash
./install.sh
```

## Usage

```bash
claudia <path_to_markdown_file>
```

Example:
```bash
claudia tasks.md
```

With debug output:
```bash
claudia --debug tasks.md
```

## Usage Limit Handling

When Claude reaches its usage limit, Claudia will:
1. Display a prominent message (that stays visible on screen)
2. Show a countdown timer updating every 30 seconds
3. Automatically resume the session when the limit resets
4. Continue from where it left off without losing progress

## How It Works

1. **Pre-processing**: Automatically adds checkboxes ([ ]) to any list items that don't have them
2. **Launch**: Spawns Claude with the `--dangerously-skip-permissions` flag in the file's directory
3. **Pass File**: Instructs Claude to read and complete all tasks, marking them with [x] when done
4. **Monitor**: Watches Claude's output for:
   - Signs that Claude has stopped (to send "Continue")
   - Usage limit messages (waits with countdown timer)
   - Task completion (all checkboxes marked)
   - Repeated patterns (prevents infinite loops)
5. **Interactive**: Passes through user keyboard input to Claude
6. **Auto-Continue**: Intelligently sends "Continue" when needed (max 50 times)
7. **Exit**: Terminates when all tasks are marked complete or on error

## Example Markdown File

```markdown
# Tasks for Claude

- Create a Python web scraper for news articles
- Add error handling and retry logic
- Write unit tests for the scraper
- Create documentation
- Add CLI arguments for configuration
```

Claudia will automatically add checkboxes to these tasks:

```markdown
# Tasks for Claude

- [ ] Create a Python web scraper for news articles
- [ ] Add error handling and retry logic
- [ ] Write unit tests for the scraper
- [ ] Create documentation
- [ ] Add CLI arguments for configuration
```

Claude will then work through the tasks autonomously, marking each with [x] as completed.

## Completion Detection

Claudia automatically detects completion by checking if all checkboxes in the markdown file are marked as complete ([x] or [X]). This is more reliable than looking for specific phrases in Claude's output.

Additionally, Claudia includes safety features:
- Detects and prevents infinite loops when Claude gets stuck
- Limits Continue commands to 50 to prevent runaway sessions
- Monitors for repeated output patterns

## Command Line Options

```bash
claudia [OPTIONS] <MD_FILE>
```

Options:
- `-d, --debug`: Enable debug mode to see additional diagnostic output
- `-h, --help`: Print help information
- `-V, --version`: Print version information

## Building for Distribution

To create an optimized binary:

```bash
cargo build --release
# Note: On macOS, avoid using strip as it can corrupt ARM64 binaries
```

The release build is optimized for size with LTO enabled.

## Requirements

- Claude CLI must be installed and accessible in PATH
- Unix-like system (Linux, macOS)
- Rust 1.70+ (for building from source)

## License

MIT