# OTR 项目架构

> 只读展示本机所有 AI Coding Agent 的 Token 消耗(类似 CC-Switch,但只做"看",不做"切"。曾用名 Token-Show)。
>
> 本文与代码对齐到 **0.3.2**(2026-10-21)。GitHub Release `v0.3.2` 的 tag 在 `786afef`;`main` 上还多一次提交 `d548013`(Grok 官方标志),那一笔还没打进安装包。仓库状态见 [HANDOVER.md](HANDOVER.md)。

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
│  dsh │ claude_code │ codex │ zcode │ opencode │ pi │ cursor │ grok │
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

注册表在 `providers/mod.rs`;用户在设置里启停。新增一个 Agent = 新增一个文件 + 注册一行,
并把它加进 `settings.rs` 的 `BUILTIN_AGENTS`。没见过的内置 Agent 会自动启用一次
(`known_agents`),用户之后关掉不会在下次启动时复活。

**当前解析版本**(`PARSER_VERSION`;改解析语义必须加一,否则旧游标让新逻辑只看见增量):

| Agent | 版本 | Agent | 版本 |
|---|---|---|---|
| DSH | 5 | Cursor | 2 |
| Codex | 3 | OpenCode | 2 |
| ZCode | 3 | Grok | 2 |
| Claude Code | 1 | Pi | 1 |

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
   (dsh = ¥ 原值;cursor / pi / opencode / grok = $ × 汇率);
3. 都没有 → 0。

输出恒为 ¥;`RangeSummary.currency` 只是"前端该按哪个币种展示"的提示,
前端 `fmtCost` 再按 `settings.currency` 换算。

`range_summary`(顶部大卡/模型占比)与 `sessions`(明细表)**共用同一套 CostBasis**。
历史上两处口径不同(明细表完全不查定价表、只按 `agent == "dsh"` 特判乘汇率),
导致两处数字系统性对不上。

**峰谷分时计价**(DeepSeek 等):`PriceEntry.peak` 是可选的`PeakTier`
(input / output / cache_read / cache_write)。有它时,某类 token 的单价按
`(1-p) × 平价 + p × 高峰价` 加权,`p` 是该行落在高峰时段的比例。

- **token 数仍以 `usage_daily` 为准**,峰谷只改单价不改数量 —— 所以加峰谷前后
  日总额的 token 数逐位一致(有单测锁住这一点)。
- `p` 从 `usage_hourly` 算:按 `(date, model, provider)` 加权聚合,取每个桶的
  四类 token 峰值占比(`peak::PeakShares`);区间聚合(不逐日)时回退到该模型的
  整体占比。**本机 hourly 与 daily 差 8%**(hourly 缺近期日),所以绝不拿 hourly
  出总额。
- 时区换算在 **Rust 里用 chrono 做**(本地 date+hour → 北京时间 → 判工作日
  9-12 / 14-18 点),不在 SQL 里做:跨日边界 SQL 会算错。
- `peak == None` 时退化成原公式,**与加峰谷之前逐位相同**。

**DeepSeek 定价修正**:`pricing.rs` 内置一张 curated 表(flash 族
0.15/0.6/0.003/0.15、pro 族 0.66/1.98/0.022/0.66,高峰 = 平价 × 2),
`Settings::load` 时跑一次 `pricing::migrate`:

- 只改**逐位命中已知错误值**的条目(`KNOWN_WRONG`,用 `f64::to_bits` 比);
- 平价已对但缺 `peak` 的只补 `peak`,三个平价一位不动;
- 用户自填的**非已知错误值一律保留**,同步 models.dev 时 `curated:` 开头的条目跳过冲突询问;
- 改过的标 `pricingSource = "curated:deepseek"`。

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
| 解析要点 | 优先用 `last`(真实的一次请求用量);`last` 缺失或全 0 而 `total` 在涨时,**用累计值的增量兜底**(否则那一次调用会无声消失),累计基准只增不减。累计值没变的重复 `token_count`(限流重发)不记。两种口径的 `input_tokens` 都已包含 cached,统一减掉才是未缓存输入;reasoning 已含在 output 里,只单列。同 thread 的多份 rollout 只保留文件名最新的一份;带 `parent_thread_id` 的派生会话整段跳过(它们重放了父线程的完整 token_count 历史),跳过数量整轮只汇总成一行日志 |
| 自带成本 | 无(靠定价表) |

