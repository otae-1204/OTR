# OTR 项目架构

> 只读展示本机所有 AI Coding Agent 的 Token 消耗(类似 CC-Switch,但只做"看",不做"切"。曾用名 Token-Show)。

## 1. 定位

**目标**
- 聚合显示本机所有 AI Coding Agent 的 Token 用量:按 Agent / 按天 / 按模型 / 按会话
- 常驻系统托盘,实时更新"今日总量";主窗口提供仪表盘与明细
- 纯本地读取,零网络依赖、零遥测;对 Agent 本身零侵入(只读文件)

**非目标**
- 不切换 Provider / 不改任何 Agent 的配置(C Cod-Switch 的职责)
- 不拦截、不代理 API 流量(完全走本地落盘数据的旁路解析)

## 2. 技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 桌面框架 | **Tauri 2** | 与 CC-Switch 同栈,可直接参考其实现;安装包 ~5MB、内存低,适合托盘常驻 |
| 后端 | **Rust** | `notify` 文件监听成熟;JSONL 大文件增量解析性能好 |
| 前端 | **React 18 + TypeScript + Vite** | 生态成熟,类型可通过 tauri-specta 从 Rust 自动生成 |
| 图表 | **Recharts**(或 ECharts) | 按天折线 + 模型占比足够;要更炫再换 ECharts |
| 本地存储 | **SQLite(rusqlite, bundled)** | 历史快照 + 文件解析游标;Agent 原始数据被清理后曲线仍完整 |
| 托盘/系统集成 | tauri-plugin-single-instance、tauri-plugin-autostart | 单实例、开机自启 |

备选方案(不建议,除非不想碰 Rust):Electron(全 JS,但包体与内存大)、Wails(Go)、纯 CLI(类似 ccusage,无托盘 UI)。

## 3. 总体架构

```
┌───────────────────────────────────────────────────────┐
│  前端 (React)                                          │
│  仪表盘 Dashboard / 明细表 / 设置 / 托盘弹窗            │
└──────────────▲────────────────────────▲────────────────┘
      Events: usage://updated          Commands: get_summary / get_daily / ...
┌──────────────┴────────────────────────┴────────────────┐
│  应用层 (Tauri)   commands.rs / events.rs / tray.rs     │
├─────────────────────────────────────────────────────────┤
│  核心服务层                                               │
│  aggregator(内存聚合)  store(SQLite快照+游标)  pricing   │
├─────────────────────────────────────────────────────────┤
│  Provider 层(AgentProvider trait,每个 Agent 一个适配器)  │
│  claude_code │ codex │ zcode │ dsh │ opencode │ pi │ cursor │
├─────────────────────────────────────────────────────────┤
│  基础设施   watcher(notify 监听+防抖)  paths(路径探测)    │
└─────────────────────────────────────────────────────────┘
```

**数据流**

1. 启动时:`paths` 探测各 Agent 数据目录 → Provider `detect()` 判断装没装
2. 首次:`scan_full()` 全量解析 → 写 SQLite 快照 → 聚合器加载到内存
3. 运行中:`watcher` 监听所有已启用 Provider 的数据目录(防抖)
4. 文件变化 → 对应 Provider 增量解析(按游标续读)→ 产出 `UsageRecord` → 更新聚合 + SQLite
5. 发事件 `usage://updated`(只带信号)→ 前端重新拉取 summary
6. 托盘 tooltip / 菜单同步"今日总量"

## 4. 核心抽象

### 4.1 Provider trait

```rust
pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &'static str;                 // "claude-code"
    fn display_name(&self) -> &'static str;       // "Claude Code"
    fn detect(&self) -> bool;                     // 数据目录是否存在
    fn watch_paths(&self) -> Vec<PathBuf>;        // 交给 watcher 的监听目标
    fn scan_full(&self) -> Result<Vec<UsageRecord>>;
    /// 增量解析:返回新游标 + 新记录;JSONL 按字节 offset 续读,整文件 JSON 直接重读
    fn scan_incremental(&self, cursors: &mut FileCursors) -> Result<Vec<UsageRecord>>;
}
```

注册表 `providers/mod.rs` 里静态注册全部 Provider;运行时可被用户在设置里启停。新增一个 Agent = 新增一个文件 + 注册一行,**不碰核心代码**。

### 4.2 数据模型

