# OTR 项目架构

> 只读展示本机所有 AI Coding Agent 的 Token 消耗(类似 CC-Switch,但只做"看",不做"切"。曾用名 Token-Show)。

## 1. 定位

**目标**
- 聚合显示本机所有 AI Coding Agent 的 Token 用量:按 Agent / 按天 / 按小时 / 按模型 / 按会话
- 常驻系统托盘,实时更新"今日总量";主窗口提供仪表盘与明细
- 纯本地读取,零遥测;对 Agent 本身零侵入(只读文件)

**非目标**
- 不切换 Provider / 不改任何 Agent 的配置(CC-Switch 的职责)
- 不拦截、不代理 API 流量(完全走本地落盘数据的旁路解析)

## 2. 技术选型

| 层 | 选型 | 说明 |
|---|---|---|
| 桌面框架 | **Tauri 2** | 安装包小、内存低,适合托盘常驻;Windows 用 WebView2 |
| 后端 | **Rust**(edition 2021) | `notify` 文件监听 + JSONL 增量解析 |
| 前端 | **React 18 + TypeScript + Vite** | 类型手写在 `src/api/bindings.ts`(没有用 tauri-specta) |
| 图表 | **Recharts** | 按天折线 + 模型占比 |
| 本地存储 | **SQLite(rusqlite,bundled)** | 历史快照 + 文件解析游标 + 各 Provider 状态 |
| 托盘/系统集成 | `tauri-plugin-single-instance` | 单实例 |

**实际依赖**(见 `src-tauri/Cargo.toml`):
`tauri 2`(tray-icon, image-png)、`tauri-plugin-single-instance`、`serde`/`serde_json`、
`notify 8`、`rusqlite 0.32`(bundled)、`chrono`、`zstd`、`dirs`、`thiserror`、
`ureq`(Cursor 的 dashboard API)、`base64`。

> **注意**:曾经规划过的 `tauri-plugin-autostart`(开机自启)**没有引入**,
> `Cargo.toml` 里没有它。设置里的"启动时最小化"已实现(启动时 `window.hide()`),
> 但真正意义的开机自启仍未做。

## 3. 总体架构

```
┌───────────────────────────────────────────────────────┐
│  前端 (React)  src/                                    │
│  App.tsx(仪表盘/设置切换) + components/                │
└──────────────▲────────────────────────▲────────────────┘
      Events: usage://updated          Commands: get_summary / get_daily / ...
┌──────────────┴────────────────────────┴────────────────┐
│  应用层 (Tauri)   lib.rs(run_scan 编排) / commands.rs / tray.rs │
├─────────────────────────────────────────────────────────┤
│  核心服务层   store.rs(SQLite 快照 + 游标 + 成本口径 CostBasis) │
├─────────────────────────────────────────────────────────┤
│  Provider 层(AgentProvider trait,每个 Agent 一个适配器)  │
│  dsh │ claude_code │ codex │ zcode │ opencode │ pi │ cursor │
├─────────────────────────────────────────────────────────┤
│  基础设施   watcher.rs(notify 监听 + 防抖)  paths.rs(路径探测) │
└─────────────────────────────────────────────────────────┘
```

**数据流**

1. 启动:`paths` 探测各 Agent 数据目录 → Provider `detect()` 判断装没装
2. 首次:`run_scan(app, false, None)` 增量扫描(不是 wipe 全量重建)→ 写 SQLite
3. 运行中:`watcher` 监听已启用 Provider 的数据目录,防抖 2 秒
4. 文件变化 → 对应 Provider 按游标增量解析 → 产出 `UsageRecord` → 写 SQLite
5. 真有数据变化时递增 kv `data_version` → 发事件 `usage://updated`(只带信号)
6. 前端重新拉 summary;子组件靠 `dataVersion` 决定要不要重拉明细
7. 托盘 tooltip / 菜单同步"今日总量"

## 4. 核心抽象

### 4.1 Provider trait