### 5.4 ZCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.zcode/cli/rollout/model-io-sess_*.jsonl`(主会话,清理快)+ `~/.zcode/cli/agents/<sess>/<agent>/transcript.jsonl`(子代理,留存较久) |
| 格式 | JSONL。用量以 camelCase `{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens}` 为准 |
| 解析要点 | `inputTokens` 已含缓存读和缓存写,两栏都减掉,缓存栏不超过 input。双源按 `requestId` 去重。transcript 一次调用会写多条带 usage 的事件,**只收白名单** `model_network_status` / `model_request_completed`(且 usage 里不带 `modelRequestCount`);`model_complete`(相同 usage、无 modelId、无 requestId)与 `turn_complete`(整轮汇总)丢弃。空模型不入库 |
| 本机实测 | transcript 里带 `payload.usage` 的 type 只有 `model_complete` / `model_network_status` / `turn_complete` 三种,`model_network_status` 是唯一同时带 modelId 与 requestId 的 |
| 自带成本 | 无(靠定价表) |

### 5.5 OpenCode
| 项 | 内容 |
|---|---|
| 数据源 | `~/.local/share/opencode/opencode.db`(SQLite) |
| 解析要点 | WAL 模式:优先只读打开,失败退回普通打开;`session` 表按会话给绝对值快照,与上次快照做 diff。`tokens_input` 与两种缓存是分开的栏,不互相扣减。官方合计另含 `tokens_reasoning`(不在 output 里),写入时并进 output,单列仍保留。按会话起始日计入按天表 |
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

### 5.8 Grok Build
| 项 | 内容 |
|---|---|
| 数据源 | `$GROK_HOME/sessions/<编码 cwd>/<session-id>/updates.jsonl`(`GROK_HOME` 缺省 `~/.grok`) |
| 格式 | JSONL。只认 `params.update.sessionUpdate = "turn_completed"` 的 `usage`,这是一轮的增量 |
| 解析要点 | `inputTokens` 在 ACP 日志里已含缓存,未缓存输入要减掉;若 `totalTokens` 表明缓存是另栏,或缓存比 input 还大,则不再减。`reasoningTokens` 已含在输出里,只单列。`modelUsage` 小于父级时补差额,大于父级时只留父级。同一 `prompt_id` 是快照,重复行和 fork 拷贝不累加,后写更大时只补差额。不读 `unified.jsonl` / `signals.json`(叠加以重计,上下文快照会漏计)。子会话整段跳过。`costUsdTicks / 1e10` 是美元 |
| 解析版本 | 2:模型拆分补差额,同一 `prompt_id` 的重复快照只记增量 |
| 自带成本币种 | USD |

### 5.9 候选扩展(本机已检测到,放路线图)
`.codebuddy` / `.codebuddycn`(CodeBuddy)、`.kimi-code`(Kimi CLI)、`.copilot`(Copilot CLI)、`.qoder-cli`(Qoder)。
实现顺序建议:每次只加一个,先 `detect()` 再摸数据格式。Gemini CLI / Qwen Code 的**用量解析**不排期。
千问只做了额度(见 §9),没有本地用量 Provider。
(CodeBuddy 已可通过设置里的"自定义 Agent"用 claude-code 布局接进来。)

## 6. 后端模块划分(src-tauri/src/)