```rust
pub struct UsageRecord {
    pub agent: String,              // "dsh"
    pub session_id: Option<String>,
    pub project: Option<String>,    // 来自 cwd 编码目录,反解成真实路径
    pub model: Option<String>,
    pub provider: Option<String>,   // DSH 有 provider 维度,如 "deepseek-official"
    pub ts: i64,                    // unix ms
    pub input_tokens: u64,          // 未命中缓存的输入
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,      // 有则填;通常已含在 output 内,展示时注明,不加总
    pub calls: u64,                 // 请求次数(DSH ledger 有,Claude 按条消息计)
}
```

**统计口径(重要,写进代码常量与 UI 提示):**
- `total = input + output + cache_read + cache_write`(`reasoning` 已含在 output 中,单独展示不加总)
- `cache_read` 永远单列展示,不与普通 input 混算——各家的缓存命中差异巨大,混算会失真
- 四个维度独立聚合:按 Agent、按天、按模型、按会话

## 5. Provider 数据源对照表(本机已实测)

### 5.1 DSH —— 优先级最高,数据质量最好
| 项 | 内容 |
|---|---|
| 数据源 | `$DSH_HOME/storages/cost-meter/ledger.json`(可选按天台账)、`$DSH_HOME/storages/session_projcache.json`(按会话)、`$DSH_HOME/sessions/**/session.jsonl[.zstd]`(按小时及无台账时的按天回退) |
| 格式 | 整文件 JSON,自描述 `version` 字段 |
| 台账结构 | `days["2026-08-29"] = { date, input, output, cacheRead, cacheWrite, reasoning, calls, cost, byProviderModel: { "<provider:model>": {...} } }` |
| 会话结构 | `tables.sessions[id].rows.costUsage = { provider, model, totals: {input, output, cacheRead, cacheWrite, reasoning, cost}, byModel: {...} }`,`identity.cwd` 可反解项目路径 |
| 解析要点 | 台账存在时作为按天权威数据并复用成本;未安装 cost-meter、没有台账时,会话日志的同一增量同时写入按天与按小时表;按天数据源首次选定后保持不变,避免运行中台账出现/消失导致双计;日志支持 zstd 多 frame,`DSH_HOME` 与宿主保持一致,未找到日志时降级使用台账会话时间 |
| 会话原始流 | `$DSH_HOME/sessions/<编码cwd>/<uuid>/session.jsonl.zstd` 是 zstd 多 frame;按小时趋势读取 usage 事件,只读不修改 |

### 5.2 Claude Code
| 项 | 内容 |
|---|---|
| 数据源 | `~/.claude/projects/<编码路径>/*.jsonl`(append-only),另 `~/.claude/stats-cache.json` 可旁证 |
| 格式 | JSONL,assistant 消息内 `message.usage = { input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }` |
| 解析要点 | **必须去重**:流式写盘会产生同 `message.id + requestId` 的重复条目(参考 ccusage 做法);文件 append-only → 支持按 offset 增量续读 |

### 5.3 Codex CLI
| 项 | 内容 |
|---|---|
| 数据源 | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`(另有 `session_index.jsonl`、`archived_sessions/`) |
| 格式 | JSONL 事件流;`token_count` 事件含 `info.total_token_usage`(累计值)与 `last_token_usage` |
| 解析要点 | 累计值 → 取该会话末次累计或用 last 值累加;`cached_input_tokens` 映射到 cache_read |

### 5.4 ZCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.zcode/cli/rollout/model-io-sess_*.jsonl`(本机确认) |
| 格式 | JSONL,同一次调用会出现两种 usage 形态:Anthropic 风格 `{input_tokens, output_tokens, cache_read_input_tokens, ...}` 与汇总风格 `{inputTokens, outputTokens, totalTokens, cacheReadTokens, cacheWriteTokens}` |
| 解析要点 | **两形态疑似同一次调用的请求/响应记录,必须按 sess + 轮次去重,否则翻倍**;文件名带 session id,天然可按会话聚合;格式未公开,需宽容解析(`serde` 默认值 + 忽略未知字段),fixtures 测试锁行为 |

### 5.5 OpenCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.local/share/opencode/opencode.db`(SQLite,新版从 JSON storage 迁移而来;旧版为 `storage/` 目录 JSON) |
| 解析要点 | **WAL 模式**:直接打开可能读到中间态。策略:以 `SQLITE_OPEN_READONLY | immutable` 打开快照视图,或把 `-wal` 一起考虑;检测版本,旧版走 JSON storage 解析 |
| 优先级 | P2(表结构未逐一验证,实现时先探表) |

