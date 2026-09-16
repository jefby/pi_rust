# Pi Rust Codebase Architecture Analysis

## Overview

This is a Rust-based coding agent (similar to @earendil-works/pi) that provides:

- **Interactive CLI** (`pi` command) for conversational coding
- **Full-screen TUI** for branch switching and session management
- **Print mode** for one-shot prompt execution
- **Multiple crates** with clear separation of concerns

## Crate Structure

### 1. `pi-ai` (Core AI logic)
- **Providers**: Abstracts different LLM providers (OpenAI, Anthropic, DeepSeek, etc.)
- **Error handling**: Centralized error types with `thiserror` and `anyhow`
- **Retry logic**: Exponential backoff with `RetryConfig` and `with_retry`
- **Streaming**: `AssistantMessageEventStream` for streaming responses
- **Session management**: Tree-structured session persistence with JSON files

### 2. `pi-agent` (CLI and permission system)
- **Permission system**: 
  - `CliPermission` for interactive CLI permission prompts
  - `TuiPermission` for TUI permission dialogs
  - Permission policies (`Yolo`, `Interactive`, `DenyAll`)
- **Tools system**: 
  - `default_tools()` returns builtin tools (read, write, edit, bash, ls, grep, etc.)
  - Tool abstraction via `AgentTool` trait
- **Session persistence**: 
  - Tree-structured sessions stored as JSON under `$XDG_CONFIG_HOME/pi/sessions/`
  - Legacy format support for pre-tree sessions
  - Import/export with upstream JSONL format

### 3. `pi-coding-agent` (Main application)
- **CLI parsing**: `clap` based command line interface with many options
- **Session management**: 
  - `Session` struct with tree structure (nodes with `id`, `parent_id`)
  - Branch switching (`branch_to`, `branch`, `abandoned`)
  - Session tree visualization (`tree_items`)
- **Interactive mode**: 
  - REPL that reads stdin, runs agent turns, streams output
  - Slash command handling (`/tree`, `/fork`, `/compact`, etc.)
- **TUI (feature flag)**: 
  - Full-screen terminal UI with branch switching, session tree, and input
  - Uses `crossterm` and `ratatui` for UI rendering
  - Permission prompts render as modal dialogs

## Key Components

### Session Structure (pi-coding-agent/src/session.rs)
- **Entry**: Node in session tree with `id`, `parent_id`, `timestamp`, `message`, `raw`
- **Session**: Contains `entries`, `active_leaf`, model info, timestamps
- **Operations**: 
  - `push_message()`: Append message as child of active leaf
  - `replace_messages()`: Replace all messages (used by `/compact`, `/reset`)
  - `branch_to()`: Get path from root to a node
  - `tree_items()`: Generate tree view for `/tree` overlay

### Permission System (pi-agent/src/permission.rs)
- **CLI**: Interactive prompts with `y`/`a`/`n` responses
- **TUI**: Modal dialogs using oneshot channels
- **Policies**: 
  - `Yolo`: Always allow
  - `Interactive`: Prompt user
  - `DenyAll`: Always deny
- **Session tracking**: `CliPermission` maintains allowed sessions for `--allow-session`

### TUI (pi-coding-agent/src/tui.rs)
- **Layout**: 
  - Title bar with session info
  - Conversation pane (scrollable)
  - Input box at bottom
  - Status line at bottom
- **Features**:
  - Branch switching (`/tree`)
  - Session tree visualization
  - Permission prompts as modals
  - Tool execution inline
- **Event flow**: UI sends `UiEvent` to background agent tasks

### Print Mode (pi-coding-agent/src/print_mode.rs)
- One-shot execution of a prompt
- Two output modes:
  - **Human**: Text streams to stdout, tool activity to stderr
  - **JSON-lines**: Structured JSON events for scripting
- **System prompt**: 
  - Default base prompt + project context files
  - Support for `--system-prompt` override
  - `--append-system-prompt` for additional context