```
src-tauri/
├── tauri.conf.json            # 窗口/打包/CSP;Windows 无边框(decorations: false)
├── tauri.macos.conf.json      # macOS:overlay 标题栏、系统红绿灯、.app/.dmg
├── Cargo.toml
├── src/
│   ├── main.rs                # 薄壳
│   ├── lib.rs                 # setup 编排 + run_scan + crate::lock + RescanGate
│   ├── model.rs               # UsageRecord / 视图结构 / 时间与格式化工具
│   ├── commands.rs            # Tauri 命令(见 §8)
│   ├── store.rs               # SQLite:快照写入、游标、CostBasis、历史查询
│   ├── settings.rs            # 用户设置:启停 agent、定价表、币种汇率、自定义 Agent、额度账号、主题 id
│   ├── themes.rs              # 用户主题目录的发现与读取(只做文件系统层;校验在前端)
│   ├── paths.rs               # 各 Agent 数据目录探测
│   ├── pricing.rs             # DeepSeek curated 定价表 + 一次性迁移
│   ├── peak.rs                # 峰谷时段判定 + 按桶的峰值占比
│   ├── tray.rs                # 托盘图标 / tooltip / 菜单
│   ├── watcher.rs             # notify + 2 秒防抖 → run_scan
│   ├── error.rs               # AppError / Result
│   ├── limits/                # 额度页(网络查询,与用量统计无关)
│   │   ├── mod.rs             # 统一模型 + 账号编排 + 内存缓存
│   │   ├── http.rs            # 认系统代理的 HTTP 层
│   │   ├── credential.rs      # keyring 封装(只暴露存在性)
│   │   ├── cursor.rs  codex.rs  grok.rs  deepseek.rs  qwen.rs  stepfun.rs
│   └── providers/
│       ├── mod.rs             # trait + 注册表
│       ├── jsonl_util.rs      # 按 offset 续读、跨扫描去重窗口
│       ├── dsh.rs  claude_code.rs  codex.rs  zcode.rs
│       ├── opencode.rs  pi.rs  cursor.rs  grok.rs  custom.rs
└── tests/
    ├── scan_real.rs           # 真实数据冒烟(cargo test --release --test scan_real -- --ignored --nocapture)
    ├── range_smoke.rs         # 真实库的成本/区间对照
    ├── dsh_repair.rs          # 按小时表校正按天表的端到端
    ├── pricing_peak_acc.rs    # 定价修正 + 峰谷计价在真实库上的验收
    ├── limits_live.rs         # 额度页真实网络验收(cargo test --test limits_live -- --ignored --nocapture)
    ├── limits_no_freeze.rs    # refresh_limits 必须异步:派发耗时与连接超时上限
    └── stepfun_live.rs        # StepFun 控制台接口的真实网络验收(ignored)
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
├── main.tsx / App.tsx           # 主窗口:仪表盘 / 额度 / 设置;无 pages/ 目录
├── api/bindings.ts              # 手写的类型与命令封装
├── hooks/useUsageData.ts        # summary + agents + settings;事件订阅 + 30s 轮询 + in-flight 去重
├── lib/
│   ├── format.ts  range.ts      # 格式化;日期区间与上一周期对比
│   ├── remote.ts                # GitHub 版本 + 实时汇率
│   └── limitsLayout.ts          # 额度卡拖拽排布(localStorage,不进 settings.json)
├── theme/                       # 主题接口(见 docs/theme_interface.md)
│   ├── builtin.ts               # 内置 otr + 蓝色大肥鱼(清单来自 examples/themes/blue-whale.json)
│   └── art/                     # 蓝色大肥鱼立绘
└── components/
    StatCard.tsx  AgentCard.tsx  TrendChart.tsx  HourProfile.tsx
    ModelPie.tsx  SessionTable.tsx  Settings.tsx  Limits.tsx
    WindowControls.tsx             # Windows 无边框标题栏按钮
    SelectMenu.tsx                 # 自绘下拉
    ThemeMascot.tsx  CookieGuide.tsx
    AgentIcon.tsx  icons.tsx  Skeleton.tsx
```

> 没有 `TrayPopup.tsx` / `useTrayPopup`。托盘只有 tooltip 与菜单,不做弹窗。

**窗口。** Windows 关掉系统标题栏,按钮在 `WindowControls.tsx`。macOS 用
`tauri.macos.conf.json` 的 overlay 标题栏,保留系统红绿灯。拖窗口时避开按钮、链接和输入框。

**仪表盘(0.3.1)。** 筛选是 Agent × 日期范围。`previousRange` 给出紧挨着的上一周期
(今日对昨日,本月对上月同期到同一天;「全部」没有上一周期),大卡用它写增减。
点模型扇区后,趋势图和时段图改看该模型;扇区「其他」不驱动这两张图。
趋势图可在折线与柱状之间切换,上一周期的标记跟着变。
`HourProfile` 把范围内各天的同一个钟点加总,固定 24 格,只表达「习惯落在几点」。
小时表可能缺近期日,这张图的合计不要拿去对按天总额。

**主题。** 内置两套:`otr`(默认,缺失 token 的最终回退)和蓝色大肥鱼。
用户主题放在数据目录的 `themes/`。单模式主题会临时锁住深浅,切回双模式主题后恢复偏好。
`docs/theme_interface.md` 开头仍写「对应 0.2.x」,正文按 `apiVersion = 1` 仍然可用。

**图标。** 知名品牌用官方 path 或位图。Grok 用 xAI 的 Logomark,深浅色各一张
(`src/assets/agents/grok-logomark-*.svg`),按当前主题切换,不改图形。

托盘:tooltip 常显「今日 X tokens」。菜单是打开主窗口、立即刷新、恢复默认主题、退出。
恢复默认主题只把 `themeId` 设回 `otr`、生效模式设回偏好,其余设置不动。

## 8. Tauri API 面

**Commands(前端拉取)**
- `list_agents() -> Vec<AgentStatus>`(是否检测到、是否启用、累计 tokens、健康提示)
- `get_summary() -> UsageSummary`(今日各 Agent 卡片 + `dataVersion`)
- `get_range_summary(agent?, from, to) -> RangeSummary`(Hero 大卡 + 模型占比)
- `get_daily(agent?, from?, to?, granularity?, model?) -> Vec<DailyUsage>`(趋势图,day/hour/month;`model` 为扇区点选)
- `get_hour_profile(agent?, from?, to?, model?) -> Vec<HourProfile>`(0–23 点的习惯分布,见 §7)
- `get_sessions(agent?, from?, to?, limit?) -> Vec<SessionUsage>`(明细表)
- `rescan(full?)`(手动扫描;有 pending 合并)
- `list_models() -> Vec<String>`(设置页定价表用)
- `get_settings() / save_settings(s)`(`save_settings` 不改额度账号和来源开关,那两样由下面的命令维护,避免设置页其它区块用打开时的快照把刚加的账号盖掉)
- `list_themes() / get_themes_dir()`(主题:枚举并读出 `<数据目录>/themes/` 里的清单文件,无参数、跳过符号链接、单文件限 256 KiB;解析与校验在前端 `src/theme/`,见 `docs/theme_interface.md`)