### 5.6 Pi
| 项 | 内容 |
|---|---|
| 数据源 | `~/.pi/agent/sessions/` |
| 格式 | JSONL,自带美元成本 |

### 5.7 Cursor
| 项 | 内容 |
|---|---|
| 数据源 | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` 只读取出 `cursorAuth/accessToken`,再请求 `POST https://cursor.com/api/dashboard/get-filtered-usage-events` |
| 为何走网络 | Cursor 3.x 起 `cursorDiskKV` 里 `tokenCount` 恒为 `{0,0}`,agent-transcripts / ai-code-tracking 也没有 token;官方口径在用量 Dashboard |
| 解析要点 | Cookie `WorkosCursorSessionToken=<sub>%3A%3A<jwt>`;按事件增量(时间戳水位 + 指纹去重);成本用 `tokenUsage.totalCents/100` 美元;失败时保留已有快照,不阻断其他 Agent |
| 监听 | `state.vscdb` 与 `-wal`,防抖后再拉接口,Provider 内 60s 节流 |

### 5.8 候选扩展(本机已检测到,放路线图)
`.codebuddy` / `.codebuddycn`(CodeBuddy)、`.kimi-code`(Kimi CLI)、`.copilot`(Copilot CLI)、`.qoder-cli`(Qoder)。
实现顺序建议:每次只加一个,先 `detect()` 再摸数据格式。Gemini CLI / Qwen Code 本机未装,不排期。

## 6. 后端模块划分(src-tauri/src/)

```
src-tauri/
├── tauri.conf.json            # 窗口/托盘/权限配置
├── Cargo.toml
└── src/
    ├── main.rs                # 薄壳
    ├── lib.rs                 # setup:探测→全量扫描→起 watcher→注册托盘
    ├── model.rs               # UsageRecord / 聚合视图 / FileCursors
    ├── commands.rs            # Tauri 命令(见 §8)
    ├── events.rs              # 事件负载定义(usage://updated 等)
    ├── providers/
    │   ├── mod.rs             # trait + 注册表 + detect 汇总
    │   ├── jsonl_util.rs      # 可复用:按 offset 续读、行级 JSON 解析
    │   ├── dsh.rs             # ledger.json + session_projcache.json
    │   ├── claude_code.rs     # projects/**/*.jsonl
    │   ├── codex.rs           # sessions/YYYY/MM/DD/rollout-*.jsonl
    │   ├── zcode.rs           # cli/rollout/model-io-sess_*.jsonl
    │   ├── opencode.rs        # opencode.db (SQLite)
    │   ├── pi.rs              # sessions JSONL
    │   └── cursor.rs          # 本机 JWT + dashboard usage events
    ├── watcher.rs             # notify + debounce → 分发给对应 provider
    ├── aggregator.rs          # 内存聚合:今日/近7天/近30天/全部 × agent/model
    ├── store.rs               # SQLite:快照写入、游标表、历史查询
    ├── pricing.rs             # (v2)价格表 + 成本估算;DSH ledger 的 prices 结构可借鉴
    ├── settings.rs            # 用户设置:启停 agent、数据目录覆盖、自启
    └── tray.rs                # 托盘菜单 / tooltip / 左键弹窗

tests/fixtures/                # 各 provider 脱敏样例数据(锁解析行为,防格式漂移)
```

**关键依赖 crate:** `tauri 2`、`notify`、`serde/serde_json`、`rusqlite(bundled)`、`chrono`、`dirs`、`tokio`、`zstd`(备用,DSH 会话流如需直读)。

**SQLite 表设计:**

```sql
-- 按天快照(展示主数据源;Agent 删了历史文件也不影响曲线)
CREATE TABLE usage_daily (
  agent TEXT NOT NULL, date TEXT NOT NULL,            -- "2026-08-29"
  model TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
  cache_read_tokens INTEGER DEFAULT 0, cache_write_tokens INTEGER DEFAULT 0,
  calls INTEGER DEFAULT 0,
  PRIMARY KEY (agent, date, model, provider)
);
-- 会话级明细(明细页)
CREATE TABLE usage_sessions (
  agent TEXT, session_id TEXT, project TEXT, model TEXT,
  started_at INTEGER, last_active_at INTEGER,
  input_tokens INTEGER, output_tokens INTEGER,
  cache_read_tokens INTEGER, cache_write_tokens INTEGER,
  PRIMARY KEY (agent, session_id)
);
-- 增量解析游标
CREATE TABLE file_cursors (
  agent TEXT, path TEXT PRIMARY KEY,
  size INTEGER, mtime_ms INTEGER, parsed_offset INTEGER
);
```

