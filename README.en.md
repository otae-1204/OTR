<div align="center">
  <p>
    <img src="src-tauri/icons/128x128.png" width="88" height="88" alt="OTR">
  </p>

  <h1>
    Otae's<br>
    <em>Token &amp; Agent Radar</em>
  </h1>

  <p>
    <img src="https://img.shields.io/badge/license-MIT-lightgrey?style=flat-square" alt="License">
    <img src="https://img.shields.io/badge/Rust-dea584?style=flat-square&logo=rust&logoColor=black" alt="Rust">
    <img src="https://img.shields.io/badge/Tauri-24C8DB?style=flat-square&logo=tauri&logoColor=white" alt="Tauri">
    <img src="https://img.shields.io/badge/React-61DAFB?style=flat-square&logo=react&logoColor=black" alt="React">
  </p>
</div>

---

[中文](README.md) · **English**

OTR (**O**tae's **T**oken **R**adar) is the radar for the token consumption of every AI coding agent on your machine into a single tray-resident desktop app. Local parsing, fully offline, zero intrusion.

## Supported Agents

| Agent | Data source |
|---|---|
| **DSH** | `~/.dsh/storages/` (daily ledger + per-session, built-in cost) |
| **Claude Code** | `~/.claude/projects/` (dedup by message.id + requestId) |
| **Codex CLI** | `~/.codex/sessions/` (per-event last_token_usage) |
| **ZCode** | `~/.zcode/cli/` (model-io + transcript, dedup by requestId) |
| **OpenCode** | `~/.local/share/opencode/opencode.db` (SQLite read-only) |
| **Pi** | `~/.pi/agent/sessions/` (built-in USD cost) |

Custom agents are supported: point any of the three built-in parsers (Claude Code / Codex / ZCode layout) at an arbitrary data directory from the settings page.

## Features

- Tray-resident with live "today" totals; incremental refresh on file changes
- Dashboard: today / 7d / 30d / month / custom range, per-agent view, hourly/day/month adaptive trend granularity
- Model share donut with click-through details and cache hit-rate
- Cost pricing: one-click fetch from models.dev, manual editing, exchange rate
- SQLite snapshots — history survives agent log cleanup

## Development

```bash
npm install
npm run tauri dev
npm run tauri build
```

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the full design doc.

## Acknowledgements

[CC-Switch](https://github.com/farion1231/cc-switch) · [models.dev](https://models.dev) · [simple-icons](https://simpleicons.org)

## License

[MIT](./LICENSE)