```rust
pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &str;                          // "claude-code"
    fn display_name(&self) -> &str;                // "Claude Code"
    fn detect(&self) -> bool;                      // 数据目录是否存在
    fn watch_paths(&self) -> Vec<PathBuf>;         // 交给 watcher 的监听目标
    /// 解析语义版本;变更时启动会全量重建该 Agent(默认 1)
    fn parser_version(&self) -> u64 { 1 }
    /// UsageRecord.cost 的币种("CNY"/"USD");None = 没有自带成本,只能靠定价表估算
    fn native_cost_currency(&self) -> Option<&'static str> { None }
    /// 从自己持久化的 state 里提取要展示给用户的健康提示(登录失效/截断…)
    fn health(&self, state: &serde_json::Value) -> Option<String> { None }
    /// 增量扫描;full 时外部已重置游标与状态,Provider 自然输出全量
    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>>;
}

pub struct ScanCtx<'a> {
    pub full: bool,
    /// 出口标志:解析器发现数据源被截断/重写时置 true,
    /// run_scan 会丢弃本次增量并以 full=true 重跑一次
    pub force_full: bool,
    pub cursors: &'a mut HashMap<String, FileCursor>,
    /// Provider 级持久化状态(以 kv `state:<id>` 落盘)
    pub state: &'a mut serde_json::Value,
}
```

注册表在 `providers/mod.rs`;用户在设置里启停。新增一个 Agent = 新增一个文件 + 注册一行。

### 4.2 数据模型

```rust
pub struct UsageRecord {
    pub agent: String,              // "dsh"
    pub session_id: Option<String>,
    pub project: Option<String>,    // 来自 cwd
    pub model: Option<String>,
    pub provider: Option<String>,   // DSH 有 provider 维度,如 "deepseek"
    pub title: Option<String>,
    pub ts: i64,                    // unix ms
    pub input_tokens: u64,          // 未命中缓存的输入
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,      // 展示用,不加总
    pub calls: u64,
    pub cost: f64,                  // 自带成本,币种见 native_cost_currency
    pub skip_daily: bool,           // 只写会话表,不写按天表
    pub skip_hourly: bool,
    pub bucket_date: Option<String>,// 覆盖按天/小时桶日期
    pub bucket_hour: Option<i64>,
    pub touch_ts: Option<i64>,      // 会话"最后活跃";缺省用 ts
}
```

**统计口径:**
- `total = input + output + cache_read + cache_write`(`reasoning` 只展示,不加总)
- `cache_read` 永远单列展示,不与普通 input 混算
- 增量语义:每条记录是"新增量"而不是累计值

### 4.3 成本口径(重要)

**定价表(`settings.pricing`)是唯一权威**,统一实现在 `store::CostBasis`:

1. 模型在定价表里有条目 → `tokens × 单价 ÷ 1e6 × 汇率` 重算,**覆盖**自带成本;
2. 没有定价 → 用数据自带成本,按该 Agent 的 `native_cost_currency()` 归一化
   (dsh = ¥ 原值;cursor / pi / opencode = $ × 汇率);
3. 都没有 → 0。

输出恒为 ¥;`RangeSummary.currency` 只是"前端该按哪个币种展示"的提示,
前端 `fmtCost` 再按 `settings.currency` 换算。

`range_summary`(顶部大卡/模型占比)与 `sessions`(明细表)**共用同一套 CostBasis**。
历史上两处口径不同(明细表完全不查定价表、只按 `agent == "dsh"` 特判乘汇率),
导致两处数字系统性对不上。

**两表的语义差异(不是 bug)**:`usage_daily` 按记录日期过滤,统计"范围内发生的用量";
`sessions` 选的是"范围内活跃的会话",金额是该会话的完整累计 —— 跨范围开始的长会话
会把范围外的部分也算进来。另外 `skip_daily` 的记录(如 DSH projcache)只进 sessions 不进 daily。

### 4.4 刷新链

`store` 在**写入用量的同一个事务**里递增 kv `data_version` 并更新 `data_updated_ms`。
`get_summary` 返回 `dataVersion`,`App.tsx` 用它拼 `refreshKey` 决定
`get_range_summary` / `get_daily` / `get_sessions` 要不要重拉。
**空扫描不动版本号** —— 否则每 2 秒一次的空扫描会让前端一直白刷。

### 4.5 容错

- `crate::lock(&mutex)` 是统一的取锁入口:中毒时 `into_inner()` 拿回内部值,
  不会因为一次 panic 让整条刷新链永久失效。
- `run_scan` 与单个 Provider 的 `scan()` 都包了 `catch_unwind`,panic 会转成
  `AppError::Msg` 并写日志。
- `RescanGate` 保证同一时刻只有一个扫描线程,连点托盘只合并成一轮。

## 5. Provider 数据源对照表(本机已实测)

