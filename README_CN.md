<div align="center">

<img src="https://pub-806a1b8456464ce7a6c110f84946697e.r2.dev/documents/v0.1/banner.webp" width="100%" alt="VibeAround — 在任意 IM 里和你的 AI 编程 Agent 对话" />

# VibeAround

**在任意 IM 里和你的 AI 编程 Agent 对话 —— 无需订阅官方会员方案。**

[English](README.md) | [简体中文](README_CN.md) | [Wiki](https://github.com/jazzenchen/VibeAround/wiki)

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.82+-000?style=flat-square&logo=rust&logoColor=fff" alt="Rust" />
  <img src="https://img.shields.io/badge/Tauri-2.10-24C8DB?style=flat-square&logo=tauri&logoColor=fff" alt="Tauri" />
  <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=000" alt="React" />
  <img src="https://img.shields.io/badge/ACP-Rust_SDK-000?style=flat-square" alt="ACP" />
  <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License: MIT" />
</p>

</div>

VibeAround 是一个把主流 AI 编程 Agent —— **Claude Code、Codex CLI、Cursor CLI、Gemini CLI、Kiro CLI、Qwen Code、OpenCode** —— 接入日常 IM 工具的友好桥梁：**Telegram、飞书、Discord、Slack、微信、钉钉、企业微信、QQ Bot**。无需任何官方付费订阅，配置第三方 API Key 即可使用。

在 Mac 上用 Claude Code 开始一个任务，通过 `/handover` 一键移交到手机上的 Telegram 继续对话，保留完整上下文，回到桌前再移交回终端。在任意 IM 里用 `/switch codex` 中途切换 Agent。可以同时让 Claude 在 Telegram 上跑、让 Codex 在 Slack 上跑，互不干扰。

Tauri 打包的桌面应用提供一键安装引导，无需复杂配置。每个 IM 渠道和每个 Agent 都是可下载的插件，核心保持小而干净。内置的 Web 控制台监听 `localhost:12358`，提供完整的 PTY + tmux Web 终端；内置的隧道（Cloudflare Tunnel / Ngrok / Localtunnel）让你随时从手机访问。

## 演示视频

[![VibeAround 演示视频 —— 会话接力、Agent 切换、多渠道并发](https://img.youtube.com/vi/6kxNKTMz-AM/maxresdefault.jpg)](https://youtu.be/6kxNKTMz-AM)

*观看 VibeAround 实际运行 —— 终端与 IM 之间的会话接力、会话中切换 Agent、多渠道并发。*

## 核心功能

- **会话接力** — 将任意 agent 的编程会话一键移交到任意 IM 频道，手机上继续对话
- **Agent 切换** — 在任何频道中 `/switch claude`、`/switch codex`、`/switch cursor` 随时切换
- **IM 频道** — Telegram、飞书、Discord、Slack、微信、钉钉、企业微信、QQ Bot — 每个都是独立插件
- **原生 IM 体验** — 飞书交互卡片、Slack Block Kit、Discord embeds、企业微信 Markdown 流式输出 —— 每个渠道都使用其最丰富的原生格式
- **实时预览** — Agent 可以分享开发服务器的实时预览链接（iframe + 反向代理）或 Markdown 文件（GitHub 风格渲染），通过手机或浏览器查看，链接 5 分钟后自动过期
- **Skills + MCP 自动集成** — 每次启动时，VibeAround 都会把自己的 skill 集合和 MCP 服务端点同步写入每个已启用的编程 Agent（Claude Code、Codex、Gemini、Cursor、Kiro、Qwen），无需手动配置即可让它们将 VibeAround 作为工具调用
- **本地 Bearer Token 鉴权** — 守护进程的 HTTP、WebSocket、MCP 端点统一通过 `~/.vibearound/auth.json`（0600，每次启动轮换）中的会话 token 鉴权
- **网页终端** — 浏览器内完整 PTY 终端，集成 tmux，shell 会话与 agent 对话并行运行
- **网页控制台** — 终端、tmux、agent 对话，访问 `localhost:12358`
- **桌面应用** — 引导向导（含安装进度）、服务监控、工作空间管理、系统托盘
- **多工作空间** — 管理项目目录、设置默认、切换上下文
- **隧道访问** — 通过 Cloudflare Tunnel、Ngrok 或 Localtunnel 远程访问

## 支持的 Agents

所有 agent 通过 [ACP (Agent Client Protocol)](https://agentclientprotocol.com/) 经由 stdio 通信。基于 npm 的 agent 首次使用时自动安装。CLI 类 agent（Cursor、Kiro、Qwen、OpenCode）需用户自行安装。

| Agent | ACP | 会话接力 |
|---|---|---|
| **Claude Code** | 可用 | 已支持 |
| **Gemini CLI** | 可用 | 已支持 |
| **Codex CLI** | 可用 | 已支持 |
| **Cursor CLI** | 可用 | 已支持 |
| **Kiro CLI** | 可用 | 已支持 |
| **Qwen Code** | 可用 | 已支持 |
| **OpenCode** | 可用 | 不支持 |

## 频道插件

每个频道都是独立的 Node.js 插件，基于 [@vibearound/plugin-channel-sdk](https://www.npmjs.com/package/@vibearound/plugin-channel-sdk) 构建。

| 频道 | 认证方式 | 私聊 | 文件/图片 | 流式输出 | 斜杠命令 | 状态 |
|---|---|---|---|---|---|---|
| **Telegram** | Bot Token | 支持 | 支持 | 支持 | `/command` | 可用 |
| **飞书 / Lark** | 应用凭证 | 支持 | 支持 | 支持（卡片） | `/command` | 可用 |
| **Discord** | Bot Token | 支持 | 支持 | 支持 | `/command` | 可用 |
| **Slack** | Bot + App Token | 支持 | 支持 | 支持 | `/va`、`/vibearound` | 可用 |
| **微信** | 二维码登录 | 支持 | 支持 | 不支持 | `/command` | 可用 |
| **钉钉** | AppKey + Secret | 支持 | 支持 | 支持（Markdown） | `/command` | 可用 |
| **企业微信** | Bot ID + Secret | 支持 | 支持 | 支持（Markdown） | `/command` | 可用 |
| **QQ 频道** | App ID + Token | 支持 | 支持 | 不支持 | `/command` | 可用 |

## 命令

### 系统命令

| 命令 | 说明 |
|---|---|
| `/help` | 显示可用命令 |
| `/new` | 重置会话（新对话） |
| `/switch <agent>` | 切换 agent（claude、gemini、codex、cursor、kiro、qwen-code、opencode） |
| `/profile <name>` | 切换 profile |
| `/close` | 关闭对话 |
| `/pickup <code>` | 恢复一个编程 agent 会话 |
| `/handover` | 将会话导出到编程 agent CLI |

### Agent 命令

| 命令 | 说明 |
|---|---|
| `/agent <command>` | 向 agent 发送斜杠命令（如 `/agent status`） |

### Slack 专用

在 Slack 中，`/` 前缀会被客户端拦截。请使用 `/va` 或 `/vibearound` 代替：

| Slack 命令 | 等同于 |
|---|---|
| `/va help` | `/help` |
| `/va switch claude` | `/switch claude` |
| `/va agent status` | `/agent status` |
| `/va new` | `/new` |

## 环境要求

| 工具 | 版本 | 安装 |
|------|------|------|
| **Rust** | 1.82+ | [rustup.rs](https://rustup.rs/) |
| **Node.js** | 20+ | [nodejs.org](https://nodejs.org/) |
| **Bun** | 1.1+ | [bun.sh](https://bun.sh/) |
| **npm** | 10+ | 随 Node.js 一起安装 |

**平台支持：** 代码库支持 macOS、Linux 和 Windows。目前只打包了 macOS 的预编译版本 —— 因为我手边只有一台 Mac，Linux 和 Windows 用户暂时需要自行从源码构建。欢迎贡献 Linux / Windows 的 CI 与发布流程。

在 macOS 上需要安装 Xcode 命令行工具：

```bash
xcode-select --install
```

## 快速开始

```bash
cd src
bun install
bun run prebuild
bun run dev
```

1. 首次运行时桌面应用会打开引导向导
2. 选择 agents，配置频道和隧道
3. 网页控制台：`http://127.0.0.1:12358`
4. 通过终端、对话或 IM 频道开始编程

## 会话接力

将编程会话移交到任意已连接的 IM 频道 — 支持 Claude Code、Gemini CLI、Codex CLI、Cursor CLI、Kiro CLI 和 Qwen Code：

```
你 (终端)    > /handover
Agent       > 移交就绪，已复制到剪贴板：
               /pickup V5RX
               在任何已连接 VibeAround 的 IM 中粘贴。
               此代码 2 分钟内有效。
```

在 Telegram、飞书、Discord、Slack 或微信中粘贴 `/pickup` 命令 — 完整上下文继续对话。完成后再次 `/handover`，将会话移交回终端。

## 架构

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   桌面端    │  │  网页控制台  │  │  IM 频道    │
│  (Tauri)    │  │  Dashboard  │  │   插件      │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘
       │                │                │
       └────────────────┼────────────────┘
                        │
              ┌─────────┴─────────┐
              │   Rust 运行时     │
              │  ┌─────────────┐  │
              │  │  ACP Hub    │  │   ← 将 prompt 路由到 agent
              │  │ (按路由分配  │  │
              │  │   ACPPod)   │  │
              │  └──────┬──────┘  │
              │         │         │
              │  ┌──────┴──────┐  │
              │  │ Agent 工厂  │  │   ← 启动 Claude/Gemini/Codex/Cursor/Kiro/Qwen/OpenCode
              │  └─────────────┘  │
              │                   │
              │  ┌─────────────┐  │
              │  │ PTY 管理器  │  │   ← 终端会话 + tmux
              │  └─────────────┘  │
              └───────────────────┘
```

## 配置

所有配置位于 `~/.vibearound/settings.json`：

```json
{
  "default_agent": "claude",
  "enabled_agents": ["claude", "gemini", "opencode", "codex", "cursor", "kiro", "qwen-code"],
  "workspaces": ["/path/to/your/project"],
  "channels": {
    "telegram": { "bot_token": "..." },
    "feishu": { "app_id": "...", "app_secret": "..." },
    "discord": { "bot_token": "..." },
    "slack": { "bot_token": "xoxb-...", "app_token": "xapp-..." }
  },
  "tunnel": {
    "provider": "cloudflare",
    "cloudflare": { "tunnel_token": "...", "hostname": "..." }
  }
}
```

## 插件 SDK

使用 SDK 构建自己的频道插件：

```bash
npm install @vibearound/plugin-channel-sdk
```

详见 [SDK README](https://github.com/jazzenchen/vibearound-plugin-channel-sdk)。

## 文档

- [Wiki 首页](https://github.com/jazzenchen/VibeAround/wiki)
- [安装指南](https://github.com/jazzenchen/VibeAround/wiki/Setup-Guide)
- [频道插件](https://github.com/jazzenchen/VibeAround/wiki/Channel-Plugins)
- [架构](https://github.com/jazzenchen/VibeAround/wiki/Architecture)
- [配置模型](https://github.com/jazzenchen/VibeAround/wiki/Configuration-Model)
- [FAQ 和故障排除](https://github.com/jazzenchen/VibeAround/wiki/FAQ-and-Troubleshooting)

## 路线图

### 更多 IM 频道

| 频道 | 状态 |
|---|---|
| WhatsApp | 计划中 |
| LINE | 计划中 |
| Microsoft Teams | 计划中 |

### 实时预览（下一步）

- 在 IM 对话里直接查看 Agent 生成的文件、截图和产物（目前可通过隧道 URL 使用）
- IM 原生内嵌预览（图片卡片、嵌入式 iframe 等）

### 更多 IM 原生功能

- 更深度的渠道集成：表情回复、话题（threads）、交互式按钮与表单、语音消息等，充分利用每个 IM 的原生能力
- 扩展文件、图片和富媒体在现有渠道的支持范围

### 工作空间管理

- 多项目工作空间切换与持久化
- 按工作空间配置 agent 和频道
- 工作空间级别的会话历史与上下文

## 许可证

[MIT](LICENSE)