**额度页 Commands**
- `get_limits() -> Vec<ProviderLimits>`(读内存缓存,**不发网络**,供前端秒开)
- `refresh_limits(account?) -> Vec<ProviderLimits>`(强制拉取;指定账号时只刷那一个)
- `list_limit_accounts() -> Vec<LimitAccount>`(含内置那条、`builtin`,以及 `keyPresent` / `cookiePresent` 两个布尔值)
- `save_limit_account(account, apiKey?, consoleCookie?)`(一次写入账号和密钥;密钥不进设置文件、不出现在返回值;空密钥表示沿用已有条目;写入失败则账号不落盘)
- `delete_limit_account(id)`(删额外账号,并删掉它名下的 `limit.*` 凭据;内置账号删不掉)
- `set_limit_provider(provider, enabled)`(额度来源开关。StepFun 默认关;千问和 Grok 对旧设置各自动打开一次,见 §9)
- `list_limit_credentials() -> Vec<{name, present}>`(**只有内置条目的布尔值,永不返回密钥**)
- `save_limit_credential(name, secret) / delete_limit_credential(name)`(读写系统凭据库;名字必须是内置条目或 `limit.*` 生成名)
- `open_cookie_guide(provider)`(Cookie 教程窗口,只认 `qwen` / `stepfun`。必须是 async:同步建 WebView 会和事件循环互相等待,窗口停在白屏)

**Events(后端推送)**
- `usage://updated`(轻量信号,前端收到后重拉 summary)

## 9. 额度页(limits/)

**与用量统计是两回事**:用量是"读本地日志算 token",额度是"走网络问各家还剩多少"。
所以单独一个模块,不塞进 `providers/` 的 trait —— 后者是纯文件解析,零网络。

### 统一模型

```rust
QuotaWindow   { key, label, used_percent: Option<f64>, reset_at, window_seconds }
Balance       { amount, currency, cash?, voucher? }
ProviderLimits{ account_id, account_label, provider, configured,
                error?, plan_label?, windows, balance?, fetched_at }
```

### 四条必须编码进实现的正确性规则(每条都有单测)

1. **Cursor 的百分比有优先阶梯**。`totalPercentUsed` → `(auto+api)/2` →
   `apiPercentUsed` → `autoPercentUsed` → 最后才轮到 `used/limit`。
   实测 `used=2000, limit=2000, bonus=6639` 会算出 **100%**,而真相是
   `totalPercentUsed = 17.4525` —— 这两个字段是"价格/额度上限"语义,不是用量。
2. **Codex 的窗口按 `limit_window_seconds` 分类,不按槽位名**。免费档只发一个窗口
   且落在 `primary_window` 槽里,按位置读会把它当成"5 小时";实测它是
   `2592000`(30 天)。`18000`=5 小时、`604800`=每周、`2592000`=每月;
   都不匹配时按位置兜底并**保留原始秒数**。
3. **字段缺失即 `None`**,UI 显示 `--` 且**不渲染进度条**。
   绝不臆造 0 —— "0% 已用"和"读不到"是完全相反的两件事,空进度条 + "已用 0%"
   会让人以为额度全满,是反向误导。
4. **凭据写回只限续期成功的字段**。Codex 的 401 只提示再跑一次 CLI,不碰 `~/.codex/auth.json`。
   Grok 访问令牌到期前 5 分钟,用同一条的 `refresh_token` 续一次,成功后只替换该账号的 `key`、`refresh_token`、`expires_at`。
   StepFun 查询返回凭据失效时续一次,成功后只替换原凭据里的 `Oasis-Token`。两边失败都不覆盖旧值。

### 子窗口的排列规则(前端 `Limits.tsx`)

后端给 `windows` 的顺序**不能直接渲染**:Cursor 把套餐额度 `insert` 到 0 位、再把分道
百分比追加到末尾,StepFun 把「套餐有效期」`push` 到最后,Codex 按槽位顺序给。
照原样渲染时同一张卡片里「5 小时」可能排在「每月」后面,两张卡片的行序也对不齐。
前端统一按 `sortWindows()` 排一次,稳定排序、同档位内不调换后端顺序:

1. **有 `windowSeconds` 的用量窗口**,按窗口长度升序:5 小时 → 每周 → 每月;
2. **没有窗口长度的用量窗口**,按 `WINDOW_ORDER` 语义序:`plan` → `auto` → `api` →
   `credit` → `topup` → `balance`,认不出的 key 排在所有已知 key 之后;
3. **到期类窗口**(只有 `resetAt`、没有 `usedPercent`,如「套餐有效期」)**一律垫底**。

所有子窗口都占满同样的三行(名称行 / 进度条行 / 脚注行):缺百分比的行用一个空的
`h-1.5` 占位,到期行把日期放在名称行的数值位、脚注行写「到期日 + 倒计时」。
这样同一张卡片里各行的基线与高度一致,不会因为某行缺字段而矮一截。
余额(含充值/赠送)收在同一个圆角块里,不再把「充值/赠送」拆成卡片底部独立一行。

### 多账号:两种凭据模式

添加同类账号是一次操作:显示名 + 密钥或目录。id 和凭据条目名由 `prepare_account` 生成,界面不展示。

| 模式 | 适用 | 认证来源 | 多账号做法 |
|---|---|---|---|
| `home` | Cursor、Codex、Grok | 本地 CLI 会话 | 多份 profile 目录(Cursor 认 `--user-data-dir`,Codex 认 `CODEX_HOME`,Grok 认 `GROK_HOME`) |
| `key` | DeepSeek | 额度 API 直接吃 key | 同一次保存里把密钥写进系统凭据库 |
| `key` + `cookie` | Qwen、StepFun | 控制台 Cookie;StepFun 另有按量 Key | 两条凭据各自一条目,查询只用这个账号自己的名字 |

- **内置账号由代码合成**(默认 profile),始终存在且不可删 —— 那是"什么都没配"时的默认那条;
  用户添加的额外账号存在 `settings.limitAccounts` 里。
- StepFun 额外账号有两个字段:`secret_ref`(按量 API Key,可空)和 `cookie_ref`(控制台 Cookie)。
  查询只用这两条自己的名字,不再把 Key 拼成 `<name>_CONSOLE_COOKIE`。
- 不支持的 provider **明确报"暂不支持"**,不拿内置账号的数字冒充(否则用户以为配好了)。
- 每个账号独立查询、独立报错;**单个失败不牵连其它**。
- 删除额外账号时,只删它名下的生成条目,不动内置凭据。
- **新来源只自动打开一次。** `known_limit_providers` 记下用户已经见过的来源。
  千问、Grok 对旧设置文件各打开一次;用户关掉之后不会在下次启动时复活。
  StepFun 依赖逆向接口,默认关闭,必须显式勾选。
- 额度卡可以拖拽重排。只有「单列」的两张卡能并到同一行;
  排布存在 `localStorage["token-show-limits-layout"]`,不进 `settings.json`。

### 凭据

- service `com.otae.radar`。
- **内置条目**(只给内置账号用):`DEEPSEEK_API_KEY` /
  `QWEN_CODING_PLAN_API_KEY` / `QWEN_CONSOLE_COOKIE` /
  `STEPFUN_API_KEY` / `STEPFUN_CONSOLE_COOKIE` / `STEPFUN_CONSOLE_TOKEN`。
- **额外账号**的条目名固定为 `limit.<provider>.<id>.key` 和
  `limit.<provider>.<id>.cookie`。空串、`../`、自拟的 `DEEPSEEK_API_KEY_WORK` 一律拒绝。
- **Windows 凭据 blob 上限是 2560 字节**,密码按 UTF-16 存,ASCII 大约 1280 字符就写不进去。
  超长秘密拆成 `名字`、`名字#1`、`名字#2`…,读的时候再拼回去。百炼控制台 Cookie 经常超限。
- 解析顺序:**keyring → 环境变量**;空白值等于没有。
- keyring 按平台收窄:Windows 凭据管理器、macOS 钥匙串。Linux 不引入 keyring
  (会拖进整套 zbus),那边走环境变量。
- **命令永不返回密钥值**:对外只有 `{name, present}`,以及账号上的 `keyPresent` / `cookiePresent`。

### StepFun:默认关闭

两条独立来源,**合并到同一张卡**上:

| 来源 | 凭据 | 端点 |
|---|---|---|
| 按量余额 | 内置 `STEPFUN_API_KEY`;额外账号 `limit.stepfun.<id>.key` | `GET api.stepfun.com/v1/accounts` |
| 订阅额度 | 内置 `STEPFUN_CONSOLE_COOKIE`;额外账号 `cookie_ref` | ConnectRPC `.../Dashboard/{QueryStepPlanRateLimit,GetStepPlanStatus}` |

