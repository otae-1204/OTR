# OTR 主题接口(Theme API)设计文档

> 版本:主题格式 `apiVersion = 1`;对应 OTR 0.2.x。
> 相关代码:`src/theme/`(前端)、`src-tauri/src/themes.rs`(后端)、`docs/theme.schema.json`(JSON Schema)、`scripts/theme-lint.mjs`(校验脚本)、`scripts/theme-contrast.mjs`(对比度检查)、`examples/themes/`(示例主题)。

## 1. 背景与现状分析

改造前的外观由四层东西拼起来,分散在代码各处:

| 层 | 位置 | 现状 |
|---|---|---|
| 语义色 | `src/index.css` 的 `:root` / `.dark` | shadcn 风格的 HSL 三元组 CSS 变量(`--background: 240 5% 12%`),`tailwind.config.js` 用 `hsl(var(--x))` 引用,深浅色靠 `<html class="dark">` 切换 |
| 状态色 / 指标色 | 各组件的 Tailwind class | 直接写死 `text-emerald-500`、`bg-amber-500`、`text-red-500`、`bg-orange-500`、`text-blue-500` 等十几处;hover 高亮写成 `hover:bg-black/5 dark:hover:bg-white/5`(30 处) |
| 图表颜色 | `src/api/bindings.ts` 的 `AGENT_COLORS` / `FALLBACK_PALETTE`,`ModelPie.tsx` 的 `PALETTE` | 十六进制常量,Recharts 的 `stroke` / `fill` / 渐变直接吃字符串;`AgentCard` 还用 `${color}1A` 拼透明度 |
| 字体 / 圆角 / 阴影 | `index.css` 的 `body { font-family }`,`tailwind.config.js` 的 `borderRadius.lg/xl` 固定值,阴影用 Tailwind 默认 | 全部硬编码 |

深浅模式的持久化:`localStorage["token-show-theme"]`(首帧防闪)+ `settings.json` 的 `theme` 字段(`"dark"` / `"light"`,由 `save_settings` 落盘,Rust 只存不读)。设置文件在 `app_data_dir()`(Windows 上是 `%APPDATA%\com.otae.radar\settings.json`),与 `radar.db` 同级。

CSP(`tauri.conf.json`)已经是 `style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:`,也就是说页面本来就不能从网络加载字体和图片,内联样式是允许的。

**结论**:变量化的底子(语义色 CSS 变量 + Tailwind 引用)已经有了,缺的是把散落的硬编码收进同一套 token、给 token 一个可从文件加载的清单格式、以及一条安全的加载/校验/回退链。

## 2. 设计目标

1. **一份 token 目录**:界面里所有可变的外观都对应一个有名字的 token;组件不再出现具体颜色。
2. **纯数据主题**:主题 = 一个 JSON 文件,没有 JS、没有任意 CSS。第三方能写、能分享、应用能安全加载。
3. **内置默认外观也是一个主题**:走完全相同的格式、校验与解析路径;选回它时与改造前逐位一致。
4. **永远能用**:任何一处失败(文件坏了、字段错了、主题被删了、JS 抛异常)都只影响那个 token 或那个主题,应用退回默认外观继续工作。
5. **可演进**:新增 token 不破坏旧主题;格式真的要变时靠 `apiVersion` 明确表达。
6. **最小改动**:不重构组件结构,只把硬编码换成 token;Rust 侧只加一个"枚举 + 读文件"的模块。

## 3. 主题可控内容(token 目录)

主题通过五组 token 控制外观。**颜色值一律要求不透明**(`#rrggbb`、`rgb()`、`hsl()`、或裸 HSL 三元组 `"240 5% 12%"`;不接受颜色关键字),原因见 §7。

### 3.1 `colors` —— 语义色(按模式给)

| token | CSS 变量 | 用途 | 默认(暗色) |
|---|---|---|---|
| `background` / `foreground` | `--background` / `--foreground` | 页面底色 / 正文 | `240 5% 12%` / `0 0% 93%` |
| `card` / `cardForeground` | `--card` / `--card-foreground` | 卡片底色 / 文字 | `240 5% 16%` / `0 0% 93%` |
| `popover` / `popoverForeground` | `--popover` / `--popover-foreground` | 浮层 | `240 5% 14%` / `0 0% 93%` |
| `primary` / `primaryForeground` | `--primary` / `--primary-foreground` | 主色(图标、选中态、「启动时最小化到托盘」开关、主按钮;也直接当**小号文字色**用:链接、角标)/ 主按钮上的文字,**同时也是设置页开关的滑块颜色**(见 §12.2) | `210 100% 56%` / `0 0% 100%` |
| `secondary` / `secondaryForeground` | `--secondary` / `--secondary-foreground` | 次级底色 | `240 5% 20%` / `0 0% 93%` |
| `muted` / `mutedForeground` | `--muted` / `--muted-foreground` | 弱化底色(分段控件轨道、进度条空轨)/ 次要文字、图表刻度 | `240 5% 20%` / `240 5% 65%` |
| `accent` / `accentForeground` | `--accent` / `--accent-foreground` | 强调底色 | `240 5% 20%` / `0 0% 93%` |
| `destructive` / `destructiveForeground` | `--destructive` / `--destructive-foreground` | 危险操作的**文字**与描边(全量重扫按钮、删除按钮 hover)、检查更新 / 汇率 / 价格拉取失败的错误文字 | `0 62% 45%` / `0 0% 100%` |
| `border` / `input` / `ring` | `--border` / `--input` / `--ring` | 边框、输入框边框、焦点环;图表网格线也用 `border` | `240 5% 24%` ×2 / `210 100% 56%` |
| `overlay` | `--overlay` | hover 高亮叠加的**基色**,界面按 5% 透明度叠加(`hover:bg-overlay/5`) | `0 0% 100%`(亮色下是黑) |
| `success` / `warning` / `danger` | `--success` / `--warning` / `--danger` | 状态色**填充**:额度进度条、缓存命中率条、设置页开关的「开」态轨道。注意 `success` / `warning` **也被直接当文字色用**(额度剩余百分比、「缓存命中率」标题、设置保存成功提示、定价表里「手动」来源标签、Agent 卡的 ⚠ 图标),要在 `card` 上可读;`danger` 只作填充 | emerald-500 / amber-500 / red-500 |
| `successText` / `warningText` / `dangerText` | `--success-text` / `--warning-text` / `--danger-text` | 状态色**文字**:放在 `card` 上,或放在同色 10–15% 淡底的角标上(默认亮色更深、暗色更浅) | emerald-400 / amber-400 / red-500 |
| `info` | `--info` | 信息色。**界面目前没有引用**(预留);「请求次数」指标用的是 `stat.calls`,只是默认值恰好相同 | sky-500 |
| `notice` | `--notice` | 「有新版本」小红点,以及角标的**文字**(底色是它自己的 15%) | orange-500 |

