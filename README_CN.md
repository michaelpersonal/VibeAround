<div align="center">

<img src="https://pub-806a1b8456464ce7a6c110f84946697e.r2.dev/documents/v0.1/banner.webp" width="100%" alt="VibeAround — 你的本地 AI Agent，随时随地 vibe coding" />

# VibeAround

**你的本地 AI Agent，随时随地 vibe coding。**

[English](README.md) | [简体中文](README_CN.md) | [Wiki](https://github.com/jazzenchen/VibeAround/wiki) | [Releases](https://github.com/jazzenchen/VibeAround/releases)

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.82+-000?style=flat-square&logo=rust&logoColor=fff" alt="Rust" />
  <img src="https://img.shields.io/badge/Tauri-2.10-24C8DB?style=flat-square&logo=tauri&logoColor=fff" alt="Tauri" />
  <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=000" alt="React" />
  <img src="https://img.shields.io/badge/ACP-Rust_SDK-000?style=flat-square" alt="ACP" />
  <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License: MIT" />
</p>

</div>

VibeAround 是一个面向本地编程 Agent 的桌面中枢，支持 Claude Code、Codex CLI、Gemini CLI、Cursor CLI、Kiro CLI、Qwen Code 和 OpenCode。Agent 仍然运行在你自己的电脑上，VibeAround 负责把它们接到更自然的入口：桌面一键启动、手机 IM 对话、以及可从任意设备打开的浏览器终端。

当你既想保留本地工具链，又希望远程随时接入时，它会很顺手。你可以用预设 profile 启动带有正确 API Key 的 CLI，在飞书或 Slack 里直接和本地 Agent 对话，把终端里的会话接力到手机上，或者给本地预览生成一个可访问的临时链接。

## 演示视频

[![VibeAround 演示视频 —— 会话接力、Agent 切换、多频道并发](https://img.youtube.com/vi/6kxNKTMz-AM/maxresdefault.jpg)](https://youtu.be/6kxNKTMz-AM)

*在任意 IM 中直达本地 Agent，在终端与手机之间接力会话，并在对话中切换 Agent。*

## 为什么是 VibeAround

### 用正确的 profile 一键启动编程 CLI

为 Azure OpenAI、DeepSeek、Gemini、Minimax、Moonshot、OpenRouter、Z.ai 等服务商保存 API profile。选择工作目录和终端后，即可一键启动 Claude Code 或 Codex，环境变量自动就位。

### 在日常 IM 里使用本地 Agent

通过 Telegram、飞书/Lark、Discord、Slack、微信、钉钉、企业微信或 QQ Bot 和 Agent 对话。Agent 可以写代码、运行命令、启动服务，并把执行过程流式返回到频道里。

### 一个会话，多个入口

使用 `/handover` 和 `/pickup` 在 IM、终端和浏览器之间移动同一个编程会话。上下文跟着会话走，设备变了，工作不需要重来。

### 打开真正可用的浏览器终端

Web Terminal 提供完整 shell，可从手机、平板或另一台电脑进入。移动端内置 ESC、Ctrl、方向键等命令面板；搭配 tmux 时，关闭浏览器后会话也可以继续保留。

### 把本地预览分享出去

本地开发服务器、Markdown 或 HTML 渲染结果都可以通过带鉴权的短时链接打开。适合“Agent 改完了，我想马上在手机上看一眼”的日常循环。

### 安装状态不再靠猜

首次启动向导会安装选中的 Agent bridge 与频道插件，写入 MCP 和 Skill 配置，收集频道凭证，并展示每个步骤的安装日志，让成功、跳过、失败都能看清楚。

## 下载 VibeAround

最新桌面版本可在 [GitHub Releases](https://github.com/jazzenchen/VibeAround/releases) 下载。

| 平台 | 构建产物 |
|---|---|
| macOS Apple Silicon | `.dmg` |
| Windows | `.zip` |
| Linux | 暂时从源码构建 |

源码可在 macOS、Windows 和 Linux 上运行。跨平台打包还在持续完善中，不同平台的安装体验会逐步对齐。

## 支持的 Agents

所有 Agent 均通过 [ACP (Agent Client Protocol)](https://agentclientprotocol.com/) 在 stdio 上通信。需要 npm 分发 bridge 时，VibeAround 会在启用相关能力时安装。

| Agent | IM 对话 | 会话接力 | Launch Profile |
|---|---|---|---|
| **Claude Code** | 支持 | 支持 | 支持 |
| **Codex CLI** | 支持 | 支持 | 支持 |
| **Gemini CLI** | 支持 | 支持 | 直接启动 |
| **Cursor CLI** | 支持 | 支持 | 直接启动 |
| **Kiro CLI** | 支持 | 支持 | 直接启动 |
| **Qwen Code** | 支持 | 支持 | 直接启动 |
| **OpenCode** | 支持 | 暂不支持 | 直接启动 |

## 频道插件

每个 IM 频道都是独立的 Node.js 插件，基于 [@vibearound/plugin-channel-sdk](https://www.npmjs.com/package/@vibearound/plugin-channel-sdk) 构建。官方插件可在首次启动向导中安装。

| 频道 | 认证方式 | 私聊 | 文件/图片 | 流式输出 |
|---|---|---|---|---|
| **Telegram** | Bot Token | 支持 | 支持 | 支持 |
| **飞书 / Lark** | 应用凭证 | 支持 | 支持 | 支持 |
| **Discord** | Bot Token | 支持 | 支持 | 支持 |
| **Slack** | Bot + App Token | 支持 | 支持 | 支持 |
| **微信** | 二维码登录 | 支持 | 支持 | 暂不支持 |
| **钉钉** | AppKey + Secret | 支持 | 支持 | 支持 |
| **企业微信** | Bot ID + Secret | 支持 | 支持 | 支持 |
| **QQ Bot** | App ID + Token | 支持 | 支持 | 暂不支持 |

## 工作方式

- **本地优先的运行时** —— Agent、频道和会话都运行在你自己的机器上，VibeAround 只负责提供受控入口。
- **统一 Agent 协议** —— ACP 让 Claude、Codex、Gemini、Cursor、Kiro、Qwen 和 OpenCode 进入同一套路由与会话模型。
- **理解服务商的启动 profile** —— 桌面 profile 保存不同 API 服务商的配置，并用正确环境启动兼容的 CLI。
- **插件进程模型** —— 每个 IM 频道独立为子进程，新频道可以在不修改核心的情况下扩展。
- **频道原生渲染** —— 插件直接使用 Telegraf、Lark SDK、Slack Bolt 等平台 SDK，让消息以平台支持的最佳格式呈现。
- **带鉴权的隧道入口** —— Web Terminal 和预览链接可以被其他设备打开，但仍需要 VibeAround 授权。
- **Skill + MCP 自动注入** —— 启用的 Agent 会自动发现 VibeAround 写入的技能文件和 MCP 配置。

## 本地开发

```bash
cd src
bun install
bun run prebuild
bun run dev
```

环境要求：Rust 1.82+、Node.js 20+、Bun 1.1+。macOS 还需要执行 `xcode-select --install`。

全新 clone 不需要本地 plugin SDK checkout。只有在本地开发频道插件时，才需要 `src/plugins/channel-sdk`。

## 斜杠命令

| 命令 | 说明 |
|---|---|
| `/help` | 显示可用命令 |
| `/new` | 重置会话，开始新的对话 |
| `/switch <agent>` | 在对话中切换 Agent |
| `/profile <name>` | 切换 profile |
| `/close` | 关闭当前对话 |
| `/handover` | 导出当前会话，以便在其他设备继续 |
| `/pickup <code>` | 恢复从其他频道移交的会话 |
| `/agent <command>` | 向底层 Agent 发送斜杠命令，例如 `/agent status` |

在 Slack 中，`/` 前缀会被客户端拦截，请改用 `/va` 或 `/vibearound`，例如 `/va switch claude`。

## 文档

- [Wiki 首页](https://github.com/jazzenchen/VibeAround/wiki)
- [安装指南](https://github.com/jazzenchen/VibeAround/wiki/Setup-Guide)
- [频道插件](https://github.com/jazzenchen/VibeAround/wiki/Channel-Plugins)
- [架构](https://github.com/jazzenchen/VibeAround/wiki/Architecture)
- [配置模型](https://github.com/jazzenchen/VibeAround/wiki/Configuration-Model)
- [FAQ 和故障排除](https://github.com/jazzenchen/VibeAround/wiki/FAQ-and-Troubleshooting)

## 路线图

- 更完整的跨平台安装器与自动更新
- 更强的 Web Chat：历史、文件上传、富文本渲染控制
- 更多频道插件，以及更顺滑的插件市场安装流程
- 更细的 workspace 隔离，适合团队和多项目场景
- 沙盒化或容器化的 Agent 执行边界

## 许可证

[MIT](LICENSE)
