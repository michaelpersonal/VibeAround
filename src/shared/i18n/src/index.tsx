import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export const LOCALES = ["en", "zh-CN"] as const;
export type Locale = (typeof LOCALES)[number];

export const LOCALE_LABELS: Record<Locale, string> = {
  en: "English",
  "zh-CN": "简体中文",
};

const STORAGE_KEY = "vibearound.locale";

type Params = Record<string, string | number | null | undefined>;

const zhCN: Record<string, string> = {
  // Shared
  Language: "语言",
  English: "English",
  "Simplified Chinese": "简体中文",
  Loading: "加载中",
  "Loading…": "加载中…",
  Refresh: "刷新",
  Start: "启动",
  Stop: "停止",
  Cancel: "取消",
  Back: "返回",
  Next: "下一步",
  Confirm: "确认",
  "Get Started": "开始",
  "Save changes": "保存更改",
  "Create profile": "创建 profile",
  "Saving…": "保存中…",
  Edit: "编辑",
  Delete: "删除",
  More: "更多",
  Restore: "恢复",
  Maximize: "最大化",
  "Close session": "关闭会话",
  Unavailable: "不可用",
  current: "当前",
  "not installed": "未安装",

  // Desktop dashboard
  Running: "运行中",
  Spawning: "启动中",
  "Not started": "未启动",
  Stopped: "已停止",
  Crashed: "已崩溃",
  Failed: "失败",
  Busy: "忙碌",
  Idle: "空闲",
  Status: "状态",
  Tunnel: "隧道",
  Agents: "Agent",
  Channels: "频道",
  Launch: "启动",
  Previews: "预览",
  Workspaces: "工作区",
  Live: "实时",
  Polling: "轮询中",
  "Server failed to start": "服务器启动失败",
  Retry: "重试",
  "Open Config Wizard": "打开配置向导",
  "Open Web Dashboard": "打开 Web 控制台",
  "Runtime health for tunnels, agents, and messaging channels.":
    "查看隧道、Agent 和消息频道的运行状态。",
  "No tunnel running": "没有运行中的隧道",
  "No agents running": "没有运行中的 Agent",
  "No channels running": "没有运行中的频道",
  "retry {{seconds}}s": "{{seconds}} 秒后重试",
  "Tunnel ({{provider}})": "隧道（{{provider}}）",
  "Stopped {{name}}": "已停止 {{name}}",
  "Restarting {{name}}": "正在重启 {{name}}",

  // Launch
  "One-click coding agent in your Terminal": "一键在终端启动 Coding Agent",
  "New profile": "新建 profile",
  "Quick launch": "快速启动",
  "Launch the default CLI": "启动默认 CLI",
  "Launch {{agent}} directly": "直接启动 {{agent}}",
  "Launch {{agent}} with {{profile}}": "用 {{profile}} 启动 {{agent}}",
  "Terminal opened for {{profile}}": "已为 {{profile}} 打开终端",
  "{{agent}} launched (no env injected)": "{{agent}} 已启动（未注入环境变量）",
  "Quick launch opened": "已打开快速启动",
  "Quick Launch default updated": "已更新快速启动默认 profile",
  "Direct Quick Launch default updated": "已更新直接快速启动默认项",
  "Profile order updated": "Profile 顺序已更新",
  "No profiles yet": "还没有 profile",
  "Add your provider's API key once. From then on it's one click to launch claude or codex with that key already wired up — VibeAround opens a fresh Terminal window and stays out of the way.":
    "只需保存一次服务商 API key，之后一键即可用该 key 启动 Claude 或 Codex。VibeAround 会打开新的终端窗口，其余交给 CLI 自己运行。",
  "Add your first profile": "添加第一个 profile",
  "Direct launch": "直接启动",
  "Direct launch stays pinned above profiles": "直接启动固定显示在 profile 上方",
  "No profile — uses each CLI's existing login session":
    "不使用 profile，沿用各 CLI 已有登录会话",
  "Use {{agent}} as Quick Launch default without a profile":
    "将 {{agent}} 设为不使用 profile 的快速启动默认项",
  "Edit profile · {{label}}": "编辑 profile · {{label}}",
  "Pick a provider": "选择服务商",
  "New profile · {{provider}}": "新建 profile · {{provider}}",
  "Configure a Quick Launch provider profile.": "配置快速启动服务商 profile。",
  "The provider {{provider}} is no longer in the catalog. Form fell back to a custom endpoint — re-pick a provider via Back, or edit the URL/key and save.":
    "服务商 {{provider}} 已不在 catalog 中。表单已回退到自定义 endpoint，你可以返回重新选择服务商，或编辑 URL/key 后保存。",
  "Label is required": "必须填写名称",
  "Pick at least one API type": "至少选择一种 API 类型",
  "{{field}} is required": "必须填写 {{field}}",
  "Model is required for {{apiType}}": "{{apiType}} 必须填写模型",
  "Base URL is required for {{apiType}}": "{{apiType}} 必须填写 Base URL",
  Profile: "Profile",
  Label: "名称",
  "Visible name for this profile.": "这个 profile 的显示名称。",
  Credentials: "凭据",
  "Model settings": "模型设置",
  Endpoint: "Endpoint",
  "Base URL": "Base URL",
  "Leave blank to use the catalog default.": "留空则使用 catalog 默认值。",
  "Required for custom endpoints.": "自定义 endpoint 必填。",
  "Endpoint URL from the provider dashboard.": "服务商控制台里的 endpoint URL。",
  "Deployment name": "部署名称",
  Model: "模型",
  "Select a model": "选择模型",
  "model id (e.g. gpt-4o, claude-sonnet-4-6)":
    "模型 id（例如 gpt-4o、claude-sonnet-4-6）",
  "Reasoning effort": "推理强度",
  "DeepSeek proxy": "DeepSeek 代理",
  "Thinking mode": "Thinking 模式",
  "Replay reasoning content": "回放 reasoning content",
  "API kinds": "API 类型",
  "Select every API shape this endpoint supports.": "选择这个 endpoint 支持的所有 API 形态。",
  Hide: "隐藏",
  Reveal: "显示",
  "Search providers": "搜索服务商",
  "Preset providers": "预设服务商",
  "No providers found. The catalog ships with the desktop binary; if you see this, the install is broken.":
    "未找到服务商。Catalog 随桌面应用一起发布，如果看到这条消息，说明安装已损坏。",
  "No matching providers": "没有匹配的服务商",
  Custom: "自定义",
  "Custom endpoint": "自定义 endpoint",
  "Bring your own URL + key": "使用自己的 URL 和 key",
  "Use API key": "使用 API key",
  "API key": "API key",
  "Used by Codex and OpenCode for reasoning/tools. Must be an Azure deployment that supports the Responses API.":
    "Codex 和 OpenCode 会用它处理 reasoning/tools。必须是支持 Responses API 的 Azure 部署。",
  "Chat Completions fallback for CLIs/providers that cannot use Responses.":
    "给不支持 Responses 的 CLI/服务商使用的 Chat Completions 回退方案。",
  "Anthropic API": "Anthropic API",
  "OpenAI-compatible Chat": "OpenAI 兼容 Chat",
  "OpenAI Responses": "OpenAI Responses",
  "Gemini API": "Gemini API",
  "Terminal app": "终端应用",
  "API proxy": "API 代理",
  Auto: "自动",
  "Force on": "强制开启",
  Off: "关闭",
  "Launch settings": "启动设置",
  "Terminal: {{terminal}}": "终端：{{terminal}}",
  "API proxy: {{mode}}": "API 代理：{{mode}}",
  Workspace: "工作区",
  "Choose Launch Workspace": "选择启动工作区",
  "Choose folder...": "选择文件夹...",
  "Choose launch workspace": "选择启动工作区",
  "Use {{agent}} with {{profile}} as Quick Launch default":
    "将 {{agent}} + {{profile}} 设为快速启动默认项",
  "Launch {{agent}} via {{apiType}}": "通过 {{apiType}} 启动 {{agent}}",
  "Click to launch {{agent}} via {{apiType}} anyway.":
    "仍然通过 {{apiType}} 启动 {{agent}}。",
  "Delete profile \"{{label}}\"?": "删除 profile「{{label}}」？",
  "Reorder {{label}}": "重新排序 {{label}}",

  // Onboarding
  Welcome: "欢迎",
  "Quick Launch": "快速启动",
  "Welcome to VibeAround": "欢迎使用 VibeAround",
  "Let's set things up so you can vibe code from anywhere. This will only take a minute — configure your agents, messaging channels, and tunnel.":
    "先完成几个简单设置，你就可以从任何地方 vibe code。只需一分钟，配置 Agent、消息频道和隧道即可。",
  "Step {{current}} of {{total}} — {{step}}": "第 {{current}} / {{total}} 步 — {{step}}",
  "Continue Anyway": "仍然继续",
  "Open VibeAround": "打开 VibeAround",
  "Confirming…": "确认中…",
  "Pick the CLI VibeAround should start from Launch and IM messages.":
    "选择 VibeAround 在启动页和 IM 消息中默认启动的 CLI。",
  "Best recommended": "推荐选择",
  "Other CLIs": "其他 CLI",
  "API profiles": "API profile",
  "Optional. Save API keys now; choose launch defaults later in Launch.":
    "可选。现在保存 API key，之后在启动页选择默认启动项。",
  "Add API profile": "添加 API profile",
  "No API profiles yet. You can add one now or skip this step.":
    "还没有 API profile。你可以现在添加，也可以跳过这一步。",
  "Default workspace": "默认工作区",
  "IM Channels": "IM 频道",
  "Connect messaging bots to vibe code from your phone. Install plugins from the registry, then configure and enable them.":
    "连接消息机器人后，就能在手机上 vibe code。先从 registry 安装插件，然后配置并启用。",
  "View on GitHub": "在 GitHub 查看",
  Installing: "安装中",
  "Installing…": "安装中…",
  Install: "安装",
  "QR Login": "二维码登录",
  "Generate a QR code, scan it with the app, then wait for authorization.":
    "生成二维码，用对应 App 扫描，然后等待授权。",
  Reconnect: "重新连接",
  "Waiting…": "等待中…",
  Connect: "连接",
  "QR code": "二维码",
  "Scan with the app and confirm on your phone.": "用 App 扫码，并在手机上确认。",
  "Expose your local server to the internet for IM webhooks and remote access. Skip if you only use it locally.":
    "将本地服务暴露到互联网，用于 IM webhook 和远程访问。如果只本地使用，可以跳过。",
  Recommended: "推荐",
  "Auth Token": "Auth Token",
  "Domain (optional)": "域名（可选）",
  "Tunnel Token": "Tunnel Token",
  "Hostname (optional)": "主机名（可选）",
  "Ready to Launch": "准备启动",
  "Review your configuration. You can always change these in settings.json later.":
    "检查你的配置。之后仍可在 settings.json 中修改。",
  "None configured": "未配置",
  "VibeAround will add an MCP server entry to your coding agents' global settings and install a handover skill for session transfer between devices. Your existing agent settings will not be overwritten.":
    "VibeAround 会向 Coding Agent 的全局设置添加 MCP server，并安装用于跨设备转移会话的 handover skill。已有 Agent 设置不会被覆盖。",
  "Installation Cancelled": "安装已取消",
  "Installation Completed with Errors": "安装完成，但有错误",
  "Installation Complete": "安装完成",
  "Installing VibeAround": "正在安装 VibeAround",
  "Review the results below.": "查看下面的结果。",
  "Setting up your agents and plugins...": "正在设置 Agent 和插件...",
  "Collapse install log": "收起安装日志",
  "Expand install log": "展开安装日志",
  "Delete {{label}}": "删除 {{label}}",
  "Toggle {{name}}": "启用/停用 {{name}}",
  "Already authenticated.": "已完成认证。",
  "Scan the QR code.": "请扫描二维码。",
  "Connected successfully.": "连接成功。",
  "Not confirmed.": "未确认。",
  "Connection lost. Try again.": "连接丢失，请重试。",
  "Cancelled.": "已取消。",
  Cancelled: "已取消",
  "Already installed": "已安装",
  "Install complete": "安装完成",
  "Installing MCP config…": "正在安装 MCP 配置…",
  "MCP config installed": "MCP 配置已安装",
  "Skill file installed": "Skill 文件已安装",
  "Plugin not found in registry": "Registry 中未找到插件",
  "Running: git clone + npm install + build": "正在运行：git clone + npm install + build",
  "MCP config": "MCP 配置",
  "Skill file": "Skill 文件",
  "CLI install": "CLI 安装",
  "Plugin install": "插件安装",

  // Desktop pages
  "Active dev-server proxies and markdown previews. Owner links are permanent; share links rotate every {{minutes}} minutes.":
    "活跃的开发服务器代理和 Markdown 预览。Owner 链接长期有效，分享链接每 {{minutes}} 分钟轮换一次。",
  "No active previews. Ask your coding agent to run preview or md_preview.":
    "没有活跃预览。让你的 Coding Agent 运行 preview 或 md_preview。",
  Local: "本地",
  "Tunnel · owner": "隧道 · owner",
  "Tunnel · share": "隧道 · 分享",
  "Tunnel not running": "隧道未运行",
  "Share key expired": "分享 key 已过期",
  "Close (kills dev server)": "关闭（会终止开发服务器）",
  Close: "关闭",
  "Workspace folders where agents build projects. The built-in workspace is used when no default folder is selected.":
    "Agent 构建项目时使用的工作区文件夹。未选择默认文件夹时，会使用内置工作区。",
  "Select Workspace Folder": "选择工作区文件夹",
  Selecting: "选择中",
  "Selecting…": "选择中…",
  "Add Folder": "添加文件夹",
  "Built-in": "内置",
  Default: "默认",
  "Set as default": "设为默认",
  "Remove workspace": "移除工作区",
  "No workspaces configured": "未配置工作区",
  "unified runtime for ai coding agents": "面向 AI Coding Agent 的统一运行时",

  // Web dashboard
  Terminal: "终端",
  Chat: "聊天",
  "{{running}}/{{total}} active": "{{running}}/{{total}} 活跃",
  connected: "已连接",
  "Switch to light theme": "切换到浅色主题",
  "Switch to dark theme": "切换到深色主题",
  "Tab view": "标签视图",
  "Grid view": "网格视图",
  "WebSocket follows page host (tunnel works on phone)": "WebSocket 跟随页面 host（手机隧道可用）",
  "Tunnel: — (see desktop tray)": "隧道：—（见桌面托盘）",
  "Exit Maximized": "退出最大化",
  "{{count}} proc": "{{count}} 个进程",
  "Add CLI": "添加 CLI",
  "Add CLI session": "添加 CLI 会话",
  "New session": "新会话",
  Profiles: "Profiles",
  "tmux sessions": "tmux 会话",
  "session name…": "会话名…",
  "No sessions yet. Add a CLI to start.": "还没有会话。添加一个 CLI 开始。",
  RUNNING: "运行中",
  IDLE: "空闲",
  STOPPED: "已停止",
  ERROR: "错误",
  "Restore panel": "恢复面板",
  "Maximize panel": "最大化面板",
  Prompt: "输入",
  "Type text to send to terminal…": "输入要发送到终端的文本…",
  Send: "发送",
  "No messages yet": "还没有消息",
  "Send a message to start a conversation.": "发送一条消息开始对话。",
  "Chat with {{agent}}": "与 {{agent}} 聊天",
  "Send a message to start.": "发送消息开始。",
  "channel: web": "频道：web",
  "chat: {{value}}": "聊天：{{value}}",
  "agent: {{value}}": "Agent：{{value}}",
  "version: {{value}}": "版本：{{value}}",
  "sessionId: {{value}}": "会话 ID：{{value}}",
  "Using tool: {{tool}}…": "正在使用工具：{{tool}}…",
  "Error: {{error}}": "错误：{{error}}",
  "Message {{agent}}…": "给 {{agent}} 发消息…",
  "Connecting…": "连接中…",
  "Message Claude…": "给 Claude 发消息…",
  "Chat with": "聊天对象",
  "Loading sessions…": "正在加载会话…",
  "Scroll to bottom": "滚动到底部",

  // Pairing
  "Token is required.": "必须填写 token。",
  "That doesn't look like a VibeAround auth token.": "这看起来不像 VibeAround auth token。",
  "Pair your browser": "配对浏览器",
  "Connect this browser to your VibeAround instance.": "将这个浏览器连接到你的 VibeAround 实例。",
  "Generating pairing code…": "正在生成配对码…",
  "Your pairing code": "你的配对码",
  "Waiting for /pair {{code}} · {{seconds}}s": "等待 /pair {{code}} · {{seconds}} 秒",
  "Code expired": "配对码已过期",
  "✓ Paired! Loading dashboard…": "✓ 已配对，正在加载控制台…",
  "How to pair": "如何配对",
  "Open any IM channel connected to VibeAround.": "打开任意已连接 VibeAround 的 IM 频道。",
  "This page will update automatically.": "此页面会自动更新。",
  "Generate new code": "生成新的配对码",
  "I have a token — paste it": "我已有 token，直接粘贴",
  "Session auth token (from ~/.vibearound/auth.json)":
    "Session auth token（来自 ~/.vibearound/auth.json）",
  "hex token…": "hex token…",
  "Unlock dashboard": "解锁控制台",
  "VibeAround · pairing codes expire after 1 minute":
    "VibeAround · 配对码 1 分钟后过期",
};

interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: string, params?: Params) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => initialLocale());

  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.lang = locale;
    }
  }, [locale]);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    if (typeof window !== "undefined") {
      window.localStorage.setItem(STORAGE_KEY, next);
    }
  }, []);

  const t = useCallback(
    (key: string, params?: Params) => translate(locale, key, params),
    [locale],
  );

  const value = useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const value = useContext(I18nContext);
  if (!value) {
    throw new Error("useI18n must be used inside I18nProvider");
  }
  return value;
}

export function translate(locale: Locale, key: string, params?: Params): string {
  const template = locale === "zh-CN" ? zhCN[key] ?? key : key;
  if (!params) return template;
  return template.replace(/\{\{\s*(\w+)\s*\}\}/g, (_match, name: string) => {
    const value = params[name];
    return value == null ? "" : String(value);
  });
}

function initialLocale(): Locale {
  if (typeof window !== "undefined") {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (isLocale(stored)) return stored;

    const languages = window.navigator.languages?.length
      ? window.navigator.languages
      : [window.navigator.language];
    for (const language of languages) {
      const normalized = normalizeLocale(language);
      if (normalized) return normalized;
    }
  }
  return "en";
}

function normalizeLocale(value: string | undefined): Locale | null {
  if (!value) return null;
  const lower = value.toLowerCase();
  if (lower === "zh-cn" || lower === "zh-hans" || lower.startsWith("zh-cn")) {
    return "zh-CN";
  }
  if (lower.startsWith("zh")) return "zh-CN";
  if (lower.startsWith("en")) return "en";
  return null;
}

function isLocale(value: string | null): value is Locale {
  return value === "en" || value === "zh-CN";
}