Tailwind 侧对应的 class:`bg-success` / `text-success-text` / `text-warning-text` / `text-danger-text` / `bg-notice` / `hover:bg-overlay/5` 等(见 `tailwind.config.js`)。

**目前界面没有引用的 token**:`cardForeground`、`popover` / `popoverForeground`、`secondary` / `secondaryForeground`、`accent` / `accentForeground`、`input`、`destructiveForeground`、`info`。它们会被校验、写成 CSS 变量,但当前没有组件使用 —— 例如卡片上的文字用的是 `foreground` 而不是 `cardForeground`。主题里写上它们无害(以后组件用到时自动生效),但改它们现在看不到变化(见待确认问题 Q12)。

### 3.2 `stat` —— 统计卡六项指标的强调色

| token | CSS 变量 | 默认 |
|---|---|---|
| `input` | `--stat-input` | blue-500 `#3b82f6` |
| `output` | `--stat-output` | purple-500 `#a855f7` |
| `cacheRead` | `--stat-cache-read` | emerald-500 `#10b981` |
| `cacheWrite` | `--stat-cache-write` | amber-500 `#f59e0b` |
| `calls` | `--stat-calls` | sky-500 `#0ea5e9` |
| `cost` | `--stat-cost` | green-500 `#22c55e`(会话明细表的成本列也用它) |

这六个颜色都是**文字色**:统计卡里 11px 的指标标签(底色是 `background` 40% 叠在 `card` 上),`cost` 还用于会话表的成本数字。选色时按正文对比度要求。

### 3.3 `chart` —— 图表调色板(JS 直接消费,归一化成 `#rrggbb`)

| token | 类型 | 用途 | 默认 |
|---|---|---|---|
| `palette` | 1–16 个颜色 | 模型占比环形图与图例按序取色 | 9 色(`#3b82f6`, `#a855f7`, `#10b981`, `#f97316`, `#f59e0b`, `#06b6d4`, `#ec4899`, `#84cc16`, `#64748b`) |
| `agents` | `{ agentId: 颜色 }` | 各 Agent 的品牌色:趋势图线与渐变、Agent 卡片图标底、筛选 chip 圆点、会话表圆点。**按 id 合并**:只写 `dsh` 就只改 DSH;也可以给自定义 Agent(如 `custom-codebuddy`)配色 | `dsh #8b5cf6`、`claude-code #f59e0b`、`codex #3b82f6`、`zcode #10b981`、`opencode #06b6d4`、`pi #ec4899`、`cursor #A3A3A3` |
| `agentFallback` | 1–16 个颜色 | 没配品牌色的 Agent 按 id 哈希从这里取一个固定颜色 | 6 色 |

它们同时也导出为 CSS 变量 `--chart-1…N` 与 `--agent-<id>`,供以后 CSS 使用。

### 3.4 `font` / `radius` / `shadow`

| token | CSS 变量 | 用途 | 默认 |
|---|---|---|---|
| `font.sans` | `--font-sans` | 全局字体(`body` 与 Tailwind `font-sans`) | `-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "PingFang SC", "Microsoft YaHei", sans-serif` |
| `font.mono` | `--font-mono` | `font-mono`(Cookie 教程里的路径/代码) | Tailwind 默认等宽栈 |
| `radius.md` / `lg` / `xl` | `--radius-md` / `--radius-lg` / `--radius-xl` | `rounded-md`(角标)/ `rounded-lg`(按钮、输入框、图标底、图表 tooltip)/ `rounded-xl`(卡片、分段控件) | `0.375rem` / `0.75rem` / `0.875rem` |
| `shadow.sm` / `base` / `md` / `lg` | `--shadow-sm` / `--shadow` / `--shadow-md` / `--shadow-lg` | `shadow-sm`(卡片 hover、分段选中)/ `shadow`(开关滑块)/ `shadow-md`(选中的 Agent 卡)/ `shadow-lg`(图表 tooltip) | Tailwind 3.4 默认值 |

### 3.5 明确**不允许**主题控制的内容

- **任何脚本**:清单是纯 JSON,不存在执行入口。
- **任意 CSS**:不提供 `css` / `stylesheet` 字段。只能改上面列出的 token,不能改选择器、布局、动画、滚动条。
- **间距、尺寸、断点、布局**(Tailwind spacing scale、`max-w-6xl`、栅格列数、图表高度):这些决定可用性与对齐,不开放(见待确认问题 Q3)。
- **图标、图片、字体文件**:不能引用任何 `url()`;CSP 也会拦截。品牌 logo 不可替换。
- **半透明的语义色**:界面自己会在语义色上叠透明度(`bg-primary/10`、`border-border/40`),token 带透明度会叠出错误结果,所以校验时拒绝。
- **窗口、托盘、CSP、原生行为**:与主题无关。
- **`rounded-full` 与 `--radius`(未使用的旧变量)**:不受主题影响。