### Configuration (pi-coding-agent/src/config.rs & file_config.rs)
- **FileConfig**: TOML config at `$XDG_CONFIG_HOME/pi/config.toml`
  - Optional fields: `model`, `max_turns`, `thinking_level`, `yolo`, `json`, `branch_summary`
- **AgentHome**: Reads upstream `~/.pi/agent` config
  - `settings.json`: default provider/model/level
  - `auth.json`: per-provider API keys
  - `models.json`: custom providers and models
  - `models-store.json`: provider model catalogs
- **Model resolution**: 
  - CLI flags > env vars > config.toml > upstream config > env fallback
  - `resolve_model()` handles all precedence layers

## Architecture Flow

1. **Startup**:
   - Load config files (`FileConfig`, `AgentHome`)
   - Resolve model/provider with precedence: CLI > env > config > upstream
   - Set up permission policy (`CliPermission` or `TuiPermission`)

2. **Session Handling**:
   - Load or create new session
   - Handle `--resume`, `--fork`, `--clone` commands
   - Persist session after each turn

3. **Interactive Mode**:
   - REPL reads user input
   - Dispatches slash commands or agent turns
   - Streams agent output to stdout/stderr
   - Handles permission prompts and tool execution

4. **TUI Mode**:
   - Full-screen terminal UI
   - Renders session tree, conversation, input
   - Handles branch switching and permission dialogs
   - Sends events to background agent tasks

5. **Print Mode**:
   - One-shot execution of a prompt
   - Streams output in human or JSON format
   - Handles tool execution and permission checks

## Design Principles

1. **Minimalism**: 
   - No unnecessary abstractions
   - Prefer stdlib/native features over dependencies
   - One-line solutions when possible

2. **Deletion over Addition**: 
   - Prefer removing code over adding new files/abstractions
   - Delete reinvented stdlib functionality

3. **Single Responsibility**: 
   - Each crate has clear boundaries
   - Tools, permissions, sessions, and UI are separate concerns

4. **Persistence**:
   - Sessions are tree-structured JSON files
   - Upstream compatibility with JSONL format
   - Legacy format migration support

5. **Extensibility**:
   - Provider abstraction allows adding new LLM providers
   - Tool system is modular and extensible
   - TUI is optional feature flag

## Notable Implementation Details

### Session Tree Structure
- Nodes are stored in a flat Vec with parent-child relationships
- Active leaf is tracked separately for branch switching
- Tree visualization uses depth-first traversal with stack-based algorithm

### Permission System
- Separate policies for CLI (stdin) and TUI (modal dialogs)
- Session tracking for `--allow-session` functionality
- `PermissionDecision` enum handles allow/deny outcomes

### TUI Architecture
- Background tasks run agent logic in separate threads/channels
- UI receives events via `mpsc` channels
- Input handling is non-blocking with async/await
- Event loop processes messages and renders UI

### Error Handling
- Centralized error types with `thiserror` and `anyhow`
- Retry logic with exponential backoff and jitter
- Permission decisions are explicit and auditable

## Summary

This codebase implements a modern, flexible coding agent with:

- **Clear separation** of concerns across multiple crates
- **Robust permission system** with CLI and TUI support
- **Tree-structured session management** with import/export
- **Flexible output modes** (human and JSON)
- **Feature flags** for optional functionality (TUI)
- **Production-ready** error handling and retry logic

The architecture follows modern Rust best practices with:
- Strong typing and error handling
- Minimal dependencies
- Clear module boundaries
- Test coverage for critical components
- Active development with ongoing feature additions

The design prioritizes:
- **Simplicity** over clever abstractions
- **Maintainability** through clear separation
- **Performance** with streaming and async I/O
- **User experience** with intuitive CLI/TUI interactions

This architecture enables the agent to handle complex workflows while remaining minimal and focused on the core task of assisting with code development.