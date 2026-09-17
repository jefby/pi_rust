# Agent 流程分析（从提示词到结果输出）

基于 `pi-agent/src/agent_loop.rs` 和 `pi-coding-agent/src/interactive.rs` 的实际代码追踪。

---

## 1. 整体流程图

```
用户输入提示词 (stdin)
       ↓
[interactive.rs] 构建 AgentConfig + 加载 Session 历史
       ↓
[session.rs] session.push_message(Message::user_text(prompt))
       ↓
[agent_loop.rs] run_agent_with_history(config, history, Some(tx))
       ↓
构建 Context (system_prompt + messages + tool_defs)
       ↓
stream_simple(&model, &ctx, &options) → 异步流式响应
       ↓
流式事件处理循环 (AgentEvent)
       ↓
输出到 stdout/stderr (TextDelta、ThinkingDelta、ToolExecutionStart...)
       ↓
如果 AssistantMessage 包含 ToolCall：
  → 权限检查 (permission.check)
  → 执行工具 (tool.execute)
  → 生成 ToolResultMessage
  → 追加到 messages
  → 继续下一轮 (while turn < max_turns)
       ↓
如果没有 ToolCall 或 StopReason != ToolUse：
  → break 'outer (退出循环)
       ↓
保存结果到 Session (session.append_messages + save)
```

---

## 2. 逐步分解

### 步骤 A：输入接收 (`interactive.rs` loop)

```rust
let mut line = String::new();
stdin.lock().read_line(&mut line)?;
let prompt = line.trim().to_string();
```

- 从 `stdin` 读取一行
- 如果是 `/` 开头，处理斜杠命令（`/tree`、`/compact`、`/fork` 等）
- 否则作为用户提示词继续

---

### 步骤 B：配置构建

```rust
let cfg = AgentConfig::new(app.model.clone(), system_prompt.clone())
    .with_tools(default_tools())
    .with_max_turns(app.max_turns)
    .with_thinking(app.thinking_level)
    .with_api_key(app.api_key.clone())
    .with_permission(permission.clone());
```

- `AgentConfig` 组合：模型、系统提示、工具列表、最大轮数、思考级别、API 密钥、权限策略

---

### 步骤 C：会话持久化（执行前）

```rust
session.push_message(Message::user_text(prompt));
crate::session::save(&app.config_dir, &mut session);
```

- 用户消息立即保存到磁盘，确保中断后仍可恢复
- `session.messages()` 提取完整历史作为输入

---

### 步骤 D：启动 Agent 运行 (`agent_loop.rs` `run_agent_with_history`)

```rust
pub async fn run_agent_with_history(
    config: &AgentConfig,
    mut messages: Vec<Message>,
    events: Option<mpsc::UnboundedSender<AgentEvent>>,
) -> Result<AgentRun> {
```

核心参数：
- `messages`: 当前会话历史（包含刚推入的用户提示）
- `events`: `mpsc::UnboundedSender` 用于流式事件传递
- `AgentConfig`: 包含模型、工具、系统提示等

---

### 步骤 E：构建流式上下文 (`Context`)

```rust
let ctx = Context {
    system_prompt: Some(config.system_prompt.clone()),
    messages: messages.clone(),
    tools: tool_defs.clone(),
};

let mut options = config.stream_options.clone();
if options.reasoning.is_none() && config.thinking_level != ThinkingLevel::Off {
    options.reasoning = Some(config.thinking_level);
}
```

- `system_prompt` 由 `build_system_prompt(app)` 生成（包含基础提示 + 项目上下文文件 + 附加提示）
- `messages` 是完整对话历史
- `tool_defs` 转换自 `AgentTool` trait 对象

---

### 步骤 F：调用 LLM 流 (`stream_simple`)

```rust
let mut stream = stream_simple(&config.model, &ctx, &options).await?;
```

- `stream_simple` 来自 `pi-ai`，根据 `model.api` 选择提供商（Anthropic、OpenAI、DeepSeek 等）
- 返回异步 `Stream`，逐块传递 `AssistantMessageEvent`

---

### 步骤 G：流式事件处理循环