### 5.1 DSH —— 优先级最高,数据质量最好
| 项 | 内容 |
|---|---|
| 数据源 | `$DSH_HOME/storages/cost-meter/ledger.json`(可选按天台账)、`$DSH_HOME/storages/session_projcache.json`(按会话)、`$DSH_HOME/sessions/**/session.jsonl[.zstd]`(按小时及无台账时的按天回退) |
| 格式 | 整文件 JSON,自描述 `version` 字段;会话日志是 zstd 多 frame |
| 台账结构 | `days["2026-08-29"] = { date, input, output, cacheRead, cacheWrite, reasoning, calls, cost, byProviderModel: {...} }` |
| 会话结构 | `tables.sessions[id].rows.costUsage = { provider, model, totals, byModel }`,`identity.cwd` 可反解项目路径 |
| 解析要点 | 台账存在时作为按天权威并复用成本;没有台账时,会话日志的同一增量同时写按天与按小时表;按天数据源首次选定后保持不变(避免台账出现/消失双计);`DSH_HOME` 与宿主一致;日志按 **zstd 魔数**而非扩展名判断;未找到日志时降级用台账会话时间 |
| 性能 | 本机 2232 个文件 / 166MB。`DshState.fileCache` 按 `(size, mtime)` 缓存每个文件的小时聚合,冷扫 ~5.4s → 热扫 ~0.5s |
| 自带成本币种 | CNY(台账是人民币实际计费) |

### 5.2 Claude Code
| 项 | 内容 |
|---|---|
| 数据源 | `~/.claude/projects/<编码路径>/*.jsonl`(append-only) |
| 格式 | JSONL,`message.usage = { input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }` |
| 解析要点 | **必须去重**:流式写盘会产生同 `message.id + requestId` 的重复条目;append-only → 按 offset 增量续读 |
| 自带成本 | 无(靠定价表) |

### 5.3 Codex CLI
| 项 | 内容 |
|---|---|
| 数据源 | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`(另有 `archived_sessions/`) |
| 格式 | JSONL 事件流;`token_count` 事件的 `info` 同时含 `last_token_usage`(本次)与 `total_token_usage`(会话累计) |
| 解析要点 | 优先用 `last`(真实的一次请求用量);`last` 缺失或全 0 而 `total` 在涨时,**用累计值的增量兜底**(否则那一次调用会无声消失),累计基准只增不减。两种口径的 `input_tokens` 都已包含 cached,统一减掉才是未缓存输入。同 thread 的多份 rollout 只保留文件名最新的一份;带 `parent_thread_id` 的派生会话整段跳过(它们重放了父线程的完整 token_count 历史),跳过数量整轮只汇总成一行日志 |
| 自带成本 | 无(靠定价表) |

### 5.4 ZCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.zcode/cli/rollout/model-io-sess_*.jsonl`(主会话,清理快)+ `~/.zcode/cli/agents/<sess>/<agent>/transcript.jsonl`(子代理,留存较久) |
| 格式 | JSONL。用量以 camelCase `{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens}` 为准 |
| 解析要点 | 双源按 `requestId` 去重。transcript 一次调用会写多条带 usage 的事件,**只收白名单** `model_network_status` / `model_request_completed`(且 usage 里不带 `modelRequestCount`);`model_complete`(相同 usage、无 modelId、无 requestId)与 `turn_complete`(整轮汇总)丢弃。空模型不入库 |
| 本机实测 | transcript 里带 `payload.usage` 的 type 只有 `model_complete` / `model_network_status` / `turn_complete` 三种,`model_network_status` 是唯一同时带 modelId 与 requestId 的 |
| 自带成本 | 无(靠定价表) |

### 5.5 OpenCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.local/share/opencode/opencode.db`(SQLite) |
| 解析要点 | WAL 模式:优先只读打开,失败退回普通打开;`session` 表按会话给绝对值快照,与上次快照做 diff;按会话起始日计入按天表 |
| 自带成本币种 | USD |

### 5.6 Pi
| 项 | 内容 |
|---|---|
| 数据源 | `~/.pi/agent/sessions/` |
| 格式 | JSONL,自带美元成本(`usage.cost.total`) |
| 自带成本币种 | USD |

### 5.7 Cursor
| 项 | 内容 |
|---|---|
| 数据源 | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` 只读取出 `cursorAuth/accessToken`,再请求 `POST https://cursor.com/api/dashboard/get-filtered-usage-events` |
| 为何走网络 | Cursor 3.x 起本地 `cursorDiskKV` 里 `tokenCount` 恒为 `{0,0}`,官方口径在用量 Dashboard |
| 解析要点 | Cookie `WorkosCursorSessionToken=<sub>%3A%3A<jwt>`;`tokenUsage.totalCents/100` 是美元 |
| 水位 | 事件按 `ts` 取"重叠窗口"(回退 10 分钟)+ key 去重,晚到的事件不会被永久丢掉;老状态(没有 overlap 标记)第一次升级上来仍严格按水位拉,避免重算历史 |
| 节流 | 成功 60s;401/403 等失败**也**记节流(否则每个 watcher tick 都拿失效 token 打一次 cursor.com),失败原因写进 state 并在 AgentCard 上显示 ⚠ |
| 分页 | 撞到 `MAX_PAGES` 截断时**不推进水位**并暴露提示,下次自动重试补齐 |
| 监听 | `state.vscdb` 与 `-wal` |
| 自带成本币种 | USD |