## 4. 清单格式(manifest)

一个主题 = 一个 JSON 文件。顶层字段:

| 字段 | 必填 | 类型 | 说明 |
|---|---|---|---|
| `$schema` | 否 | string | 可指向 `docs/theme.schema.json`(在线地址见 §12.2),给编辑器补全用;应用忽略 |
| `apiVersion` | **是** | integer | 主题格式版本,当前必须是 `1` |
| `id` | **是** | string | 唯一 id:`^[a-z0-9][a-z0-9._-]{0,63}$`。不能用内置主题的 id(`otr`) |
| `name` | **是** | string | 展示名,1–64 字符 |
| `version` | 否 | string | 主题自己的版本,≤32 字符,仅展示 |
| `author` / `description` | 否 | string | ≤200 字符,仅展示 |
| `homepage` | 否 | string | `http(s)://` 开头,≤200 字符;**仅作文本展示,应用不会访问** |
| `tokens` | 否 | tokens 对象 | 与模式无关的公共 token(字体、圆角、图表色……颜色也可以放这里) |
| `modes` | **是** | `{ dark?: tokens, light?: tokens }` | 至少一个模式。`"dark": {}` 也合法,表示"提供暗色模式,值全部用默认" |

`tokens` 对象的结构在两处完全相同(公共 / 按模式),五个组:`colors`、`stat`、`chart`、`font`、`radius`、`shadow`,每个组的键见 §3。**所有 token 都是可选的**。

### 4.1 示例

<!-- theme-example:start -->
```json
{
  "$schema": "https://raw.githubusercontent.com/otae-1204/OTR/main/docs/theme.schema.json",
  "apiVersion": 1,
  "id": "ocean",
  "name": "Ocean",
  "version": "1.0.0",
  "author": "example",
  "description": "蓝绿色调的示例主题,提供深浅两种模式。",
  "tokens": {
    "font": {
      "sans": "\"Inter\", \"PingFang SC\", \"Microsoft YaHei\", sans-serif"
    },
    "radius": { "md": "0.5rem", "lg": "0.875rem", "xl": "1rem" },
    "chart": {
      "palette": ["#0ea5e9", "#14b8a6", "#6366f1", "#f59e0b", "#f43f5e", "#84cc16", "#a855f7", "#64748b"],
      "agents": { "claude-code": "#f59e0b", "codex": "#0ea5e9" }
    }
  },
  "modes": {
    "dark": {
      "colors": {
        "background": "#0b1220",
        "foreground": "#e2e8f0",
        "card": "#111a2e",
        "cardForeground": "#e2e8f0",
        "popover": "#0f172a",
        "popoverForeground": "#e2e8f0",
        "primary": "hsl(199 89% 48%)",
        "primaryForeground": "#ffffff",
        "secondary": "#1e293b",
        "secondaryForeground": "#e2e8f0",
        "muted": "#1e293b",
        "mutedForeground": "#94a3b8",
        "accent": "#1e293b",
        "accentForeground": "#e2e8f0",
        "destructive": "#be123c",
        "destructiveForeground": "#ffffff",
        "border": "#233047",
        "input": "#233047",
        "ring": "#0ea5e9",
        "overlay": "#ffffff",
        "success": "#2dd4bf",
        "successText": "#5eead4"
      },
      "shadow": {
        "md": "0 6px 16px -4px rgba(0, 0, 0, 0.5)"
      }
    },
    "light": {
      "colors": {
        "background": "210 40% 98%",
        "foreground": "222 47% 11%",
        "card": "#ffffff",
        "cardForeground": "222 47% 11%",
        "primary": "#0284c7",
        "primaryForeground": "#ffffff",
        "muted": "#e2e8f0",
        "mutedForeground": "#64748b",
        "border": "#cbd5e1",
        "input": "#cbd5e1",
        "ring": "#0284c7",
        "overlay": "#000000"
      }
    }
  }
}
```
<!-- theme-example:end -->

要点:

- 暗色模式给了完整的一套语义色,亮色只改了一部分 —— 没给的(如 `popover`、`secondary`)自动用默认亮色主题的值。
- `chart.agents` 只写了两个 Agent,其余 Agent 仍用默认品牌色。
- 颜色写法可以混用:`#hex`、`hsl()`、裸三元组。

## 5. 加载与发现流程

```
启动
 ├─ main.tsx: bootTheme()                     ← 同步,React 挂载前
 │    读 localStorage["token-show-theme"] → 切 .dark class
 │    读 localStorage["otr-theme-cache"]  → 若模式一致,把上次的 CSS 变量先刷上去(防闪)
 └─ <ThemeProvider> 挂载 → bootstrap()        ← 异步
      ├─ api.getSettings()      → themeId(默认 "otr")、theme(模式,设置文件是真相)
      ├─ api.getThemesDir()     → 显示给用户的目录(不存在则创建)
      ├─ api.listThemes()       → Rust 枚举 <数据目录>/themes/,读出每个文件的文本
      ├─ 内置主题 + 用户文件 → parseThemeFile() 校验 → ThemeEntry[](含诊断)
      ├─ decide(entries, themeId, mode)  → 找不到 / 无效 → 默认主题;不支持该模式 → 切到支持的模式
      └─ applyResolvedTheme()   → <html>.style 写全部 CSS 变量、切 class、color-scheme、写缓存
```