```rust
while let Some(ev) = stream.next().await {
    let ev = ev?;
    match ev {
        AssistantMessageEvent::Done { reason, message } => {
            stop = reason;
            final_message = Some(message);
            break;  // 流结束
        }
        AssistantMessageEvent::TextDelta { delta, .. } => {
            emit(&events, AgentEvent::TextDelta { delta });
        }
        AssistantMessageEvent::ThinkingDelta { delta, .. } => {
            emit(&events, AgentEvent::ThinkingDelta { delta });
        }
        AssistantMessageEvent::Error { reason, error } => {
            return Err(AgentError::Other(err_msg));
        }
        _ => {}
    }
}
```

流式事件类型：
- `TextDelta`: 助手文本增量 → 输出到 `stdout`
- `ThinkingDelta`: 思考过程增量 → 输出到 `stdout`
- `Done`: 流式结束，包含完整 `AssistantMessage` 和 `StopReason`
- `Error`: 提供商错误 → 直接返回错误

---

### 步骤 H：处理完整助手消息

```rust
let Some(msg) = final_message else { ... };
let assistant_message = Message::Assistant(msg.clone());
messages.push(assistant_message.clone());
```

- 将完整助手消息追加到历史
- 发送 `AgentEvent::AssistantMessage` 事件

---

### 步骤 I：提取工具调用

```rust
let tool_calls: Vec<(String, String, Value)> = msg
    .content
    .iter()
    .filter_map(|c| match c {
        Content::ToolCall { id, name, arguments } => Some((id.clone(), name.clone(), arguments.clone())),
        _ => None,
    })
    .collect();
```

- 从 `msg.content` 提取所有 `ToolCall` 项

---

### 步骤 J：判断是否需要继续

```rust
if tool_calls.is_empty() || stop != StopReason::ToolUse {
    emit(&events, AgentEvent::TurnEnd);
    break 'outer;  // 退出外层循环
}
```

- **无工具调用** 或 **停止原因不是 `ToolUse`** → 结束整个 Agent 运行
- 否则进入工具执行阶段

---

### 步骤 K：工具执行（循环处理每个工具调用）

```rust
for (id, name, args) in tool_calls {
    // 1. 权限检查
    let tool_obj = tool_index.get(&name);
    let needs_perm = ... && !session_allowed.contains(&name);
    if needs_perm {
        match config.permission.check(&name, &args).await {
            PermissionDecision::Allow => {},
            PermissionDecision::AllowSession => { session_allowed.insert(name.clone()); },
            PermissionDecision::Deny { reason } => {
                emit(AgentEvent::PermissionDenied { ... });
                // 生成拒绝结果的 ToolResultMessage
                messages.push(Message::ToolResult(tr));
                continue;  // 跳过执行，继续处理下一个工具
            }
        }
    }

    // 2. 执行工具
    emit(AgentEvent::ToolExecutionStart { ... });
    let (content, is_error, terminate) = match tool_obj {
        Some(tool) => match tool.execute(&id, args).await {
            Ok(AgentToolResult { content, details: _, terminate }) => (content, false, terminate),
            Err(e) => (vec![Content::text(...)], true, false),
        },
        None => (vec![Content::text("unknown tool...")], true, false),
    };

    // 3. 发送执行结束事件
    emit(AgentEvent::ToolExecutionEnd { ... });

    // 4. 生成 ToolResultMessage 并追加到历史
    let tr = ToolResultMessage { ... };
    messages.push(Message::ToolResult(tr));
}
```

工具执行流程：
1. 检查是否需要权限（根据 `AgentTool::requires_permission()` 和会话允许列表）
2. 调用 `permission.check(name, args).await` → `Allow` / `AllowSession` / `Deny`
3. 执行 `tool.execute(id, args).await`
4. 生成 `AgentToolResult`（内容 + 是否错误 + 是否终止）
5. 追加 `Message::ToolResult` 到历史

---

### 步骤 L：判断是否终止整个运行

```rust
if any_terminate {
    break;
}
emit(&events, AgentEvent::TurnEnd);
```

- 如果任何工具设置了 `terminate = true`，则 `any_terminate` 变为 `false`（因为 `!terminate` 条件），实际上 `any_terminate` 逻辑是：如果所有工具都没有设置 `terminate = true`，则继续循环；如果有 `terminate`，则退出外层循环
- 实际行为：如果工具请求终止，则退出 Agent 运行

---

### 步骤 M：轮数限制检查

```rust
while turn < config.max_turns {
    turn += 1;
    ...
}

if turn >= config.max_turns {
    stopped_at_turn_limit = true;
}
```

- 每轮增加 `turn`
- 如果达到最大轮数，设置 `stopped_at_turn_limit = true`

---

