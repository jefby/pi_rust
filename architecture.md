# Pi Rust 代码架构分析

## 概述

这是基于 Rust 的编码助手（类似于 `@earendil-works/pi`），提供以下功能：

- **交互式命令行** (`pi` 命令)：用于对话式编程辅助
- **全屏 TUI**（终端界面）：支持分支切换、会话管理、富文本输入
- **打印模式**：单次执行提示词，支持人类输出或 JSON 输出
- **多提供商 LLM 支持**：OpenAI、Anthropic、DeepSeek、Groq、Mistral、Qwen 等
- **丰富工具集**：读取、写入、编辑、列出目录、待办事项等
- **会话持久化**：带有树状结构的对话历史，可导入/导出

## 包结构

```
pi-rust/
├── pi-ai/                  # 核心 AI 逻辑（提供商、错误处理、重试、会话持久化）
│   ├── providers/          # 内置提供商（OpenAI、Anthropic、DeepSeek、Gemini、Groq、Mistral、Qwen 等）
│   ├── error/              # 分层错误类型（ModelError、ProviderError、RetryError、PermissionError 等）
│   └── session/           # 会话管理（树状对话历史）
└── pi-coding-agent/       # 主应用（命令行 + TUI + 打印模式）
    ├── config/            # 配置加载（TOML、环境变量、上游配置）
    ├── agent/             # 命令行接口（斜杠命令、权限控制、会话管理）
    ├── session/           # 会话状态机（树状会话、分支切换）
    ├── tui/               # 全屏终端 UI（分支选择器、会话树、输入框）
    ├── print/             # 单次执行（人类模式 / JSON 模式）
    └── tools/             # 内置工具（read、write、edit、ls、todo 等）
```

## 核心架构模式

### 1. 会话管理（pi-ai/session.rs）

应用的心脏是 **Session** —— 一个树状对话状态。

- **结构**：每个会话是一个节点树（id、父节点、时间戳、消息、原文）
- **活动叶子**：跟踪当前活跃分支（用于 `/tree`、`/fork` 等操作）
- **持久化**：保存到 `$XDG_CONFIG_HOME/pi/sessions/` 下的 JSON 文件；也支持 JSONL 导出以兼容上游
- **操作**:
  - `push_message()`：追加一条消息（如果不存在则创建新叶子）
  - `replace_messages()`：替换所有消息（用于 `/fork`）
  - `branch_to(id)`：跳转到特定节点
  - `abandoned()`：提取被放弃的分支内容
  - `compact()`：总结旧消息

### 2. 权限系统（pi-agent/permission.rs）

两种不同的权限模型：

- **`CliPermission`**（命令行）：通过标准输入交互式询问用户
  - `Yolo`：总是允许
  - `Interactive`：询问用户（y/a/n）
  - `DenyAll`：永远拒绝所有操作

- **`TuiPermission`**（TUI）：模态对话框用于权限决策
  - 当工具需要交互时显示
  - 支持 `y`/`a`/`n` 响应

权限控制工具执行和某些动作（如 `/fork`、`/compact`）

### 3. 工具系统（pi-agent/tools/）

轻量级异步工具，通过 `AgentTool`  trait 实现：

| 工具 | 描述 |
|------|------|
| `read` | 读取文件内容（可选偏移量/限制） |
| `write` | 写入文件内容（覆盖已存在） |
| `edit` | 文件中查找并替换一次/多次 |
| `ls` | 列出目录内容（文件/目录/符号链接） |
| `todo` | 任务列表（添加、完成、列出、清空） |

每个工具都要求权限（除 `Yolo` 模式外），执行异步文件 I/O，并返回结构化的 `AgentToolResult`（成功/失败 + 输出）。

### 4. TUI（pi-coding-agent/tui.rs）

基于 `ratatui` 的全屏终端 UI

- **状态机**：`State` 追踪输入缓冲区、光标位置、滚动、分支长度等
- **键盘处理**：键盘导航（箭头键、Enter、Backspace、Home/End、PageUp/Down、Ctrl+K/J）同时支持编辑器和树视图
- **分支控制**：
  - `/tree` —— 切换到其他分支
  - `/fork` —— 从当前分支克隆新会话
  - `/compact` —— 压缩旧消息
  - `/new` —— 开始新会话
- **树覆盖层**：可视化会话树，支持展开/折叠/过滤
- **实时反馈**：作为代理逐步流式传输的响应

### 5. 打印模式（pi-coding-agent/print_mode.rs）

单次执行的轻量级执行

- **人类模式**：直接将助手文本流式输出到标准输出
- **JSON 模式**：结构化 JSON 事件（适用于脚本自动化）

两者在执行前都会保存会话状态，并在完成后保存更新后的状态。

## 数据流

```
[命令行/TUI] → [AgentConfig] → [Session] → [工具/模型]
                     ↑                              │
                     └───────────────────────┘
                          ↓
                   [输出] → [显示]（控制台/TUI）
```