### 5.8 候选扩展(本机已检测到,放路线图)
`.codebuddy` / `.codebuddycn`(CodeBuddy)、`.kimi-code`(Kimi CLI)、`.copilot`(Copilot CLI)、`.qoder-cli`(Qoder)。
实现顺序建议:每次只加一个,先 `detect()` 再摸数据格式。Gemini CLI / Qwen Code 本机未装,不排期。
(CodeBuddy 已可通过设置里的"自定义 Agent"用 claude-code 布局接进来。)

## 6. 后端模块划分(src-tauri/src/)

```
src-tauri/
├── tauri.conf.json            # 窗口/打包/CSP 配置
├── Cargo.toml
├── src/
│   ├── main.rs                # 薄壳
│   ├── lib.rs                 # setup 编排 + run_scan + crate::lock + RescanGate
│   ├── model.rs               # UsageRecord / 视图结构 / 时间与格式化工具
│   ├── commands.rs            # Tauri 命令(见 §8)
│   ├── store.rs               # SQLite:快照写入、游标、CostBasis、历史查询
│   ├── settings.rs            # 用户设置:启停 agent、定价表、币种汇率、自定义 Agent
│   ├── paths.rs               # 各 Agent 数据目录探测
│   ├── tray.rs                # 托盘图标 / tooltip / 菜单
│   ├── watcher.rs             # notify + 2 秒防抖 → run_scan
│   ├── error.rs               # AppError / Result
│   └── providers/
│       ├── mod.rs             # trait + 注册表
│       ├── jsonl_util.rs      # 按 offset 续读、跨扫描去重窗口
│       ├── dsh.rs  claude_code.rs  codex.rs  zcode.rs
│       ├── opencode.rs  pi.rs  cursor.rs  custom.rs
└── tests/
    ├── scan_real.rs           # 真实数据冒烟(cargo test --release --test scan_real -- --ignored --nocapture)
    └── range_smoke.rs         # 真实库的成本/区间对照
```

> 各 Provider 的解析单测直接写在对应源文件的 `#[cfg(test)] mod tests` 里(内联 fixture),
> **没有** `tests/fixtures/` 目录。

**SQLite 表(实际 schema,见 `store.rs` 的 `SCHEMA`):**

```sql
usage_daily(agent, date, model, provider,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
            reasoning_tokens, calls, cost)            -- PK(agent,date,model,provider)
usage_hourly(agent, date, hour, model, provider, ...)  -- PK(agent,date,hour,model,provider)
usage_session_models(agent, session_id, model, provider, ..., last_ts)
                                                       -- PK(agent,session_id,model,provider)
session_meta(agent, session_id, project, title, started_at, last_active)
file_cursors(agent, path, data)                        -- 增量解析游标(JSON)
kv(k, v)                                               -- state:<agent> / data_version / ...
```

## 7. 前端结构(src/)

```
src/
├── main.tsx / App.tsx           # 主窗口:仪表盘 / 设置 切换 + 筛选栏(Agent × 日期范围)
├── api/bindings.ts              # 手写的类型与命令封装
├── hooks/useUsageData.ts        # summary + agents + settings;事件订阅 + 30s 轮询 + in-flight 去重
├── lib/{format,range,remote}.ts # 格式化 / 日期区间 / GitHub 版本 + 实时汇率
└── components/
    StatCard.tsx  AgentCard.tsx  TrendChart.tsx
    ModelPie.tsx  SessionTable.tsx  Settings.tsx
    AgentIcon.tsx  icons.tsx  Skeleton.tsx
```

> 没有 `pages/` 目录,也没有 `TrayPopup.tsx` / `useTrayPopup`(托盘只有 tooltip 与菜单,不做弹窗)。

托盘交互:tooltip 常显"今日 X tokens";菜单提供刷新/显示主窗口/退出。

## 8. Tauri API 面

