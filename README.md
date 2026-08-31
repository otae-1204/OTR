<div align="center">
  <p>
    <img src="src-tauri/icons/128x128.png" width="88" height="88" alt="OTR">
  </p>

  <h1>
    Otae's<br>
    <em>Token Radar</em>
  </h1>

  <p>
    <img src="https://img.shields.io/badge/license-MIT-lightgrey?style=flat-square" alt="License">
    <a href="https://github.com/otae-1204/OTR/releases/latest"><img src="https://img.shields.io/github/v/release/otae-1204/OTR?style=flat-square&label=release&color=orange" alt="Release"></a>
    <img src="https://img.shields.io/badge/Rust-dea584?style=flat-square&logo=rust&logoColor=black" alt="Rust">
    <img src="https://img.shields.io/badge/Tauri-24C8DB?style=flat-square&logo=tauri&logoColor=white" alt="Tauri">
    <img src="https://img.shields.io/badge/React-61DAFB?style=flat-square&logo=react&logoColor=black" alt="React">
  </p>
</div>

---

**中文** · [English](README.en.md)

OTR(**O**tae's **T**oken **R**adar)是你电脑上所有 AI Coding Agent 的 Token 消耗雷达:收进一个常驻托盘的桌面应用。无需在各个 Agent 的日志目录之间来回翻找,本地解析、纯离线、零侵入。

## 支持的 Agent

| Agent | 数据源 | 说明 |
|---|---|---|
| **DSH** | `~/.dsh/storages/` | 按天台账 + 会话明细,自带成本 |
| **Claude Code** | `~/.claude/projects/` | 按 message.id + requestId 去重 |
| **Codex CLI** | `~/.codex/sessions/` | 逐事件解析 last_token_usage |
| **ZCode** | `~/.zcode/cli/` | model-io + transcript 双源,按 requestId 去重 |
| **OpenCode** | `~/.local/share/opencode/opencode.db` | SQLite 只读 |
| **Pi** | `~/.pi/agent/sessions/` | 自带美元成本 |

更多 Agent 可在设置页**自定义**(内置 Claude Code / Codex / ZCode 三种布局解析器,指向任意数据目录即可)。

## 功能

- **托盘常驻**:tooltip 实时显示今日总量;文件变化自动增量刷新
- **仪表盘**:今日/近7天/近30天/本月/自定义范围;单 Agent 视图;按小时/天/月自适应的趋势刻度
- **模型占比**:点击扇区查看该模型明细,含缓存命中率
- **成本定价**:models.dev 一键获取 + 手动编辑 + 汇率换算
- **本地持久化**:SQLite 快照,Agent 清理日志文件也不影响历史曲线

## 下载 & 安装

前往最新 [Releases](https://github.com/otae-1204/OTR/releases/latest):

- `OTR_x.x.x_x64-setup.exe` — 安装版(NSIS)
- `OTR_x.x.x_x64-portable.zip` — 便携版,解压即用(需系统已有 WebView2)

## 开发

```bash
npm install
npm run tauri dev     # 开发(端口 5174)
npm run tauri build   # 打包 NSIS 安装包
npm run build:portable # 打包便携版 ZIP
npm run build:release  # 同时生成 NSIS 安装版和便携版
```

要求:Node 18+、Rust 1.77+、Windows(WebView2)。

- 前端在 `src/`,后端在 `src-tauri/`
- 完整架构设计见 [ARCHITECTURE.md](./ARCHITECTURE.md)
- 后端命令:`list_agents` / `get_summary` / `get_range_summary` / `get_daily` / `get_sessions` / `rescan` 等;数据变化推送事件 `usage://updated`

## 统计口径

`total = 输入 + 输出 + 缓存读 + 缓存写`(reasoning 已含在输出内,单独展示不加总)。成本优先使用 Agent 自带数据,其余按设置页定价估算。

## 致谢

- [CC-Switch](https://github.com/farion1231/cc-switch) — 前端风格与 Pi 图标的参考来源
- [models.dev](https://models.dev) — 模型定价数据
- [simple-icons](https://simpleicons.org) — Claude / OpenAI / OpenCode 品牌 path

## License

[MIT](./LICENSE)