## 7. 前端结构(src/)

```
src/
├── main.tsx / App.tsx           # 主窗口路由:Dashboard / Settings
├── api/bindings.ts              # tauri-specta 生成(或手写)的类型与命令封装
├── hooks/
│   ├── useUsage.ts              # 打开时拉一次 get_summary,之后订阅 usage://updated 重拉
│   └── useTrayPopup.ts
├── pages/
│   ├── Dashboard.tsx            # 汇总卡(今日/7天/30天 × 各agent) + 趋势折线 + 模型占比 + 明细表
│   └── Settings.tsx             # agent 启停 / 数据目录覆盖 / 自启 / 价格表(v2)
└── components/
    ├── StatCard.tsx  AgentCard.tsx  TrendChart.tsx
    ├── ModelPie.tsx  SessionTable.tsx  TrayPopup.tsx
```

托盘交互:tooltip 常显"今日 X tokens";左键弹小型 popup(今日各 agent 简表),双击/按钮开主窗口;右键菜单(刷新/设置/退出)。

## 8. Tauri API 面

**Commands(前端拉取)**
- `list_agents() -> Vec<AgentStatus>`(是否检测到、是否启用、上次扫描时间)
- `get_summary() -> UsageSummary`(今日/近7天/近30天/累计,按 agent × model)
- `get_daily(agent?, from?, to?) -> Vec<DailyUsage>`(折线图)
- `get_sessions(agent?, limit) -> Vec<SessionUsage>`(明细表)
- `rescan(agent?) -> ()`(手动全量重扫)
- `get_settings() / save_settings(s)`

**Events(后端推送)**
- `usage://updated`(轻量信号,前端收到后重拉 summary;避免推大 payload)

## 9. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| **M0** | 脚手架(Tauri2+React)、model/trait 定义、Claude Code Provider 全量扫描、静态仪表盘 | 打开窗口能看到 Claude Code 按天用量 |
| **M1** | watcher + 增量解析(offset 游标)、SQLite 快照、托盘(今日总量) | 跑一轮对话,托盘数字几秒内自动变 |
| **M2** | **DSH Provider(ledger + projcache,含成本复用)**、Codex、ZCode Provider;各 provider fixtures 单测 | 四个 agent 数据同屏聚合正确 |
| **M3** | OpenCode(SQLite/WAL);会话明细页、图表完善;设置页 | 明细能定位到单会话/单模型 |
| **M4** | 通用成本估算(价格表可编辑)、候选 agent(CodeBuddy/Kimi/Copilot/Qoder)、安装包(NSIS)+ 自启 + 单实例 | 交付可分发版本 |

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| 各家落盘格式无官方契约,版本升级会漂移 | Provider 完全隔离;宽容解析(默认值+忽略未知字段);fixtures 单测锁行为;解析失败只降级不崩溃 |
| 首次全量扫描可能面对数百 MB JSONL | offset 游标增量;全量放后台线程,UI 先显示"扫描中" |
| 重复计数(Claude 流式重复、ZCode 双形态) | 每个 Provider 内置去重规则并用样例数据测试 |
| 读 SQLite(WAL)读到中间态 | 只读 + immutable 打开,或读快照副本 |
| Agent 清理/轮转历史文件 | 自持 SQLite 快照,曲线不断;游标文件消失时重置该 provider |
| 计量口径争议(cache/reasoning) | 分列展示不加总,UI 注明口径:`total = input + output + cache_read + cache_write` |

## 11. 新增一个 Provider 的步骤(SOP)

1. 本机找到该 Agent 的数据目录,确认格式(JSONL / SQLite / 压缩包),选最小、最聚合的数据源
2. 在 `providers/` 新建 `xxx.rs`,实现 trait 四个方法;能复用 `jsonl_util` 就复用
3. 取 1~2 份脱敏样本放进 `tests/fixtures/`,写解析单测(含边界:空文件、半行、去重)
4. `providers/mod.rs` 注册一行 + `watch_paths` 给出监听目录
5. 手动验证:`rescan` 后 Dashboard 出现新 Agent 卡片,数字与官方/第三方统计(ccusage 等)对得上