**Commands(前端拉取)**
- `list_agents() -> Vec<AgentStatus>`(是否检测到、是否启用、累计 tokens、健康提示)
- `get_summary() -> UsageSummary`(今日各 Agent 卡片 + `dataVersion`)
- `get_range_summary(agent?, from, to) -> RangeSummary`(Hero 大卡 + 模型占比)
- `get_daily(agent?, from?, to?, granularity?) -> Vec<DailyUsage>`(折线图,day/hour/month)
- `get_sessions(agent?, from?, to?, limit?) -> Vec<SessionUsage>`(明细表)
- `rescan(full?)`(手动扫描;有 pending 合并)
- `list_models() -> Vec<String>`(设置页定价表用)
- `get_settings() / save_settings(s)`

**Events(后端推送)**
- `usage://updated`(轻量信号,前端收到后重拉 summary)

## 9. 时间与分桶(已知的一次性不连续)

日期/小时桶按**本地时区**计算(`model::local_date` / `local_hour`),
`UsageRecord.bucket_date` / `bucket_hour` 可以覆盖(DSH 台账、会话快照走这条)。

**这意味着**:换时区、跨 DST,同一批历史数据会被切到不同的日期/小时桶,
历史曲线可能出现一次**一次性不连续**(某天变多、某天变少)。已入库的行不会重算 ——
只有重新全量扫描才会按新时区重排。这是有意接受的取舍:本地时间才是用户看到"今天用了多少"
时的直觉口径,而 `bucket_date` 让带明确日期的数据源(台账)不受时区影响。

## 10. 里程碑

| 阶段 | 内容 | 状态 |
|---|---|---|
| **M0** | 脚手架、model/trait 定义、Claude Code Provider、静态仪表盘 | 已完成 |
| **M1** | watcher + 增量解析、SQLite 快照、托盘今日总量 | 已完成 |
| **M2** | DSH(ledger + projcache + 会话日志)、Codex、ZCode | 已完成 |
| **M3** | OpenCode、Pi、Cursor;会话明细页、图表、设置页 | 已完成 |
| **M4** | 定价表与统一成本口径、自定义 Agent、NSIS 安装包 + 便携 zip、单实例 | 已完成(v0.1.7) |
| **M5** | DSH per-file 缓存、刷新链收敛、容错加固、CSP、健康提示 | v0.1.8 |
| **M6** | 开机自启(需引入 tauri-plugin-autostart)、更多候选 Agent | 待排期 |

## 11. 风险与对策

| 风险 | 对策 |
|---|---|
| 各家落盘格式无官方契约,版本升级会漂移 | Provider 完全隔离;宽容解析(默认值 + 忽略未知字段);内联 fixture 单测锁行为;解析失败只降级不崩溃 |
| 首次全量扫描面对数百 MB JSONL | offset 游标增量;DSH 按文件缓存解析结果;全量扫描放后台线程 |
| 重复计数(Claude 流式重复、ZCode transcript 多事件、Codex 派生会话) | 每个 Provider 内置去重规则并用 fixture 测试;跨扫描用 `jsonl_util` 的 DEDUP_WINDOW |
| 读 SQLite(WAL)读到中间态 | 只读 + immutable 打开,或读快照副本 |
| Agent 清理/轮转历史文件 | 自持 SQLite 快照,曲线不断;游标文件消失时重置该 provider;会话日志按 (size, mtime) 失效缓存 |
| 后台线程 panic 导致刷新永久静默失效 | `crate::lock` 容忍中毒 + `catch_unwind` 隔离 + Provider 健康提示上报 UI |
| 计量口径争议(cache/reasoning) | 分列展示不加总,UI 注明口径:`total = input + output + cache_read + cache_write` |
| 定价表过期导致成本失真 | 定价表是唯一权威,但每行标注来源(官方/手动/未知),同步时对已手动设置的价格先询问 |

## 12. 新增一个 Provider 的步骤(SOP)

1. 本机找到该 Agent 的数据目录,确认格式(JSONL / SQLite / 压缩包),选最小、最聚合的数据源
2. 在 `providers/` 新建 `xxx.rs`,实现 trait;能复用 `jsonl_util` 就复用;
   有自带成本就声明 `native_cost_currency()`
3. 在同一个文件里写 `#[cfg(test)] mod tests` + 内联 fixture(含边界:空文件、半行、去重、截断)
4. `providers/mod.rs` 注册一行 + `watch_paths` 给出监听目录
5. 手动验证:`rescan` 后 Dashboard 出现新 Agent 卡片,数字与官方/第三方统计(ccusage 等)对得上;
   再跑一次 `cargo test --release --test scan_real -- --ignored --nocapture` 看记录数与 token 总量