**鉴权是 `Cookie: Oasis-Token=<jwt>`,不是 `Authorization`。** 实测三种回应可以
区分得很干净:不带头 → `token is missing`;名字写错(如 `oasis_token`)→ 仍是
`missing`;名字对但值假 → `token is illegal`。该 cookie **不是 HttpOnly**,
所以浏览器 `document.cookie` 就能读到。用户可能粘整段 cookie、只粘那一段、
或只粘裸 JWT —— `cookie_header()` 三种都认(裸 JWT 靠"没有 `=` 或以 `eyJ` 开头"判定)。

**控制台是 ConnectRPC**,路径形如 `/api/<package>.<Service>/<Method>`。
早先代码写的 `/api/v1/<Method>` 实测已 **404**,正确的服务是 `Dashboard`。

字段名取自控制台 JS 里的 protobuf 描述(**不是猜的**):

```text
QueryStepPlanRateLimitResponse
  five_hour_usage_left_rate   2   weekly_usage_left_rate    5
  five_hour_usage_reset_time  3   weekly_usage_reset_time   6
  plan_credit_rate_limit     11   → subscription_credit_left_rate / credit_buckets
GetStepPlanStatusResponse
  subscription                3   → name(套餐名) / expired_at
```

⚠️ **`*_left_rate` 是"剩余率"不是"已用率"**。直接当已用会得到相反的结论
(用了 10% 会显示成"已用 90%")。统一走 `used_percent_from_left_rate()` 换算,
并兼容上游某天改成百分数口径(`>1` 时除 100,免得差 100 倍)。

### ⚠️ `Oasis-Token` 是**复合 journal**,那三个点是格式的一部分

这是最容易搞反的一处(我自己就搞反过)。控制台 `Oasis-Token` 的值形如:

```text
<访问令牌JWT>...<刷新令牌JWT>
```

**字面的三个点**对应 passport 的 `journal_b64`,不是"从聊天记录里粘进来的省略号"。
- 整条原样发 → 200
- 只发前半段(或拆开成单个 JWT)→ 一律 401 `token is illegal`

`extract_journal()` 因此**保留** `...`,并从中提取 `device_id` 用作
`Oasis-Webid` / `Oasis-Did`(两个头都要带)。

### 请求头(缺一不可)

```text
Oasis-Token: <整条 journal>
Oasis-Webid:  <device_id>     ← 与 Did 同值
Oasis-Did:    <device_id>
Oasis-appID:  10300
Oasis-Platform: web
```

### 续期

`RefreshToken` 必须用**同一条 journal**(两个 JWT 都在)去换,回包里
`refreshToken.raw` 要拼成新 journal 继续用;只拿刷新令牌单独换会 400
`oasis header is invalid`。新 journal **只留内存**,不写回文件。
401 时自动续期一次,失败才报错。

### 显示口径按 `plan_family` 分叉

```text
plan_family = 1  CODING 旧套餐 → 5 小时 / 每周 滚动窗口
plan_family = 2  TOKEN  新套餐 → Credit 月池
```

**必须按 family 只取该看的那一组**。拿错组会显示一堆假数字:实测 TOKEN 家族下
`five_hour_usage_left_rate` 恒为 0,换算出来是"已用 100%"这种荒唐值。
另外上游把"没有该窗口"填成 `"0"`,要当作 None,否则会显示 1970 年的倒计时。

订阅额度是**逆向接口**,官方可能随时改动,所以默认关闭、要用户显式勾选,
且失败只影响该卡。字段认不出时把响应键名报给用户,便于下次修正映射。
两条来源中任一拿到数据就算成功 —— 一个失败不该把另一个已有数字染成红色报错。

### Qwen:百炼控制台 Cookie

官方文档只让用户到 Coding Plan 页面上看用量,没有公开 REST。那个页面实际调的是
`queryCodingPlanInstanceInfoV2`。`sk-sp-` Key 打上去得到 `ConsoleNeedLogin`,
查得到数字的是浏览器登录态(Cookie + 页面里的 `sec_token`)。

编排在 `fetch_qwen`:配了 Cookie 且查出窗口就用 Cookie;Cookie 没有数字时才退回 Key,
并告诉用户去贴 Cookie。没有凭据走不发网络的快路径。

| 形态 | 接口 |
|---|---|
| Coding Plan 套餐实例 | `zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2` |
| 个人版 Token Plan Solo | 套餐实例列表为空时,改走 `tokenplan/personal` 的 `usage` / `subscription` |

国内走 `bailian.console.aliyun.com` / `bailian-cs`,国际走
`modelstudio.console.alibabacloud.com`。Cookie 里有 `login_aliyunid_ticket` 时只打国内;
否则两处都试,国内在前。字段缺失保持 `None`。
这是逆向接口,失败只影响该卡。

### Grok:本地 CLI 会话