1. **初始化**：加载配置（TOML + 环境变量 + 上游 `~/.pi/agent`）
2. **命令处理**:
   - 斜杠命令（`/tree`、`/fork`、`/compact`、`/new`、`/reset`、`/help`）
   - 打印模式（`/print`、`--print`）
   - 直接工具调用（`read <path>`、`write <path> <content>`、`edit <path> <old> <new>`、`ls <path>`、`todo <action>`）
3. **会话管理**:
   - 新会话 → 空树
   - `/fork` → 将当前分支复制到新会话 ID
   - `/compact` → 总结旧消息
   - `/new` → 开始新会话
4. **工具执行**:
   - 工具被调用 → 异步文件 I/O → 返回结果
5. **持久化**:
   - 每次回合保存会话
   - 导出到 JSONL 供外部消费者使用

## 关键设计原则

| 原则 | 实现 |
|------|------|
| **极简主义** | 不添加不必要的抽象；每个 crate 负责单一职责 |
| **声明式配置** | TOML 配置 + 环境变量 + 上游 `~/.pi/agent` |
| **声明式权限** | 启动时选择权限模式（`--yolo`、`--interactive`、`--deny-all`） |
| **持久化优先** | 所有状态（会话、配置）均保存到磁盘；会话在崩溃后恢复 |
| **分离关注点** | `pi-ai` 处理 AI 逻辑；`pi-coding-agent` 处理 CLI/TUI；`pi-agent` 处理工具 |
| **特性标志** | TUI 是可选功能（`--tui` 标志）；其他功能始终开启 |

## 组件细分

### pi-ai（核心 AI）
- **提供商**：OpenAI、Anthropic、DeepSeek、Google Gemini、Groq、Mistral、Qwen 等
- **错误处理**：分层错误（`ModelError`、`ProviderError`、`RetryError`、`PermissionError`）
- **重试逻辑**：针对瞬时故障的指数退避 + 抖动
- **会话持久化**：树状 JSON 存储，支持 JSONL 导出

### pi-coding-agent（应用层）
- **斜杠命令**：`/tree`、`/fork`、`/compact`、`/new`、`/reset`、`/help`
- **权限系统**：CLI 与 TUI 权限模式
- **会话管理**：在 CLI 和 TUI 之间推送/拉取会话状态
- **工具集成**：委托给 `pi-agent/tools/*` 实现

### pi-coding-agent（应用程序）
- **TUI**：基于 `ratatui` 的全屏 UI
- **打印模式**：单次执行的人类/JSON 输出
- **配置**：从 TOML 加载配置
- **集成**：将 CLI、TUI 和打印模式连接起来

## 文件-功能对应表

| 文件 | 角色 |
|------|-------|
| `pi-ai/session.rs` | 会话树、持久化、分支切换 |
| `pi-ai/error.rs` | 分层错误类型 |
| `pi-ai/retry.rs` | 指数退避重试 |
| `pi-coding-agent/config.rs` | TOML + 环境变量 + 上游配置合并 |
| `pi-coding-agent/agent_config.rs` | 应用配置（模型、提供商、最大轮数、思考深度） |
| `pi-coding-agent/session.rs` | 会话状态机（State 结构） |
| `pi-coding-agent/tui.rs` | 全屏终端 UI |
| `pi-coding-agent/print_mode.rs` | 单次执行 |
| `pi-coding-agent/tools/` | read、write、edit、ls、todo 工具 |
| `pi-coding-agent/permission.rs` | CLI/TUI 权限系统 |

## 使用流程

### 交互式命令行
```bash
pi --model deepseek-v4-spanish --yolo  # --yolo 表示总是允许
pi --model deepseek-v4-spanish /help  # 显示 /help、/tree、/fork、/compact、/new 等命令
pi /tree                      # 切换到其他分支
pi /fork                      # 克隆当前分支为新会话
pi /compact                   # 总结旧消息
pi /new                       # 开始新会话
pi /reset                     # 重置到最新会话
pi /print                     # 单次执行
```

### TUI
```
$ pi --tui                    # 启动全屏 UI
# 使用箭头键导航，按 /tree、/fork、/compact、/new
# 输入 /print 执行单次命令
```

### 打印模式
```bash
pi --print                     # 人类输出
pi --print --json              # JSON 事件（供自动化使用）
```

## 总结

架构层次分明：

1. **pi-ai** — AI 引擎（提供商、错误处理、会话持久化）
2. **pi-coding-agent** — 应用层（CLI、TUI、打印模式）
3. **pi-agent** — 工具系统（读、写、编辑、列出、待办）

设计遵循**最小化**、**声明式配置**、**持久化优先**、**关注点分离** 四大原则。TUI 是最复杂的组件（包含键盘导航、实时反馈、分支管理），但通过 `--tui` 标志可独立运行。打印模式为脚本提供轻量级替代方案。两者均与底层 AI 引擎无缝集成。

*架构文档已从 pi-rust 仓库生成。*