### 步骤 N：发送结束事件

```rust
emit(&events, AgentEvent::AgentEnd { messages: messages.clone() });
```

- 通知监听者 Agent 运行已结束，并传递完整消息历史

---

### 步骤 O：返回结果

```rust
Ok(AgentRun {
    messages,
    stopped_at_turn_limit,
})
```

- `AgentRun` 包含完整的对话历史（包括用户提示、助手回复、工具结果）
- `stopped_at_turn_limit` 表示是否因为轮数限制而停止

---

### 步骤 P：结果保存到 Session（`interactive.rs` 后续处理）

```rust
if let Some(new_messages) = res.messages.get(branch_len..) {
    session.append_messages(new_messages);
}
if let Err(e) = crate::session::save(&app.config_dir, &mut session) {
    eprintln!("(warning: session save failed: {e})");
}
```

- 提取新增消息（从原历史长度开始）
- 追加到 `session`
- 保存到磁盘

---

## 3. 事件流总结（`AgentEvent` 类型）

| 事件 | 触发时机 | 输出位置 |
|------|----------|----------|
| `UserMessage` | 开始时，发送用户提示 | 无（仅记录） |
| `AgentStart` | Agent 运行开始 | 无 |
| `TurnStart` | 每轮开始 | 无 |
| `TextDelta` | 流式文本增量 | `stdout` |
| `ThinkingDelta` | 流式思考增量 | `stdout` |
| `AssistantMessage` | 完整助手消息生成 | `stdout` 换行 |
| `ToolExecutionStart` | 工具执行开始 | `stderr`（`→ name(args)`） |
| `ToolExecutionEnd` | 工具执行结束 | `stderr`（`← name ok/error`） |
| `PermissionDenied` | 权限被拒绝 | `stderr`（`✗ name denied: reason`） |
| `TurnEnd` | 本轮结束 | 无 |
| `AgentEnd` | 整个 Agent 运行结束 | 无 |

---

## 4. 流式响应的关键点

1. **异步流式**：`stream_simple` 返回的是一个 `futures::Stream`，逐块传递，不是一次性返回完整响应
2. **实时输出**：`TextDelta` 事件立即写入 `stdout`，用户可以看到逐字生成的内容
3. **中断安全**：用户提示在执行前已保存到磁盘（`session.save`），即使中断也可恢复
4. **工具结果循环**：工具执行结果作为 `Message::ToolResult` 追加到历史，然后继续下一轮（如果没有终止标志）

---

## 5. 从提示词到结果的完整时间线

```
T0: 用户输入 "写一个 hello.py"
T1: session.push_message(user_text) + save()
T2: run_agent_with_history() 启动
T3: 构建 Context (system_prompt + messages + tools)
T4: stream_simple() 开始流式响应
T5: 流式事件：TextDelta("我可以帮你写...")
T6: 流式事件：TextDelta("首先...")
T7: 流式事件：Done { stop=ToolUse, message=AssistantMessage { content=[ToolCall { name="write", args={path:"hello.py", content:"print('hello')"}}] }}
T8: 提取 ToolCall (name="write", args=...)
T9: 权限检查（如果 write 需要权限）
T10: 执行 tool.execute(id, args)
T11: 生成 ToolResultMessage
T12: messages.push(ToolResult)
T13: 检查 any_terminate → false（没有终止标志），继续循环
T14: 下一轮 turn += 1
T15: 再次调用 stream_simple()（包含用户提示 + 工具结果）
T16: 流式响应完成，没有新的 ToolCall → break 'outer
T17: AgentEnd 事件
T18: session.append_messages(new_messages) + save()
T19: 交互式循环继续，等待下一个用户输入
```

---

## 6. 关键代码位置索引

| 文件 | 行数范围 | 功能 |
|------|----------|------|
| `agent_loop.rs` | `run_agent_with_history` | 主 Agent 循环 |
| `agent_loop.rs` | `stream.next()` 循环 | 流式事件处理 |
| `agent_loop.rs` | `tool_calls` 提取 | 工具调用解析 |
| `agent_loop.rs` | 权限检查 + 执行 | 工具执行与权限控制 |
| `interactive.rs` | 主 `loop` | 用户输入处理 |
| `interactive.rs` | `AgentConfig` 构建 | 配置组装 |
| `interactive.rs` | `rx.recv()` 循环 | 流式输出到终端 |
| `interactive.rs` | `session.append_messages` | 结果保存 |