和 Codex 同一类,不贴 Cookie。读 `$GROK_HOME/auth.json`(缺省 `~/.grok/auth.json`)里的 `key`,
请求 `cli-chat-proxy.grok.com` 的 `/v1/billing?format=credits` 和 `/v1/user?include=subscription`。
头要带 `Authorization: Bearer` 和 `x-grok-client-mode: cli`。

`creditUsagePercent` 是**已用率**。`currentPeriod.type` 决定「每周 / 每月」,
`end - start` 写入 `windowSeconds`。`productUsage` 是同一周池的拆分,不是第二个额度。
只有一个产品、百分比又和周池相同(只用 Grok Build 时常见)时不再另画一格;
两个以上产品都有用量才分列,0% 的产品不显示。`prepaidBalance.val` 是美分,大于 0 才显示成美元余额。
`onDemandCap.val > 0` 才加「按量」行。没有周池百分比时,才用旧的 `monthlyLimit` /
`used`(同样是美分)。

**不刷新、不写回** `auth.json`。JWT 已过期或 401/403 只提示运行一次 `grok`。
没有登录态不发网络。套餐名接口失败不影响已经拿到的额度数字。

### 代理(踩过的坑)

实测本机开着本地代理(`HKCU\...\Internet Settings` 的 `ProxyServer`),
`cursor.com` / `api.deepseek.com` 直连能通,但 **`chatgpt.com` 直连必超时**。
而 `ureq` 不像 curl / .NET 那样自动读系统代理 —— 于是 Codex 的额度永远拉不到,
报出来还是"网络超时"这种看不出原因的错。

`limits/http.rs` 因此按 **环境变量代理 → 系统代理 → 直连** 依次尝试,
并且**直连永远保留为最后一档**(代理挂了不该把本来能通的家一起拖死);
服务器一旦有响应(哪怕 401)就立刻返回,不再换代理重试。

### 刷新

- `AppState.limits` 是**内存缓存**,不落盘(额度是易失数据,重启重拉即可)。
- 独立线程按 `settings.refreshSecs`(默认 60)刷新,**不挂在 watcher 的 2 秒 tick 上** ——
  网络 I/O 进热路径会拖慢文件监听,而额度也不需要秒级新鲜度。
- 刷新失败**保留上一次成功的数据**并挂上 `error`,不清空缓存。
- 线程体包 `catch_unwind`,panic 不会把整条后台刷新带停。

### 不阻塞 UI(踩过的坑)

**`refresh_limits` 必须是 async 命令。** Tauri 把非 async 命令判为
`ExecutionContext::Blocking`,命令体是**内联在 IPC 调用里**执行的
(见 tauri-macros 的 `body_blocking`),即跑在窗口/事件循环那条线程上。
额度查询是阻塞式网络 I/O,一旦这样写,某家登录态失效或域名连不通时,
UI 会被冻住。耗时还会被三件事放大:

1. **连接超时**:ureq 的 `timeout_connect` **默认 30 秒,且优先于 `timeout()`**。
   只设 `.timeout()` 不起作用 —— 所以 `limits/http.rs` 显式设了
   `timeout_connect(5s)`,并用一个"1 秒超时打黑洞地址"的测试把它钉住
   (若默认值生效,该测试会耗掉 30 秒)。
2. **多账号串行**:耗时相加。
3. **代理逐档重试**:环境变量代理 → 系统代理 → 直连,再乘一遍。

改成 async 后命令体被 `respond_async` 派到 async 线程池;里面仍是阻塞调用,
所以用 `spawn_blocking` 而不是直接在 async 上下文里跑 —— 阻塞一个 tokio worker
会饿死其它任务。`tests/limits_no_freeze.rs` 从两个角度锁住这个修复:
派发耗时(<200ms,实测 ~1ms)与连接超时上限。

另外,**没有凭据时必须走"不发网络"的快路径**:Codex 读不到 `auth.json`
就直接返回 `unconfigured` + 引导文案,不去连 `chatgpt.com`
(`missing_login_returns_immediately_without_touching_the_network` 锁住)。

## 10. 时间与分桶(已知的一次性不连续)

日期/小时桶按**本地时区**计算(`model::local_date` / `local_hour`),
`UsageRecord.bucket_date` / `bucket_hour` 可以覆盖(DSH 台账、会话快照走这条)。

**这意味着**:换时区、跨 DST,同一批历史数据会被切到不同的日期/小时桶,
历史曲线可能出现一次**一次性不连续**(某天变多、某天变少)。已入库的行不会重算 ——
只有重新全量扫描才会按新时区重排。这是有意接受的取舍:本地时间才是用户看到"今天用了多少"
时的直觉口径,而 `bucket_date` 让带明确日期的数据源(台账)不受时区影响。

## 11. 里程碑

