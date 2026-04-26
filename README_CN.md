<div align="center">

<img src="https://pub-806a1b8456464ce7a6c110f84946697e.r2.dev/documents/v0.1/banner.webp" width="100%" alt="VibeAround — 你的本地 AI Agent，随时随地 vibe coding" />

# VibeAround

**你的本地 AI Agent，随时随地 vibe coding。**

[English](README.md) | [简体中文](README_CN.md) | [Wiki](https://github.com/jazzenchen/VibeAround/wiki)

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.82+-000?style=flat-square&logo=rust&logoColor=fff" alt="Rust" />
  <img src="https://img.shields.io/badge/Tauri-2.10-24C8DB?style=flat-square&logo=tauri&logoColor=fff" alt="Tauri" />
  <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=000" alt="React" />
  <img src="https://img.shields.io/badge/ACP-Rust_SDK-000?style=flat-square" alt="ACP" />
  <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License: MIT" />
</p>

</div>

VibeAround 为本地 AI Agent（Claude Code、Codex、Cursor、Gemini CLI…）提供两种远程访问方式：通过常用 IM（Telegram、Slack、飞书、Discord…）直接对话，或通过浏览器打开 Web Terminal（可搭配 tmux 使用）。可根据场景选择 —— 在手机 IM 中发送消息驱动 Agent，或在笔记本 / 平板上进入终端进行调试 —— 访问的始终是同一个 Agent、同一个 workspace。

桌面应用是一个基于 Tauri 的轻量单体应用，提供图形化的配置与服务管理。Agent 和 IM 频道均以插件形式按需启用。

## 演示视频

[![VibeAround 演示视频 —— 会话接力、Agent 切换、多渠道并发](https://img.youtube.com/vi/6kxNKTMz-AM/maxresdefault.jpg)](https://youtu.be/6kxNKTMz-AM)

*VibeAround 实际效果演示：在任意 IM 中直达本地 Agent、终端与手机之间的会话接力、对话中切换 Agent。*

## 核心能力

### 💬 在任意 IM 里和本地 AI Agent 对话

打开 Telegram、Slack、飞书或 Discord，直接向 Agent 发送消息即可协作。编写代码、执行命令、启动服务 —— 完整的编程能力均可在对话中完成。

### 💻 Web Terminal

浏览器中的完整 shell。移动端内置命令面板，提供 ESC / Ctrl / 方向键等特殊按键的快捷输入；可搭配 tmux 使用，关闭浏览器后会话依然保留。

### 🔄 双向会话接力

使用 `/handover` 和 `/pickup` 在终端与 IM 之间接力同一个编程会话，上下文完整保留。支持 Claude Code、Gemini CLI、Codex CLI、Cursor CLI、Kiro CLI 和 Qwen Code。

### 🎛️ 对话中途切换 Agent

在任何频道中执行 `/switch claude`、`/switch codex` 或 `/switch cursor` 即可切换 Agent，无需重启 VibeAround。

### 👁️ 实时预览

Agent 可将开发服务器或渲染后的 Markdown/HTML 通过短时效链接分享，手机和浏览器均可直接访问。

### 🖥️ 一键引导向导

向导自动安装 Agent 依赖、配置各频道凭证、选择隧道服务，基本无需手动修改配置文件。

## 快速开始

```bash
cd src
bun install
bun run prebuild
bun run dev
```

桌面应用首次启动时会进入引导向导：选择 Agent、配置频道、设置隧道。

**环境要求：** Rust 1.82+、Node.js 20+、Bun 1.1+。macOS 还需要执行 `xcode-select --install`。

若尚未安装，请参考各自官网的安装指引：[Rust](https://rust-lang.org/tools/install/)、[Bun](https://bun.com/)、[Node.js](https://nodejs.org/en/download/)。

## 支持的 Agents

所有 Agent 均通过 [ACP (Agent Client Protocol)](https://agentclientprotocol.com/) 在 stdio 上通信。通过 npm 分发的 Agent 首次使用时会自动安装。

| Agent | ACP | 会话接力 |
|---|---|---|
| **Claude Code** | ✅ | ✅ |
| **Gemini CLI** | ✅ | ✅ |
| **Codex CLI** | ✅ | ✅ |
| **Cursor CLI** | ✅ | ✅ |
| **Kiro CLI** | ✅ | ✅ |
| **Qwen Code** | ✅ | ✅ |
| **OpenCode** | ✅ | ❌ |

## 频道插件

每个频道均为独立的 Node.js 插件，基于 [@vibearound/plugin-channel-sdk](https://www.npmjs.com/package/@vibearound/plugin-channel-sdk) 构建。

| 频道 | 认证方式 | 私聊 | 文件/图片 | 流式输出 | 状态 |
|---|---|---|---|---|---|
| **Telegram** | Bot Token | ✅ | ✅ | ✅ | ✅ |
| **飞书 / Lark** | 应用凭证 | ✅ | ✅ | ✅ | ✅ |
| **Discord** | Bot Token | ✅ | ✅ | ✅ | ✅ |
| **Slack** | Bot + App Token | ✅ | ✅ | ✅ | ✅ |
| **微信** | 二维码登录 | ✅ | ✅ | ❌ | ✅ |
| **钉钉** | AppKey + Secret | ✅ | ✅ | ✅ | ✅ |
| **企业微信** | Bot ID + Secret | ✅ | ✅ | ✅ | ✅ |
| **QQ 频道** | App ID + Token | ✅ | ✅ | ❌ | ✅ |

## 内部架构

- **基于 [ACP (Agent Client Protocol)](https://agentclientprotocol.com/)** —— 所有支持的 Agent 均通过 stdio 使用统一协议通信，新增 Agent 成本低，切换体验保持一致。
- **内置隧道** —— 使 Web Terminal 与实时预览可从手机或任意浏览器直接访问。内置 ngrok、localtunnel、Cloudflare 三种后端。
- **插件化架构** —— 每个 IM 频道均为独立的 Node.js 子进程，发布至 npm 后按需加载。第三方基于 [@vibearound/plugin-channel-sdk](https://www.npmjs.com/package/@vibearound/plugin-channel-sdk) 即可扩展新频道，无需修改核心。
- **频道原生渲染** —— 每个频道插件直接使用对应平台的 SDK（Telegraf、Lark SDK、Slack Bolt…），消息以该平台支持的最佳原生格式呈现，而非套用最低公分母的"翻译层"。
- **对外入口鉴权** —— 通过隧道暴露的 Web Terminal 与实时预览链接均需鉴权，即使 URL 公开，也仅授权用户可访问。
- **多 Agent / 多频道并发** —— 不同 Agent 可同时运行在不同频道上（如 Claude 运行在 Telegram、Codex 运行在 Slack），各自独立的路由与会话互不干扰。
- **Skill + MCP 自动注入** —— 启动时 VibeAround 将自身的技能集（`SKILL.md`）与 MCP 端点写入已启用 Agent 的全局配置，使 Agent 自动发现 VibeAround。

## 命令

VibeAround 提供两类斜杠命令：

- **系统命令** —— 管理 VibeAround 会话本身（重置、切换 Agent、跨频道接力会话等），是 VibeAround 在 Agent 之上增加的交互层。
- **Agent 透传** —— `/agent <command>` 将任意斜杠命令原样转发给底层 Agent，使 Agent 原生能力（如 Claude Code 的 `/status`）在 IM 对话中可直接使用。

| 命令 | 说明 |
|---|---|
| `/help` | 显示可用命令 |
| `/new` | 重置会话，开始新的对话 |
| `/switch <agent>` | 在对话中切换 Agent（claude、gemini、codex、cursor、kiro、qwen-code、opencode） |
| `/profile <name>` | 切换 profile |
| `/close` | 关闭当前对话 |
| `/handover` | 导出当前会话，以便在其他设备继续 |
| `/pickup <code>` | 恢复从其他频道移交的会话 |
| `/agent <command>` | 向 Agent 发送斜杠命令，例如 `/agent status` |

在 Slack 中，`/` 前缀会被客户端拦截，请改用 `/va` 或 `/vibearound`，例如 `/va switch claude`。

## 平台支持

代码库支持 macOS、Linux 和 Windows。目前仅提供 macOS 的预编译版本，Linux 和 Windows 用户仍可从源码构建。欢迎贡献跨平台 CI。

## 文档

- [Wiki 首页](https://github.com/jazzenchen/VibeAround/wiki)
- [安装指南](https://github.com/jazzenchen/VibeAround/wiki/Setup-Guide)
- [频道插件](https://github.com/jazzenchen/VibeAround/wiki/Channel-Plugins)
- [架构](https://github.com/jazzenchen/VibeAround/wiki/Architecture)
- [配置模型](https://github.com/jazzenchen/VibeAround/wiki/Configuration-Model)
- [FAQ 和故障排除](https://github.com/jazzenchen/VibeAround/wiki/FAQ-and-Troubleshooting)

## 下一步

- **一键启动编程 CLI** —— 在桌面应用中保存 API Key，一键启动 Claude Code / Codex 并自动对接所选服务商（智谱、Minimax、通义千问、DeepSeek…）
- **更多 IM 频道** —— WhatsApp、LINE、Microsoft Teams
- **容器化 + 沙盒架构** —— 每个 Agent 运行在独立的容器 / 沙盒中，文件系统与网络访问均限定在安全边界内
- **Web Chat 增强** —— 升级内置 Web 聊天界面，补齐更丰富的渲染、文件上传与历史管理能力
- **工作空间隔离** —— 按工作空间分别管理 Agent、频道和会话历史

## 许可证

[MIT](LICENSE)
