# OTR 交接说明

给下一个 Agent。先读这一份，再按需要下钻。深度设计在 [`ARCHITECTURE.md`](ARCHITECTURE.md)（主体仍对齐 0.3.2；0.3.3 的登录取 Cookie 只写在这里），主题在 [`docs/theme_interface.md`](docs/theme_interface.md)。

仓库：<https://github.com/otae-1204/OTR>。本地目录名仍是 `Token-Show`，产品名是 **OTR**（Otae's Token Radar）。曾用名 Token-Show。包名、crate 名都是 `otr`，应用标识 `com.otae.radar`。

## 1. 现在停在哪

2026-10-21 交接时：

- 分支 `main`，工作区干净，已与 `origin/main` 同步。
- `HEAD` = `d548013`（`feat: Grok Build 使用官方标志`）。
- 版本号 **0.3.3**，写在 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。安装包已在本地打出。代码已提交，**没有 tag**。
- 0.3.3 比已发布的 0.3.2 多三件事：`d548013` 的 Grok 官方标志，StepFun / 千问的登录窗自动取 Cookie，以及 StepFun / Grok 的登录态自动续期。
- GitHub Release [v0.3.2](https://github.com/otae-1204/OTR/releases/tag/v0.3.2) 的 tag 仍指向 `786afef`，不包含上面两件。用户明确说发版之前，不要自己打 tag。
- 本地 `git describe` 若仍显示 `v0.3.1`，是 tag 没 fetch。远程 tag：`v0.3.0` → `8a882ec`，`v0.3.1` → `fe20499`，`v0.3.2` → `786afef`。

没有 CI。发版是本地 `npm run build:release`，再手动建 GitHub Release（NSIS + 便携 zip）。

## 2. 这是什么

本机所有 AI Coding Agent 的 Token 消耗雷达。Tauri 2 托盘常驻，只读本地日志，SQLite 自己留快照。额度页另走网络，问各家还剩多少。

明确不做：切换 Provider、改 Agent 配置、代理或拦截 API、遥测、开机自启（`tauri-plugin-autostart` 没引进；设置里的「启动时最小化」只是 `window.hide()`）。

技术栈：Rust edition 2021 + Tauri 2 + React 18 + TypeScript + Vite + Tailwind + Recharts。前端类型手写在 `src/api/bindings.ts`，没有 tauri-specta。SQLite 用 `rusqlite` bundled。Windows 凭据走 keyring；macOS 走钥匙串；Linux 不引入 keyring，额度凭据只认环境变量。

窗口：Windows 无边框，标题栏按钮在 `src/components/WindowControls.tsx`。macOS 用 `src-tauri/tauri.macos.conf.json` 的 overlay 标题栏，系统红绿灯，打包目标 `.app` / `.dmg`。便携 zip 只在 Windows 生成。

## 3. 先看这些文件

| 要做的事 | 先读 |
|---|---|
| 改某个 Agent 的用量算法 | 该文件顶部的模块注释，再看同文件 `#[cfg(test)]` |
| 改额度 | `src-tauri/src/limits/mod.rs`，再看对应的 `limits/<provider>.rs` |
| 改成本数字 | `store.rs` 的 `CostBasis`，以及 `pricing.rs` / `peak.rs` |
| 改刷新、扫描、托盘 | `lib.rs` 的 `run_scan` / `RescanGate`，`watcher.rs`，`tray.rs` |
| 改界面结构 | `src/App.tsx`（仪表盘 / 额度 / 设置，没有 `pages/`） |
| 改主题 | `docs/theme_interface.md`，`src/theme/`，`docs/theme.schema.json` |
| 加一个用量来源 | `ARCHITECTURE.md` 第 13 节，外加本文第 6 节 |

数据在 Tauri 的 app data dir。Windows 上是 `%APPDATA%\com.otae.radar\`：`radar.db`、`settings.json`、用户主题目录 `themes\`。启动时若没有 `radar.db`，会从旧标识 `com.otae.app` / `com.token-show.app` 拷一次（`otae.db` 或 `token-show.db`）。

## 4. 还没写进架构文档的地方

`ARCHITECTURE.md` 已对齐到 0.3.2，包括千问额度、凭据分片、周期对比、时段图和当前 `PARSER_VERSION`。

还剩一句：`docs/theme_interface.md` 开头仍写「对应 OTR 0.2.x」。主题格式就是 `apiVersion = 1`，正文可以信，开头那句版本号过时。

## 5. 不能改错的口径

这些是已经踩过、并用测试钉住的。改相关代码时先读对应注释，再动公式。

**用量合计：** `total = input + output + cache_read + cache_write`。`reasoning` 只展示，不加进合计。每条 `UsageRecord` 是增量，不是累计值。

**成本：** 定价表是唯一权威，实现在 `store::CostBasis`。有定价就按定价重算并覆盖自带成本；没有定价才用 Agent 自带成本，按 `native_cost_currency()` 换算（DSH 是人民币原值；Cursor / Pi / OpenCode / Grok 是美元乘汇率）。大卡和会话明细必须走同一套。峰谷（DeepSeek）只改单价，不改 token 数；token 以 `usage_daily` 为准，禁止用 `usage_hourly` 出总额（本机 hourly 会少一截近期数据）。时区换算在 Rust 里做。

**两表本来就可能对不上：** `usage_daily` 是范围内发生的用量；`sessions` 是范围内活跃的会话，金额是该会话的完整累计。`skip_daily` 的记录只进会话表。这不是 bug。

**刷新：** 只有真写入用量才递增 kv `data_version`。空扫描不要动版本号，否则前端会跟着 watcher 的 2 秒节拍白刷。`refresh_limits` 必须是 async 命令，里面用 `spawn_blocking`。同步命令跑在窗口线程上，一家连不通就能把 UI 冻住。`ureq` 的连接超时要单独设 `timeout_connect`（现在 5 秒），只设 `.timeout()` 挡不住默认 30 秒。

**额度显示：** 读不到的字段保持 `None`，界面显示 `--`，不要画 0% 进度条。Cursor 百分比有固定优先阶梯，禁止用 `used/limit` 当主口径。Codex 窗口按 `limit_window_seconds` 分类，不按槽位名。StepFun 的 `*_left_rate` 是剩余率；`Oasis-Token` 的值是 `<访问JWT>...<刷新JWT>`，三个点是格式的一部分，拆开必 401。Grok 的 `creditUsagePercent` 是已用率；`productUsage` 是同一周池的拆分，只有一个产品且百分比相同就不要再画一格。

**登录取 Cookie：** 设置里 StepFun / 千问的「自动获取」在教程旁边。命令是 `capture_console_cookie`，实现在 `limits/login.rs`。每个来源用临时目录里的独立 WebView，不碰用户浏览器。StepFun 只存能通过 `extract_journal` 的 `Oasis-Token`；千问整段必须含 `login_aliyunid_ticket`，其余只留额度请求会读的名字。写入走该账号已有的凭据条目。关窗、超时、形状不对都不覆盖旧值。命令返回值只有状态句，Cookie 和 JWT 不返回、不写日志。Windows 上 `cookies()` 仍在独立线程读。

**凭据边界：** Tauri 命令只返回「在不在」，永不返回密钥值。OTR 只读 Codex 的 `auth.json` 和 Cursor 的 `state.vscdb`。Grok 的访问令牌还有效时也不改文件；到期前 5 分钟内，用同一条的 `refresh_token` 向 `oidc_issuer` 发现的 token 端点续一次，成功后只替换该账号的 `key`、`refresh_token`、`expires_at`。StepFun 查询返回凭据失效时续一次，成功后只替换原凭据里的 `Oasis-Token`。两边失败都不覆盖旧值。

**扫描隔离：** 用 `crate::lock`，不要 `unwrap()` 一把可能被后台线程毒化的锁。`run_scan` 和额度循环都包了 `catch_unwind`。`RescanGate` 保证同时只有一个扫描线程。

**Grok 用量不要双重计算：** 只认 `updates.jsonl` 里 `turn_completed` 的增量。不要读 `unified.jsonl` 或 `signals.json`。子会话整段跳过。同一 `prompt_id` 是快照，后写更大只补差额。`inputTokens` 是否已含缓存，要按文件顶部注释的条件判断，不能一律相减。

## 6. 怎么加东西

**新的用量 Agent：** `providers/` 新文件，实现 `AgentProvider`，在 `providers/mod.rs` 的 `all_providers()` 注册，在 `settings.rs` 的 `BUILTIN_AGENTS` 加 id。复用 `jsonl_util`。有自带成本就实现 `native_cost_currency()`。同文件写内联 fixture 测试。改解析语义就抬 `PARSER_VERSION`。自定义 Agent 已能复用 Claude Code / Codex / ZCode 三种布局，指向任意目录；CodeBuddy 可以走这条，不必先做专用 Provider。

候选、本机出现过但还没做专用解析：`.codebuddy` / `.codebuddycn`、`.kimi-code`、`.copilot`、`.qoder-cli`。一次只加一个。Gemini CLI / Qwen Code 的用量解析没有排期；千问只做了额度。

**新的额度来源：** 放 `limits/`，不要塞进 `AgentProvider`。用量解析保持零网络。接进 `limits/mod.rs` 的分发，以及 `settings.rs` 的 `LIMIT_PROVIDERS`。新来源用 `known_limit_providers` 做「只自动打开一次」。没有凭据就走不发网络的快路径。一家失败不要清空其它家的缓存，也不要拿内置账号的数字冒充。

## 7. 本地怎么跑、怎么验

要求 Node 18+、Rust 1.77+。Windows 需要 WebView2。Shell 是 Windows PowerShell 5，**不支持 `&&`**。

```powershell
npm install
npm run tauri dev          # 开发，Vite 在 5174
npm run typecheck
npm run test:theme         # 主题 CSS 校验器
npm run test:range         # 日期区间
npm run tauri build        # NSIS
npm run build:portable     # 便携 zip
npm run build:release      # 两者
```

Rust 测试在各 Provider 文件里。真实数据 / 真实网络的验收测试带 `--ignored`，会读本机 Agent 目录或打外网，不要在无关改动里当默认套件跑：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --release --test scan_real -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --test limits_live -- --ignored --nocapture
```

改界面时用 `npm run tauri dev` 把受影响的路径点一遍（仪表盘筛选、趋势图、额度卡、设置）。这是桌面 WebView，不是浏览器里的独立站点。用户习惯看一版编出来的程序，常会说「重新编译」。

发版时三处版本号一起改（`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`；锁文件会跟着变）。没有 GitHub Actions。用户说推送或发版之前，不要自己 push，也不要自己打 tag。

## 8. 和用户协作

用户用中文下任务，代码注释也是中文，注释写「为什么」，尤其是容易写反的口径。

常见节奏：先要方案，确认后再实现；做完要一版能打开的程序看效果；满意后再说换版本号、推送、发 Release。范围会中途改准（Grok 先被说成订阅，实际先要的是 Grok Build 用量，订阅是下一问）。视觉上讨厌卡片被撑高、行高不齐、图标不是官方的。用量数字要对得上同类工具或官方字段说明，不要只凭字段名猜测。

未提交的改动留在工作区，等用户明确说要提交再提交。不要顺手做开机自启、新 Agent、或文档大重构，除非这一轮的任务就是那个。

## 9. 还没做

- 开机自启。
- CodeBuddy / Kimi / Copilot / Qoder 的专用用量解析。
- Linux 系统凭据库。
- 把本地 0.3.3（含 `d548013` 和登录取 Cookie）提交、推送并打 Release。用户还没让做。
- `docs/theme_interface.md` 开头的版本句仍写 0.2.x。改主题时顺手改掉即可。