| 阶段 | 内容 | 状态 |
|---|---|---|
| **M0** | 脚手架、model/trait 定义、Claude Code Provider、静态仪表盘 | 已完成 |
| **M1** | watcher + 增量解析、SQLite 快照、托盘今日总量 | 已完成 |
| **M2** | DSH(ledger + projcache + 会话日志)、Codex、ZCode | 已完成 |
| **M3** | OpenCode、Pi、Cursor;会话明细页、图表、设置页 | 已完成 |
| **M4** | 定价表与统一成本口径、自定义 Agent、NSIS 安装包 + 便携 zip、单实例 | 已完成(v0.1.7) |
| **M5** | DSH per-file 缓存、刷新链收敛、容错加固、CSP、健康提示 | v0.1.8 |
| **M6** | 额度页(Cursor/Codex/DeepSeek/StepFun)、DeepSeek 定价修正、峰谷分时计价、多账号 | 已完成(v0.2.0) |
| **M7** | 主题接口、受限自定义 CSS、示例主题、托盘恢复默认主题;无边框窗口、自绘下拉、蓝色大肥鱼内置主题 | 已完成(v0.3.0) |
| **M8** | 周期对比、点模型看趋势、趋势柱状图、使用时段 | 已完成(v0.3.1) |
| **M9** | Grok Build 用量、Grok 订阅额度、千问 Coding Plan 额度。Grok 官方标志在 `v0.3.2` tag 之后的 `d548013`,已在 `main`,尚未打进 Release | 已完成(v0.3.2) |
| **M10** | 开机自启(需引入 tauri-plugin-autostart)、更多候选 Agent 的用量解析 | 待排期 |

## 12. 风险与对策

| 风险 | 对策 |
|---|---|
| 各家落盘格式无官方契约,版本升级会漂移 | Provider 完全隔离;宽容解析(默认值 + 忽略未知字段);内联 fixture 单测锁行为;解析失败只降级不崩溃 |
| 首次全量扫描面对数百 MB JSONL | offset 游标增量;DSH 按文件缓存解析结果;全量扫描放后台线程 |
| 重复计数(Claude 流式重复、ZCode transcript 多事件、Codex 派生会话) | 每个 Provider 内置去重规则并用 fixture 测试;跨扫描用 `jsonl_util` 的 DEDUP_WINDOW |
| 读 SQLite(WAL)读到中间态 | 只读 + immutable 打开,或读快照副本 |
| Agent 清理/轮转历史文件 | 自持 SQLite 快照,曲线不断;游标文件消失时重置该 provider;会话日志按 (size, mtime) 失效缓存 |
| 后台线程 panic 导致刷新永久静默失效 | `crate::lock` 容忍中毒 + `catch_unwind` 隔离 + Provider 健康提示上报 UI |
| 额度接口直连不通(实测 chatgpt.com 必超时) | `limits/http.rs` 依次试 环境变量代理 → 系统代理 → 直连;ureq 不会自己读系统代理 |
| 额度接口改字段(尤其 StepFun 的逆向控制台 RPC) | 缺字段一律 `None` → UI 显示 `--`;**认不出的响应把原始键名报给用户**,绝不臆造数字;失败只影响该张卡 |
| 密钥经 Tauri 命令泄漏到前端 | 命令只返回 `{name, present}`;密钥值只存在于 `credential.rs` 内部,写入即离开内存;A9 测试对真实密钥逐个反查所有返回值 |
| 两个程序同时改同一个凭据文件 | OTR **只读** `~/.codex/auth.json` 与 Cursor 的 `state.vscdb`(后者以只读方式打开)。Grok 只在续期成功后替换该账号三个字段;StepFun 只替换原凭据里的 `Oasis-Token` |
| Windows 凭据写不下长 Cookie | blob 上限 2560 字节(UTF-16);超长秘密按 `名字#1`…分片,见 §9 |
| 计量口径争议(cache/reasoning) | 分列展示不加总,UI 注明口径:`total = input + output + cache_read + cache_write` |
| 定价表过期导致成本失真 | 定价表是唯一权威,但每行标注来源(官方/手动/未知),同步时对已手动设置的价格先询问 |

## 13. 新增一个 Provider 的步骤(SOP)

1. 本机找到该 Agent 的数据目录,确认格式(JSONL / SQLite / 压缩包),选最小、最聚合的数据源
2. 在 `providers/` 新建 `xxx.rs`,实现 trait;能复用 `jsonl_util` 就复用;
   有自带成本就声明 `native_cost_currency()`
3. 在同一个文件里写 `#[cfg(test)] mod tests` + 内联 fixture(含边界:空文件、半行、去重、截断)
4. `providers/mod.rs` 注册一行 + `watch_paths` 给出监听目录
5. 手动验证:`rescan` 后 Dashboard 出现新 Agent 卡片,数字与官方/第三方统计(ccusage 等)对得上;
   再跑一次 `cargo test --release --test scan_real -- --ignored --nocapture` 看记录数与 token 总量