- **内置主题**:`src/theme/builtin.ts` 里的 TS 对象,随应用打包;它也经过 `validateManifest()` 归一化,与用户主题走同一条路。
- **用户主题目录**:`<app_data_dir>/themes/`(与 `settings.json`、`radar.db` 同级;Windows 典型路径 `%APPDATA%\com.otae.radar\themes\`)。设置页显示这个路径。
- **两种布局**都认:`themes/<name>.json` 或 `themes/<name>/theme.json`。后者留给以后放预览图等附件。
- **身份以清单里的 `id` 为准**,文件名只用于排序和校验失败时的展示。多个文件同一个 `id` 时,按文件名排序的第一个生效,其余报错 `id 重复`。
- **重新扫描**:设置页「重新扫描」按钮(`reload()`),不需要重启。
- **多窗口**:Cookie 教程窗口与主窗口共用 `localStorage` 与 `ThemeProvider`,首帧就是当前主题。
- **持久化**:`settings.json` 新增 `themeId`(字符串,默认 `"otr"`);`theme` 字段仍是深浅模式。两者由设置页在切换时一起保存;Rust 侧只存取,不解释。

## 6. 版本与兼容

- `THEME_API_VERSION = 1`(`src/theme/types.ts`)。清单里 `apiVersion` 必填、整数。
- **向后兼容(老主题在新应用)**:同一 `apiVersion` 内只做**加法**(新增 token、新增可选字段)。老主题缺新 token → 回退默认值;所以老主题永远能加载。
- **向前兼容(新主题在老应用)**:
  - 新主题只是用了老应用不认识的 token / 字段 → 报 warning、忽略该字段,其余照常生效。
  - 新主题声明了更高的 `apiVersion` → 老应用拒绝(error:「主题需要更新的 OTR」),回退默认主题。语义未知时宁可不加载,不做"尽力而为"。
- **什么时候升 `apiVersion`**:改变已有 token 的含义/取值格式、删除 token、改变叠加规则。届时 `validateManifest` 增加对旧版本的迁移分支,老文件继续可读。
- 不设 `minAppVersion`:`apiVersion` + 未知字段忽略已经覆盖了实际需要(见待确认问题 Q5)。

## 7. 校验规则

实现:`src/theme/validate.ts`(`validateManifest` / `parseThemeFile`)。输出 `{ manifest | null, diagnostics[] }`,每条诊断带 `level`(`error` / `warning`)、JSON 路径、中文说明,设置页会列出来。

**error(整份拒绝,回退默认主题)**:不是 JSON 对象;`apiVersion` 缺失 / 非整数 / 高于支持版本 / 小于 1;`id` 缺失或不合法或占用内置 id;`name` 缺失或超长;`modes` 缺失或一个模式都没有;文件超过 256 KiB;不是合法 JSON(含 UTF-8 错误)。

**warning(丢弃该项,回退默认值,其余生效)**:未知字段(任何层级);颜色解析失败或带透明度;字体族含白名单外字符或引号不成对;长度不是 `0` / `px` / `rem` / `em` 或超过上限(128px / 8rem);阴影不符合 `[inset] 2–4 个长度 [颜色]` 的层语法;调色板不是 1–16 个颜色的数组(空数组 / 全部无效也回退);`agents` 里的 id 不合法;`homepage` 不是 http(s)。

**值的归一化**:语义色与 `stat` 色 → `H S% L%` 三元组;图表色 → `#rrggbb`;阴影里的颜色 → `rgb(r g b / a)`;字体族 → 逗号后统一一个空格。写进 CSS 的永远是我们自己格式化出来的字符串。

**内置主题**同样跑一遍校验(`normalizeBuiltin`),写坏了会在开发期直接抛错。

**命令行校验**:`node scripts/theme-lint.mjs <文件>`,用的是应用里同一份代码。可选的对比度检查:`node scripts/theme-contrast.mjs <文件>`(见 §12.3)。

**编码**:文件须是 UTF-8;开头的 BOM 会被忽略(Windows 记事本 / PowerShell 5.1 常会写出 BOM)。

## 8. 回退策略

| 情况 | 行为 |
|---|---|
| 单个 token 非法 | warning;该 token 用默认主题**同模式**的值 |
| 主题没给某个 token | 用默认主题同模式的值(公共 `tokens` 里给了则用公共的) |
| 主题不支持当前模式 | 自动切到它支持的第一个模式,设置页提示「该主题没有 X 模式」,并把切换后的模式写回设置 |
| 设置里的 `themeId` 找不到(文件被删)| 应用默认主题;**不改设置值**(文件放回来就恢复);设置页显示「找不到主题 X,已回退默认主题」 |
| 主题校验失败 | 同上,并在设置页列出错误;下拉框里该项灰显「(无效)」不可选 |
| `list_themes` 命令失败 / 目录不可读 | 只用内置主题,设置页显示读取失败原因 |
| `applyResolvedTheme` 抛异常 | 捕获并记日志;页面仍是 index.css 的兜底(= 默认主题) |
| JS 完全没跑起来 | `index.css` 的 `:root` / `.dark` 兜底值就是默认主题 |
| 首帧 | 上次的变量缓存(`localStorage["otr-theme-cache"]`)与本机记忆的模式先应用,随后被设置文件的真相覆盖 |

叠加顺序(单 token 粒度):默认主题公共 → 默认主题该模式 → 本主题公共 → 本主题该模式。`chart.palette` / `agentFallback` 整组替换,`chart.agents` 按 id 合并。

## 9. 安全

主题文件来自用户目录,视为**不可信输入**。分层防御:

1. **没有代码执行面**:清单只走 `JSON.parse`,不存在 `eval`、不存在动态 `import`、不存在 `<script>`。
2. **没有任意 CSS**:不提供样式字段。每个 token 都有自己的白名单语法(§7),值经解析后**重新格式化**再写入,原始字符串不落地;`url(`、`@import`、`expression(`、`;`、`{`、`}`、`<` 根本进不了 CSS。
3. **写入方式本身就受限**:`element.style.setProperty("--x", value)` 只能设置一个自定义属性的值,无法跳出到别的规则或选择器;即使某个值有问题,影响也只限于引用了该变量的属性。
4. **CSP 兜底**:`img-src 'self' data:`、`font-src 'self' data:`、`connect-src` 白名单 —— 就算前三层都失守,也不能发起外链请求。
5. **文件系统边界(Rust `themes.rs`)**:命令**没有参数**,前端无法指定路径,不存在路径穿越;只枚举固定目录、深度一层;**符号链接一律跳过**(文件或目录),读取范围不会离开主题目录;隐藏项跳过;单文件上限 256 KiB(超过只报错不读);最多 64 个主题;非 UTF-8 报错。
6. **不联网**:`homepage` 只显示文本,不是链接;应用不会因为主题去访问任何地址。
7. **不碰别的设置**:主题只影响外观;`save_settings` 依旧保留额度账号等受保护字段。
8. **不泄漏**:设置页展示的诊断只含文件路径、字段路径与固定文案,不回显文件内容。

## 10. 内置主题如何纳入体系

- `src/theme/builtin.ts` 的 `OTR_THEME` 是"OTR 默认"主题:公共 token(字体、圆角、阴影、指标色、图表色)+ `modes.dark` / `modes.light` 的语义色,值与改造前的 `index.css` / `bindings.ts` / 各组件里的 Tailwind 色**逐位相同**。它是完整的(每个 token 都有值),因此可以做所有缺失 token 的最终回退。
- 它与用户主题走**同一条**校验 → 归一化 → 解析 → 应用链;`BUILTIN_THEMES` 数组里的主题在设置页以「· 内置」后缀列出,`source = "builtin"`,用户主题不能占用它们的 id。
- `index.css` 保留同一套变量作为 JS 之前的兜底;两处必须同步(改一处改另一处)。本次实现附带做过一次自动对照:内置主题解析出的每个 CSS 变量与 `index.css` 完全一致;`agentColor()` 对已知与未知 Agent 的取色与旧实现完全一致。
- 以后要加第二个内置主题:在 `builtin.ts` 追加一个清单并放进 `BUILTIN_THEMES` 即可,格式与第三方一样。

## 11. 实现文件说明

前端(`src/`):

| 文件 | 作用 |
|---|---|
| `theme/types.ts` | token 目录常量(`COLOR_TOKENS` 等)、`ThemeManifest` / `ThemeTokens` / `ResolvedTheme` / `ThemeEntry` / `ThemeDiagnostic` 类型、`THEME_API_VERSION`、大小上限 |
| `theme/color.ts` | 颜色解析(`#hex` / `rgb()` / `hsl()` / 裸三元组)与 HSL/hex 归一化 |
| `theme/validate.ts` | 清单校验与归一化;字体 / 长度 / 阴影的白名单语法 |
| `theme/builtin.ts` | 内置默认主题(经校验归一化);`BUILTIN_THEMES` |
| `theme/resolve.ts` | 叠加合并 → `ResolvedTheme`(CSS 变量表 + 图表调色板);`pickAgentColor` |
| `theme/apply.ts` | 写 DOM(class / color-scheme / CSS 变量)、localStorage 缓存、`bootTheme()` |
| `theme/registry.ts` | 内置 + `api.listThemes()` → `ThemeEntry[]`(重复 id 处理) |
| `theme/ThemeProvider.tsx` | React context:当前主题、条目列表、`selectTheme` / `setMode` / `reload` / `agentColor` / `chartPalette`;`decide()` 决定回退 |
| `theme/index.ts` | 统一导出 |
| `main.tsx` | 首帧 `bootTheme()`;用 `ThemeProvider` 包住主窗口与教程窗口 |
| `api/bindings.ts` | `Settings.themeId`;`api.listThemes` / `api.getThemesDir`;移除硬编码的 `AGENT_COLORS` / `agentColor` |
| `index.css` | 新增状态色 / 指标色 / 字体 / 圆角 / 阴影变量(兜底值 = 默认主题);`body` 字体改用变量 |
| `tailwind.config.js` | 新增 `overlay` / `success` / `warning` / `danger` / `info` / `notice` / `stat-*` 颜色,`borderRadius` / `boxShadow` / `fontFamily` 改为引用变量 |
| `components/*.tsx`、`App.tsx` | 硬编码 Tailwind 色 → 语义 class;图表 / 卡片 / chip 改用 `useTheme().agentColor` 与 `chartPalette`;tooltip 圆角用 `var(--radius-lg)` |
| `components/Settings.tsx` | 「外观与启动」区块:主题下拉框、主题目录路径、重新扫描、诊断列表;深浅模式按钮按主题支持情况禁用;切换时把 `themeId` / `theme` 写入设置 |

后端(`src-tauri/src/`):

| 文件 | 作用 |
|---|---|
| `themes.rs` | `themes_dir()` / `ensure_dir()` / `discover()`:枚举两种布局、跳过符号链接与隐藏项、大小与数量上限、UTF-8 检查;附单测 |
| `commands.rs` | `list_themes()`(无参数)、`get_themes_dir()` |
| `lib.rs` | 注册模块与命令 |
| `settings.rs` | `Settings.theme_id`(JSON `themeId`,默认 `"otr"`);旧文件缺字段自动补默认;附单测 |

其它:`docs/theme.schema.json`(JSON Schema,编辑器补全)、`scripts/theme-lint.mjs`(命令行校验)、`scripts/theme-contrast.mjs`(按界面真实用法算前景/背景对比度)、`examples/themes/`(示例主题,见 §12.5)。

## 12. 主题作者指南(Author guide)

### 12.1 放在哪

1. 打开 OTR → 设置 → 「外观与启动」→ 「主题」一行下方显示的目录,就是主题目录(Windows 典型值 `%APPDATA%\com.otae.radar\themes\`;目录不存在时应用会自动创建)。
2. 把文件放成下面任一种:
   - `themes/<任意名字>.json`
   - `themes/<任意名字>/theme.json`
3. 回到设置页点「重新扫描」,下拉框里就会出现你的主题;选中即生效,不用重启。

文件用 UTF-8 保存(带不带 BOM 都行)。设置页提示里写的 `theme.json` 泛指主题文件:单文件布局下文件名随意,只要扩展名是 `.json`;目录布局下文件名必须是 `theme.json`。

### 12.2 怎么写

从下面这个最小骨架开始(只改主色与背景,其余全部继承默认主题):

```json
{
  "apiVersion": 1,
  "id": "my-theme",
  "name": "My Theme",
  "version": "0.1.0",
  "author": "你的名字",
  "modes": {
    "dark": {
      "colors": {
        "background": "#101418",
        "card": "#161b22",
        "primary": "#ff7b72"
      }
    }
  }
}
```

然后按需要往里加:

- **只想做一个模式**:`modes` 里只写 `dark` 或 `light`。用户选你的主题时会自动切到这个模式,另一个模式的按钮会灰掉。
- **两个模式**:`modes.dark` 和 `modes.light` 各给一套 `colors`。任何没写的 token 都用默认主题**对应模式**的值,所以可以逐步补全。
- **和模式无关的东西**(字体、圆角、阴影、图表调色板、Agent 品牌色)放在顶层 `tokens` 里,两种模式共用;如果某个模式要不一样,再在那个模式里覆盖一次即可。
- **颜色写法**:`"#3b82f6"`、`"#38f"`、`"rgb(59, 130, 246)"`、`"hsl(217 91% 60%)"`、`"217 91% 60%"` 都行;**不要带透明度**(`#ff000080`、`rgba(...)` 会被当成无效并回退)。
- **Agent 品牌色**:`chart.agents` 的键是 Agent id:内置的 `dsh`、`claude-code`、`codex`、`zcode`、`opencode`、`pi`、`cursor`;自定义 Agent 的 id 在设置页的 id 规则是 `custom-<名字>`(可在 `settings.json` 的 `customAgents` 里看到)。只写你想改的。
- **字体**:`font.sans` 是逗号分隔的字体族,例如 `"\"JetBrains Mono\", \"PingFang SC\", sans-serif"`。只能用系统已安装的字体;不能引用文件或网址。
- **圆角**:`"0.5rem"`、`"8px"`、`"0"`;上限 128px / 8rem。
- **阴影**:每层 `[inset] 偏移x 偏移y [模糊] [扩散] [颜色]`,多层用逗号;颜色可以带透明度,例如 `"0 4px 12px rgba(0,0,0,0.35)"`;`"none"` 表示无阴影。
- **单模式主题的 token 放哪**:建议把带颜色的组(`colors`、`stat`、`chart`、`shadow`)都写在那个模式里,只把 `font`、`radius` 放顶层 `tokens`。这些颜色是针对这一种底色调的,以后补另一个模式时不会被误继承;将来若取消 `modes` 层(Q1),迁移也只是把 `modes.<模式>` 合并进 `tokens`。
- **哪些颜色会被当文字**(按正文 4.5:1 选色):`foreground`(放在 `background`、`card` 上)、`mutedForeground`(`background`、`card`、`muted` 上)、`primary`(`card` 上的链接与角标)、`primaryForeground`(`primary` 上)、`destructive`、`success`、`warning`、三个 `*Text`、`notice`、`stat` 的六个颜色。图形类(`ring`、进度条、`chart.palette`、`chart.agents`)按 3:1。逐项说明见 §3.1 / §3.2,`scripts/theme-contrast.mjs` 会把这些组合算一遍。
- **开关滑块**:设置页里那排开关(额度来源、Agent 启用)的滑块颜色是 `primaryForeground`,轨道「开」是 `success`、「关」是 `mutedForeground` 的 30%。如果主色很亮、`primaryForeground` 取了深色(暗色霓虹风常见),「关」态的深色滑块会几乎看不见。`shadow.base` 只用在开关滑块上,可以用它给滑块描一圈浅色边来补救,例如 `"0 0 0 1px rgba(238, 236, 255, 0.7), 0 1px 4px 0 rgba(0, 0, 0, 0.6)"`(见待确认问题 Q11)。「启动时最小化到托盘」开关不一样:滑块是 `background` 色,轨道「开」是 `primary`、「关」是 `muted`。
- **hover 叠加色**:`overlay` 不必是纯黑 / 纯白;暖色主题用深褐、冷色主题用带色相的浅色,hover 时更协调(界面固定按 5% 叠加)。

完整的 token 名单见 §3;想要编辑器补全,在文件里加 `"$schema": "https://raw.githubusercontent.com/otae-1204/OTR/main/docs/theme.schema.json"`(或指向本仓库 `docs/theme.schema.json` 的本地路径;注意 schema 里的 `$id` 只是标识,不是可下载地址)。§4.1 有一个两种模式齐全的完整示例,§12.5 有两个单模式示例,都可以直接抄。

### 12.3 怎么验证

1. **命令行**(推荐,与应用内逻辑一致;需在 OTR 仓库根目录先 `npm install`):
   ```bash
   node scripts/theme-lint.mjs path/to/theme.json
   ```
   通过时打印 `✓ 通过`,并列出每个模式解析后的关键颜色与调色板;有 `[错误]` 表示应用会拒绝加载,有 `[警告]` 表示那一项会回退默认值。退出码 0 / 1。
2. **对比度**(可选,同样要先 `npm install`):
   ```bash
   node scripts/theme-contrast.mjs path/to/theme.json
   ```
   按界面里真实的前景 / 背景组合(包括 `bg-success/10` 这类半透明底,按浏览器方式混合)计算 WCAG 对比度:文字按 4.5:1,图形 / 控件按 3:1;标「参考」的项只打印不计入。有不达标项时退出码为 1。
3. **应用内**:设置页的主题一行下方会列出每个用户主题的错误与警告(带字段路径,如 `modes.dark.colors.prmary: 未知字段,已忽略`);无效主题在下拉框里灰显。
4. **肉眼核对清单**:仪表盘的 Hero 卡、四张 Agent 卡、趋势图(线色与图例)、模型占比环、会话表;额度页的进度条(绿 / 橙 / 红);设置页的开关、角标、错误提示;两种模式都切一遍;hover 一下卡片与按钮看叠加色。

### 12.4 常见错误

| 现象 | 原因 |
|---|---|
| 下拉框里是「(无效)」 | 顶层结构错:缺 `apiVersion` / `id` / `name` / `modes`,或 `id` 用了大写、空格 |
| 某个颜色没生效,列表里是警告 | 拼错 token 名(如 `prmary`)、带了透明度、用了颜色关键字(`red`) |
| 字体没生效 | 字体族里有括号、斜杠等白名单外字符,或引号不成对 |
| 明明改了文件但没变化 | 没点「重新扫描」;或另一个文件用了同一个 `id`(按文件名排序靠前的那个生效) |
| 选了主题后模式按钮灰了 | 主题只提供了一个模式,这是预期行为 |
| 改了 `cardForeground` / `popover` / `secondary` / `accent` 等没有任何变化 | 这些 token 目前界面没有引用(§3.1);卡片文字要改 `foreground` |
| 选回别的主题后还是亮色 / 暗色 | 单模式主题会把深浅模式切过去并保存;选回双模式主题时不会自动恢复,手动切回即可(见 Q14) |
| 「不是合法 JSON」但内容看着没错 | JSON 不允许注释和末尾多余的逗号 |

### 12.5 示例主题

`examples/themes/` 里有两个完全按本指南制作的第三方主题,安装方法见该目录的 `README.md`:

| 文件 | id / 名称 | 模式 | 风格 |
|---|---|---|---|
| `warm-paper.json` | `warm-paper` / 暖纸 Warm Paper | 只有 `light` | 米白纸张底、墨褐正文、陶土色主色、大地色图表;衬线字体、较大圆角、暖褐阴影 |
| `neon-night.json` | `neon-night` / 霓虹夜 Neon Night | 只有 `dark` | 近黑紫底、青色霓虹主色(深色按钮文字)、品红焦点环;等宽字体、近直角、发光阴影,`shadow.base` 给开关滑块描浅色边 |

两者都覆盖了全部 28 个 `colors`、6 个 `stat`、`chart` 的三项,以及 `font` / `radius` / `shadow`;布局按 §12.2「单模式主题的 token 放哪」。`theme-lint.mjs` 零错误零警告,`theme-contrast.mjs` 全部文字组合 ≥ 4.5:1。都做成单模式,是为了不论 Q1 最终怎么定都能直接沿用。

## 13. 待确认问题(Open questions)

每个问题都给出选项、我采用的默认方案与理由,并标注是否会**实质性改变主题文件格式**。

**Q1. 主题的粒度:一个主题同时含深浅两种模式,还是一个主题就是一种外观?**
选项:(a) 一个主题可含 `dark` / `light` 两个模式,用户仍可独立切换深浅(**默认**);(b) 一个主题只描述一种外观,深浅切换概念被主题选择取代;(c) 主题只描述一种外观,但清单可声明"配对主题"的 id。
默认理由:现有 UI 已有深浅切换,用户习惯与 `settings.theme` 字段都保留;单模式主题也能表达(只写一个模式)。
改变主题格式:**是**(选 b/c 会移除 `modes` 层)。

**Q2. 内置默认主题的 id / 名称。**
选项:(a) `otr` /「OTR 默认」(**默认**);(b) `default`;(c) `otr-dark` 与 `otr-light` 拆成两个内置主题。
默认理由:`otr` 与产品同名、不会与通用词冲突;拆成两个会让"深浅切换"和"主题切换"打架。
改变主题格式:**否**(只影响保留 id 列表)。

**Q3. 是否开放间距 / 密度 token。**
选项:(a) 不开放(**默认**);(b) 加一个 `density` 标量(如 0.9–1.1)整体缩放 padding;(c) 开放 Tailwind spacing 的几个关键档位。
默认理由:间距牵涉对齐与可用性,现有布局用固定档位,开放后主题很容易把额度条、表格挤坏;需要时 (b) 是加法,不破坏旧主题。
改变主题格式:**否**(以后加是新增可选字段)。

**Q4. 是否允许主题携带受限的自定义 CSS。**
选项:(a) 不允许(**默认**);(b) 允许一个 `css` 字段,只接受白名单属性(颜色 / 背景 / 边框 / 圆角 / 阴影)且选择器限于我们公开的 `data-*` 钩子;(c) 允许任意 CSS 但做正则清洗。
默认理由:token 已覆盖现有界面的全部可变外观;(c) 的清洗不可靠,(b) 需要先设计稳定的 DOM 钩子,可作为 apiVersion 2 的议题。
改变主题格式:**是**(新增字段,但语义上是新能力;旧应用会忽略并告警)。

**Q5. 是否增加 `minAppVersion` 字段。**
选项:(a) 不加,靠 `apiVersion` + 未知字段忽略(**默认**);(b) 加可选 `minAppVersion`,低于时只告警仍加载;(c) 加且低于时拒绝加载。
默认理由:token 只做加法时老应用忽略新 token 即可;真正的破坏性变化用 `apiVersion` 表达更明确。
改变主题格式:**否**(以后加是新增可选字段)。

**Q6. `settings.theme`(模式)字段是否改名为 `colorMode`。**
选项:(a) 保留 `theme` 表示模式、新增 `themeId`(**默认**);(b) 改名 `colorMode` 并做一次迁移;(c) 合并成 `appearance: { themeId, mode }`。
默认理由:改动最小、旧设置文件零迁移;字段含义已在类型注释与本文说明。
改变主题格式:**否**(只是应用设置文件)。

**Q7. 用户主题的文件布局。**
选项:(a) 同时支持 `themes/<name>.json` 与 `themes/<name>/theme.json`(**默认**);(b) 只支持单文件;(c) 只支持目录。
默认理由:单文件最易分享;目录形式给以后放预览图 / 说明留位置;两者都只认一层深度,实现成本几乎为零。
改变主题格式:**否**(清单内容不变)。

**Q8. 主题被删除后设置里的 `themeId` 怎么处理。**
选项:(a) 保留原值,运行时回退并提示(**默认**);(b) 自动改回 `otr`;(c) 提示用户选择。
默认理由:用户可能只是临时移走文件(或同步目录还没到),保留选择更友好;回退提示在设置页可见。
改变主题格式:**否**。

**Q9. 是否随应用附带第二个内置主题(如高对比)。**
选项:(a) 只内置默认主题(**默认**);(b) 加一个高对比 / 无障碍内置主题;(c) 把示例主题作为内置。
默认理由:本次只搭接口;示例主题将按本指南另行制作,验证接口后再决定是否收编为内置。(示例主题已制作,见 §12.5。)
改变主题格式:**否**。

**Q10. 状态「填充色」`success` / `warning` 被直接当文字色用,是否改成对应的 `*Text`。**
现状:额度剩余百分比、「缓存命中率」标题、设置保存成功提示、定价表「手动」来源标签、Agent 卡 ⚠ 图标用的是 `text-success` / `text-warning`(改造前就是 `text-emerald-500` / `text-amber-500`),而错误文字用的是 `text-danger-text`。结果是作者必须让 `success` / `warning` 同时当填充色和文字色都合适;默认主题亮色下这几处文字只有约 2.2–2.5:1。
选项:(a) 保持现状,在文档里写明这两个填充色也要当文字可读(**默认**);(b) 把这几处组件改成 `text-success-text` / `text-warning-text`(默认主题亮色下这几处会变深一档,不再逐位相同);(c) 新增专门的 token(如 `successLabel`)。
默认理由:不改组件、默认外观逐位不变;示例主题已按"两用"选色,证明可行。(b) 是可读性更好的方向,但改变默认主题外观,需要主人拍板。
改变主题格式:**否**((a)(b) 不变;(c) 是新增可选 token)。

**Q11. 设置页开关的滑块颜色绑在 `primaryForeground` 上。**
现状:改造前滑块是 `bg-white`,改造时映射成了 `bg-primary-foreground`(默认主题里两者都是白色);但滑块并不在 `primary` 上,而是在 `success`(开)/ `mutedForeground` 30%(关)上。主色亮、`primaryForeground` 取深色的主题,「关」态滑块几乎看不见(霓虹夜示例约 1.8:1,靠 `shadow.base` 描边补救)。
选项:(a) 保持现状,文档写明并建议用 `shadow.base` 描边(**默认**);(b) 滑块改用 `background` 或 `card`(与「启动时最小化到托盘」开关一致;默认主题暗色下滑块会变成深灰,外观变化);(c) 新增 `switchThumb` token,缺省为白色;(d) 固定白色,不受主题控制。
默认理由:不改组件、默认外观不变,且已有可行的补救写法。(c) 最干净,可作为加法随时引入。
改变主题格式:**否**((c) 是新增可选 token)。

**Q12. 当前没有被界面引用的 token 怎么处理。**
现状:`cardForeground`、`popover` / `popoverForeground`、`secondary` / `secondaryForeground`、`accent` / `accentForeground`、`input`、`destructiveForeground`、`info` 会被校验并写成 CSS 变量,但没有组件使用;作者改了看不到变化(尤其 `cardForeground`,卡片文字实际用的是 `foreground`)。
选项:(a) 保留为"预留 token",文档注明目前未引用(**默认**);(b) 让组件真正用起来(如卡片内文字改用 `text-card-foreground`),已写了这些值的主题会随之变化;(c) 在下一个 `apiVersion` 删掉用不上的;(d) 校验时对这些 token 给出提示性 warning。
默认理由:shadcn 风格的完整语义色表便于以后扩展组件,写了也无害;(b) 属于组件改动,可逐个做。注意 (d) 会让"零警告"的主题出现警告,不建议。
改变主题格式:(a)(b)(d) **否**;(c) **是**(删除 token 需升 `apiVersion`)。

**Q13. `$schema` 的正式在线地址。**
现状:原文档与 schema 的 `$id` 写的是 `https://github.com/otae-1204/OTR/docs/theme.schema.json`,这个地址在 GitHub 上是 404,编辑器加载不到 schema。文档与示例主题已改为 `https://raw.githubusercontent.com/otae-1204/OTR/main/docs/theme.schema.json`(合并到 `main` 后可用),schema 文件里的 `$id` 未改。
选项:(a) 用 `main` 分支的 raw 地址(**默认**);(b) 按版本 tag 固定(如 `.../v0.3.0/docs/theme.schema.json`),每个 `apiVersion` 一个稳定地址;(c) 发布到 GitHub Pages / 自有域名;以上任一都可顺手把 schema 的 `$id` 改成同一地址。
默认理由:零维护、立刻可用;`apiVersion` 只做加法时 `main` 上的 schema 对旧主题也适用。若以后出现 `apiVersion = 2`,再改成 (b)。
改变主题格式:**否**(`$schema` 只给编辑器用,应用忽略)。

**Q14. 选了单模式主题后,深浅模式要不要"记住原来的偏好"。**
现状:选中只有亮色的主题时,模式被切到亮色并**写回设置**;之后选回双模式主题(如默认主题)时停留在亮色,不会回到用户原来的暗色。
选项:(a) 保持现状(**默认**);(b) 单独记住用户的"偏好模式",选回支持该模式的主题时自动恢复;(c) 不写回设置,只在运行时临时切换。
默认理由:行为简单可预期,设置页有提示,手动切回只需一次点击;这个问题与 Q1 直接相关 —— 若 Q1 选 (b)/(c),模式本身就归主题管,这里自然消失,不值得先做 (b)。
改变主题格式:**否**(只影响应用设置与切换逻辑)。
