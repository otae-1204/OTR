# OTR 主题接口(Theme API)设计文档

> 版本:主题格式 `apiVersion = 1`;对应 OTR 0.2.x。
> 相关代码:`src/theme/`(前端;受限 css 在 `css.ts`)、`src-tauri/src/themes.rs`(后端;托盘「恢复默认主题」见 `tray.rs`)、`docs/theme.schema.json`(JSON Schema)、`scripts/theme-lint.mjs`(校验脚本)、`scripts/theme-contrast.mjs`(对比度检查)、`scripts/theme-css-selftest.mjs`(css 校验器自检)、`examples/themes/`(示例主题)。

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
2. **纯数据主题**:主题 = 一个 JSON 文件,没有 JS、没有任意 CSS(只有一张结构化、逐项白名单的 `css` 表,见 §14)。第三方能写、能分享、应用能安全加载。
3. **内置默认外观也是一个主题**:走完全相同的格式、校验与解析路径;选回它时与改造前逐位一致 —— 只有 Q10 / Q11 确认后**有意**改掉的几处例外(亮色下的独立状态文字更深、暗色下额度剩余 <25% 的红字更浅、亮色开关「关」态滑块改为灰色、「启动时最小化到托盘」开关与其它开关统一,见 §10)。
4. **永远能用**:任何一处失败(文件坏了、字段错了、主题被删了、JS 抛异常)都只影响那个 token 或那个主题,应用退回默认外观继续工作。
5. **可演进**:新增 token 不破坏旧主题;格式真的要变时靠 `apiVersion` 明确表达。
6. **最小改动**:不重构组件结构,只把硬编码换成 token;Rust 侧只加一个"枚举 + 读文件"的模块。

## 3. 主题可控内容(token 目录)

主题通过六组 token(`colors` / `stat` / `chart` / `font` / `radius` / `shadow`)控制外观,外加一张可选的受限 `css` 表(§3.5、§14)。**颜色值一律要求不透明**(`#rrggbb`、`rgb()`、`hsl()`、或裸 HSL 三元组 `"240 5% 12%"`;不接受颜色关键字),原因见 §7。

### 3.1 `colors` —— 语义色(按模式给)

| token | CSS 变量 | 用途 | 默认(暗色) |
|---|---|---|---|
| `background` / `foreground` | `--background` / `--foreground` | 页面底色 / 正文 | `240 5% 12%` / `0 0% 93%` |
| `card` / `cardForeground` | `--card` / `--card-foreground` | 卡片底色 / 文字 | `240 5% 16%` / `0 0% 93%` |
| `popover` / `popoverForeground` | `--popover` / `--popover-foreground` | 浮层 | `240 5% 14%` / `0 0% 93%` |
| `primary` / `primaryForeground` | `--primary` / `--primary-foreground` | 主色(图标、选中态、主按钮;也直接当**小号文字色**用:链接、角标)/ 主按钮上的文字(开关滑块已改用 `switchThumb`,见下) | `210 100% 56%` / `0 0% 100%` |
| `secondary` / `secondaryForeground` | `--secondary` / `--secondary-foreground` | 次级底色 | `240 5% 20%` / `0 0% 93%` |
| `muted` / `mutedForeground` | `--muted` / `--muted-foreground` | 弱化底色(分段控件轨道、进度条空轨)/ 次要文字、图表刻度 | `240 5% 20%` / `240 5% 65%` |
| `accent` / `accentForeground` | `--accent` / `--accent-foreground` | 强调底色 | `240 5% 20%` / `0 0% 93%` |
| `destructive` / `destructiveForeground` | `--destructive` / `--destructive-foreground` | 危险操作的**文字**与描边(全量重扫按钮、删除按钮 hover)、检查更新 / 汇率 / 价格拉取失败的错误文字 | `0 62% 45%` / `0 0% 100%` |
| `border` / `input` / `ring` | `--border` / `--input` / `--ring` | 边框、输入框边框、焦点环;图表网格线也用 `border` | `240 5% 24%` ×2 / `210 100% 56%` |
| `overlay` | `--overlay` | hover 高亮叠加的**基色**,界面按 5% 透明度叠加(`hover:bg-overlay/5`) | `0 0% 100%`(亮色下是黑) |
| `success` / `warning` / `danger` | `--success` / `--warning` / `--danger` | 状态色**填充**:额度进度条、缓存命中率条、设置页开关的「开」态轨道(`success`)。**只作填充**,界面不再拿它们当文字色(Q10) | emerald-500 / amber-500 / red-500 |
| `successText` / `warningText` / `dangerText` | `--success-text` / `--warning-text` / `--danger-text` | 状态色**文字**(角标与说明):同色 10–15% 淡底上的角标文字、额度卡的警告说明、主题诊断、错误提示(默认亮色更深、暗色更浅) | emerald-400 / amber-400 / red-500 |
| `successLabel` / `warningLabel` / `dangerLabel` | `--success-label` / `--warning-label` / `--danger-label` | **可选,新增**。直接放在卡片上的**独立状态文字**:额度剩余百分比(≥50% / 25–50% / <25%)、「缓存命中率」标题、设置保存成功提示(success)、定价表「手动」来源标签与 Agent 卡的 ⚠(warning)。缺省回退见下方「后加 token 的回退」 | emerald-500 / amber-500 / red-400(亮色:emerald-700 / amber-700 / red-600) |
| `switchThumb` / `switchThumbOff` | `--switch-thumb` / `--switch-thumb-off` | **可选,新增**。设置页开关的滑块:「开」态(轨道 `success`)/「关」态(轨道 `mutedForeground` 30% 叠在卡片上)。所有开关(额度来源、Agent 启用、启动最小化)是同一个组件 | 白 / 白(亮色「关」:zinc-500) |
| `info` | `--info` | 信息色。**界面目前没有引用**(预留);「请求次数」指标用的是 `stat.calls`,只是默认值恰好相同 | sky-500 |
| `notice` | `--notice` | 「有新版本」小红点,以及角标的**文字**(底色是它自己的 15%) | orange-500 |

Tailwind 侧对应的 class:`bg-success` / `text-success-text` / `text-success-label` / `text-danger-label` / `bg-switch-thumb` / `bg-switch-thumb-off` / `bg-notice` / `hover:bg-overlay/5` 等(见 `tailwind.config.js`)。

**三类状态色怎么分**:填充(`success` 等)画条和轨道,按图形 3:1 选;`*Text` 用在角标淡底和说明文字里;`*Label` 是卡片上单独出现的状态字(数字、标题、提示)。后两类都是文字,按 4.5:1 选。以前 `*Label` 的位置直接用填充色,作者必须让同一个颜色既当条又当字(默认主题亮色下这几处只有 2.2–2.5:1),这是 Q10 拆出 `*Label` 的原因。

**后加 token 的回退**(`COLOR_FALLBACKS`,`src/theme/types.ts`):`*Label` 与 `switchThumb*` 是 `apiVersion = 1` 内的加法,老主题里没有。主题**自己**(公共 `tokens` 或该模式)没写它们时,按下表找主题自己写了的「原先那个 token」,用它的值;链上都没写,才用默认主题的值(与其它 token 相同)。

| token | 回退链(主题自己写了才算) | 为什么 |
|---|---|---|
| `successLabel` | `success` | 这几处以前用的就是 `success`:老主题这里逐位不变(旧版指南要求它两用、在卡片上可读) |
| `warningLabel` | `warning` | 同上 |
| `dangerLabel` | `dangerText` | 额度剩余 <25% 以前用的就是 `dangerText` |
| `switchThumb` | `primaryForeground` | 开关滑块以前用的就是 `primaryForeground`,「开」态没有已知问题,保持 |
| `switchThumbOff` | `switchThumb` | 只写了一个滑块色的主题,两态同色。**不**回退到 `primaryForeground`:「关」态以前沿用它,正是 Q11 要修的「深色滑块在近黑轨道上看不见」;所以老主题没写 `switchThumb*` 时,「关」态改用默认主题该模式的滑块色(暗色白、亮色 zinc-500) |

所以:**写过这些颜色的老主题,状态文字与开关「开」态外观不变**,开关「关」态改用默认主题的滑块色、自动变得可见;**没碰过状态色 / 主按钮文字色的主题**(例如只改了背景和主色)拿到的是默认主题为这几处专门调过的可读值。`theme-lint.mjs` 会打印每个模式里这几个 token 实际取到的值和来源(`(← success)` / `(默认主题)`)。

**目前界面没有引用的 token**(预留,Q12 已确认保持):`cardForeground`、`popover` / `popoverForeground`、`secondary` / `secondaryForeground`、`accent` / `accentForeground`、`input`、`destructiveForeground`、`info`。它们会被校验、写成 CSS 变量,但当前没有组件使用 —— 例如卡片上的文字用的是 `foreground` 而不是 `cardForeground`。主题里写上它们无害(以后组件用到时自动生效),但改它们现在看不到变化。

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
| `shadow.sm` / `base` / `md` / `lg` | `--shadow-sm` / `--shadow` / `--shadow-md` / `--shadow-lg` | `shadow-sm`(卡片 hover、分段选中)/ `shadow`(开关滑块,开关两态都有)/ `shadow-md`(选中的 Agent 卡)/ `shadow-lg`(图表 tooltip) | Tailwind 3.4 默认值 |

### 3.5 `css` —— 受限自定义样式(可选)

token 表达不了的效果(卡片渐变底、毛玻璃顶栏、标题字距、选中态描边、标题略放大……)可以写在 `css` 表里:键是应用公开的**钩子名**(对应组件上的 `data-theme-part`,可加 `:hover` 等状态后缀),值是「白名单属性 → 受限值」。它可以放在公共 `tokens` 里,也可以放在 `modes.dark` / `modes.light` 里,叠加规则与 token 相同。选择器由应用生成,主题写不了选择器;属性、值逐项白名单,不合法的条目单独丢弃并告警。完整说明见 §14,写法见 §12.6。

### 3.6 明确**不允许**主题控制的内容

- **任何脚本**:清单是纯 JSON,不存在执行入口。
- **任意 CSS**:没有自由文本的样式字段。`css` 表(§14)只是「钩子 → 白名单属性 → 受限值」:选择器由应用按钩子名生成,属性与值逐项白名单;改不了布局、尺寸、动画、滚动条,写不了任何选择器。
- **间距、尺寸、断点、布局**(Tailwind spacing scale、`max-w-6xl`、栅格列数、图表高度):这些决定可用性与对齐,不开放(见待确认问题 Q3)。字号 / 行高只能在 `css` 表里按组件原值 **±20%** 缩放(Q15,§14.3),不能写绝对值。
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

`tokens` 对象的结构在两处完全相同(公共 / 按模式),六个组:`colors`、`stat`、`chart`、`font`、`radius`、`shadow`,每个组的键见 §3;外加可选的 `css` 表(§14)。**所有 token 都是可选的**。

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
 │    读 localStorage["token-show-theme"] → 上次的**生效**模式,切 .dark class
 │    读 localStorage["otr-theme-cache"]  → 若模式一致,把上次的 CSS 变量与 css 表先刷上去(防闪)
 └─ <ThemeProvider> 挂载 → bootstrap()        ← 异步
      ├─ api.getSettings()      → themeId(默认 "otr")、preferredMode(偏好模式;旧文件缺字段时由 theme 推导)
      ├─ api.getThemesDir()     → 显示给用户的目录(不存在则创建)
      ├─ api.listThemes()       → Rust 枚举 <数据目录>/themes/,读出每个文件的文本
      ├─ 内置主题 + 用户文件 → parseThemeFile() 校验 → ThemeEntry[](含诊断)
      ├─ decide(entries, themeId, 偏好模式) → 找不到 / 无效 → 默认主题;不支持偏好模式 → 临时用它支持的模式
      └─ applyResolvedTheme()   → <html>.style 写全部 CSS 变量、切 class、color-scheme、写缓存
                                  (缓存 / token-show-theme 记生效模式,otr-preferred-mode 记偏好)

托盘「恢复默认主题」(Q16,tray.rs)
 ├─ Rust:settings.themeId = "otr",theme = preferredMode,其余字段不动 → 写 settings.json
 ├─ Rust:对每个窗口 eval 固定脚本 themes::RESET_SCRIPT(删首帧缓存、删 <style id="otr-theme-css">、
 │        清 <html> 的内联变量)→ 页面立刻回到 index.css 兜底 = 默认主题,不依赖前端脚本
 ├─ Rust:emit("theme://reset") → ThemeProvider 先 resetThemeDom() 再 bootstrap() 按新设置重新应用;
 │        设置页收到后把自己快照里的 themeId / theme / preferredMode 同步成新值
 └─ Rust:把主窗口叫出来
```

- **内置主题**:`src/theme/builtin.ts` 里的 TS 对象,随应用打包;它也经过 `validateManifest()` 归一化,与用户主题走同一条路。
- **用户主题目录**:`<app_data_dir>/themes/`(与 `settings.json`、`radar.db` 同级;Windows 典型路径 `%APPDATA%\com.otae.radar\themes\`)。设置页显示这个路径。
- **两种布局**都认:`themes/<name>.json` 或 `themes/<name>/theme.json`。后者留给以后放预览图等附件。
- **身份以清单里的 `id` 为准**,文件名只用于排序和校验失败时的展示。多个文件同一个 `id` 时,按文件名排序的第一个生效,其余报错 `id 重复`。
- **重新扫描**:设置页「重新扫描」按钮(`reload()`),不需要重启。
- **多窗口**:Cookie 教程窗口与主窗口共用 `localStorage` 与 `ThemeProvider`,首帧就是当前主题。
- **持久化**:`settings.json` 里三个字段 ——
  - `themeId`(字符串,默认 `"otr"`):选中的主题;
  - `preferredMode`(`"dark"` / `"light"`):用户**偏好**的深浅模式,只由设置页的深浅按钮修改(Q14);
  - `theme`(`"dark"` / `"light"`,历史字段名):**生效**的模式。选中单模式主题时它跟着变,`preferredMode` 不变;旧版本应用只认这个字段,所以继续写。

  旧设置文件没有 `preferredMode` 时由 `theme` 推导(Rust `Settings::load` 与前端各兜一层)。局限:升级前选过单模式主题的用户,`theme` 已经被改写过,真实偏好无从得知,只能取它;之后再用深浅按钮改一次即可。Rust 侧除「恢复默认主题」外只存取、不解释这些字段。

## 6. 版本与兼容

- `THEME_API_VERSION = 1`(`src/theme/types.ts`)。清单里 `apiVersion` 必填、整数。
- **向后兼容(老主题在新应用)**:同一 `apiVersion` 内只做**加法**(新增 token、新增可选字段)。老主题缺新 token → 回退默认值;所以老主题永远能加载。
- **向前兼容(新主题在老应用)**:
  - 新主题只是用了老应用不认识的 token / 字段 → 报 warning、忽略该字段,其余照常生效。
  - 新主题声明了更高的 `apiVersion` → 老应用拒绝(error:「主题需要更新的 OTR」),回退默认主题。语义未知时宁可不加载,不做"尽力而为"。
- **什么时候升 `apiVersion`**:改变已有 token 的含义/取值格式、删除 token、改变叠加规则。届时 `validateManifest` 增加对旧版本的迁移分支,老文件继续可读。
- 不设 `minAppVersion`:`apiVersion` + 未知字段忽略已经覆盖了实际需要(见待确认问题 Q5)。
- **`css` 字段**就是「同一 `apiVersion` 内只做加法」的第一个实例:它是新增的可选字段,老主题没有它、行为不变;更早的应用版本遇到它按未知字段忽略并告警,其余 token 照常生效。钩子目录与属性白名单以后也只增不减(§14.2)。
- **第二批加法**:颜色 token `successLabel` / `warningLabel` / `dangerLabel` / `switchThumb` / `switchThumbOff`(Q10 / Q11),以及 `css` 白名单里的 `font-size` / `line-height`(Q15)。老主题缺新 token 时按 §3.1 的回退链取「原先那个 token」,外观不变;更早的应用版本遇到新 token 报「未知字段,已忽略」、遇到 `font-size` 报「不支持的属性」,其余照常。

## 7. 校验规则

实现:`src/theme/validate.ts`(`validateManifest` / `parseThemeFile`)。输出 `{ manifest | null, diagnostics[] }`,每条诊断带 `level`(`error` / `warning`)、JSON 路径、中文说明,设置页会列出来。

**error(整份拒绝,回退默认主题)**:不是 JSON 对象;`apiVersion` 缺失 / 非整数 / 高于支持版本 / 小于 1;`id` 缺失或不合法或占用内置 id;`name` 缺失或超长;`modes` 缺失或一个模式都没有;文件超过 256 KiB;不是合法 JSON(含 UTF-8 错误)。

**warning(丢弃该项,回退默认值,其余生效)**:未知字段(任何层级);颜色解析失败或带透明度(新 token `*Label` / `switchThumb*` 同样要求不透明);字体族含白名单外字符或引号不成对;长度不是 `0` / `px` / `rem` / `em` 或超过上限(128px / 8rem);阴影不符合 `[inset] 2–4 个长度 [颜色]` 的层语法;调色板不是 1–16 个颜色的数组(空数组 / 全部无效也回退);`agents` 里的 id 不合法;`homepage` 不是 http(s);`css` 表里未知的钩子或状态、白名单外的属性、不合语法的值(包括超出 0.8–1.2 倍率的 `font-size` / `line-height`;逐条报告,路径如 `modes.dark.css.card.position`,见 §14.5)。

**值的归一化**:语义色与 `stat` 色 → `H S% L%` 三元组;图表色 → `#rrggbb`;阴影里的颜色 → `rgb(r g b / a)`;字体族 → 逗号后统一一个空格;`css` 里的值先分词再按属性语法重新拼出(颜色 → `#rrggbb` / `rgb(r g b / a)`,数字 → 最多三位小数 + 单位)。写进 CSS 的永远是我们自己格式化出来的字符串。

**内置主题**同样跑一遍校验(`normalizeBuiltin`),写坏了会在开发期直接抛错。

**命令行校验**:`node scripts/theme-lint.mjs <文件>`,用的是应用里同一份代码;加 `--print-css` 可打印 `css` 表最终生成的样式文本。可选的对比度检查:`node scripts/theme-contrast.mjs <文件>`(`--builtin` 查内置默认主题,`--only` 只看某几组,见 §12.3)。自检用例:`npm run test:theme`(`scripts/theme-css-selftest.mjs`,一千二百余条断言:css 校验器与各类注入尝试、字号 / 行高倍率、后加 token 的回退链、偏好模式、以及 schema / `index.css` / `tailwind.config.js` / Rust 复位脚本与代码的一致性)。

**编码**:文件须是 UTF-8;开头的 BOM 会被忽略(Windows 记事本 / PowerShell 5.1 常会写出 BOM)。

## 8. 回退策略

| 情况 | 行为 |
|---|---|
| 单个 token 非法 | warning;该 token 用默认主题**同模式**的值 |
| 主题没写后加的可选 token(`*Label` / `switchThumb*`) | 回退到主题自己写了的「原先那个 token」(§3.1 的回退链),都没写才用默认主题的值 |
| `css` 表里某个键 / 属性 / 值非法 | warning;只丢那一条,其余规则照常生效(内置主题没有 css,所以没有"回退值",只是不写) |
| 主题没给某个 token | 用默认主题同模式的值(公共 `tokens` 里给了则用公共的) |
| 主题不支持偏好模式 | 临时用它支持的第一个模式(`theme` 字段记下这个生效模式);**偏好模式不变**,深浅按钮整体禁用并提示「你偏好的 X 会在换回双模式主题时恢复」;选回双模式主题时自动按偏好恢复(Q14) |
| 设置里的 `themeId` 找不到(文件被删)| 应用默认主题;**不改设置值**(文件放回来就恢复);设置页显示「找不到主题 X,已回退默认主题」 |
| 主题校验失败 | 同上,并在设置页列出错误;下拉框里该项灰显「(无效)」不可选 |
| `list_themes` 命令失败 / 目录不可读 | 只用内置主题,设置页显示读取失败原因 |
| `applyResolvedTheme` 抛异常 | 捕获并记日志;页面仍是 index.css 的兜底(= 默认主题) |
| JS 完全没跑起来 | `index.css` 的 `:root` / `.dark` 兜底值就是默认主题(自检脚本逐个变量对照) |
| 主题把界面弄得不可见 / 设置页点不到 | 托盘菜单「恢复默认主题」:Rust 直接改设置并对窗口执行固定复位脚本,不依赖设置页也不依赖前端脚本(§5,Q16) |
| 首帧 | 上次的变量缓存(`localStorage["otr-theme-cache"]`)与本机记忆的**生效**模式先应用,随后被设置文件的真相覆盖;读不到设置时,偏好模式用本机记忆的 `otr-preferred-mode` |

叠加顺序(单 token 粒度):默认主题公共 → 默认主题该模式 → 本主题公共 → 本主题该模式。`chart.palette` / `agentFallback` 整组替换,`chart.agents` 按 id 合并,`css` 按「钩子[:状态] → 属性」粒度合并(§14.6)。

## 9. 安全

主题文件来自用户目录,视为**不可信输入**。分层防御:

1. **没有代码执行面**:清单只走 `JSON.parse`,不存在 `eval`、不存在动态 `import`、不存在 `<script>`。
2. **没有任意 CSS**:每个 token 都有自己的白名单语法(§7),值经解析后**重新格式化**再写入,原始字符串不落地。`css` 字段也不是文本而是一张表(§14):选择器由应用按钩子名生成、属性只认白名单、值先分词(字符集只有字母数字、空格与 `-+.%#(),/`)再按属性语法逐 token 匹配、最后由我们重新拼出字符串。`url(`、`@import`、`expression(`、`image-set(`、`!important`、`;`、`{`、`}`、`<`、`@`、注释、引号、反斜杠在分词阶段就被整体拒绝,`var()` 只能引用本主题体系的变量(§14.4)。`font-size` / `line-height` 只接受单个 0.8–1.2 的倍率,输出的是应用自己的倍率变量而不是字面值(§14.3);`--otr-*` 变量主题既不能引用也不能写,首帧缓存里出现也会被丢掉。序列化前每个值再归一化一遍,所以哪怕 localStorage 缓存被改也进不来结构字符。校验不是正则黑名单,是分词 + 逐项白名单 + 自检用例(`npm run test:theme`)。
3. **写入方式本身就受限**:token 走 `element.style.setProperty("--x", value)`,只能设置一个自定义属性的值,无法跳出到别的规则或选择器;`css` 表只经过**唯一一个**受控的 `<style id="otr-theme-css">`,内容整体替换、切换主题时移除,不用 `innerHTML`、不拼接用户字符串进选择器。CSP 的 `style-src 'self' 'unsafe-inline'` 本来就允许内联样式,本功能没有放宽任何策略。
4. **CSP 兜底**:`img-src 'self' data:`、`font-src 'self' data:`、`connect-src` 白名单 —— 就算前三层都失守,也不能发起外链请求。
5. **文件系统边界(Rust `themes.rs`)**:命令**没有参数**,前端无法指定路径,不存在路径穿越;只枚举固定目录、深度一层;**符号链接一律跳过**(文件或目录),读取范围不会离开主题目录;隐藏项跳过;单文件上限 256 KiB(超过只报错不读);最多 64 个主题;非 UTF-8 报错。
6. **不联网**:`homepage` 只显示文本,不是链接;应用不会因为主题去访问任何地址。
7. **不碰别的设置**:主题只影响外观;`save_settings` 依旧保留额度账号等受保护字段。托盘「恢复默认主题」只改 `themeId` 与 `theme`,其余设置原样保留(Rust 单测覆盖)。
8. **不泄漏**:设置页展示的诊断只含文件路径、字段路径与固定文案,不回显文件内容。
9. **复位脚本是常量**:「恢复默认主题」对窗口执行的 `themes::RESET_SCRIPT` 是编译期常量,不拼接任何输入,只做三件事(删首帧缓存键、删受控 `<style>`、清 `<html>` 内联样式);只能由托盘菜单在 Rust 侧触发,前端没有对应命令。Rust 单测与自检脚本分别检查它的内容与前端常量一致。

## 10. 内置主题如何纳入体系

- `src/theme/builtin.ts` 的 `OTR_THEME` 是"OTR 默认"主题:公共 token(字体、圆角、阴影、指标色、图表色)+ `modes.dark` / `modes.light` 的语义色,值与改造前的 `index.css` / `bindings.ts` / 各组件里的 Tailwind 色**逐位相同**。它是完整的(每个 token 都有值),因此可以做所有缺失 token 的最终回退。
- **Q10 / Q11 之后有意改动的默认外观**(其余仍逐位相同):

  | 位置 | 改造前 | 现在 | 对比度(对卡片 / 轨道) |
  |---|---|---|---|
  | 亮色:额度剩余 ≥50%、「缓存命中率」标题、保存成功提示(`successLabel`) | emerald-500 | emerald-700 `#047857` | 2.54 → 5.49:1 |
  | 亮色:额度剩余 25–50%、定价「手动」、Agent 卡 ⚠(`warningLabel`) | amber-500 | amber-700 `#b45309` | 2.15 → 5.01:1 |
  | 亮色:额度剩余 <25%(`dangerLabel`) | red-500 | red-600 `#dc2626` | 3.76 → 4.83:1 |
  | 暗色:额度剩余 <25%(`dangerLabel`) | red-500 | red-400 `#f87171` | 3.97 → 5.40:1 |
  | 暗色:`successLabel` / `warningLabel` | emerald-500 / amber-500 | 不变 | 5.88 / 6.95:1 |
  | 亮色:开关「关」态滑块(`switchThumbOff`) | 白 | zinc-500 `#71717a` | 1.48 → 3.27:1 |
  | 开关「开」态滑块(`switchThumb`)、暗色「关」态 | 白 | 不变 | 2.54:1(见 Q18)/ 8.60:1 |
  | 「启动时最小化到托盘」开关 | 自己一套小号实现:轨道开 `primary` / 关 `muted`,滑块 `background`,36×20 | 与其它开关同一组件:轨道开 `success` / 关 `mutedForeground` 30%,滑块 `switchThumb*`,44×24 | — |

  默认主题其余的历史配色(例如亮色下主色当链接文字 3.27:1、统计标签 2.1–4.0:1、部分图表色低于 3:1)未动,`node scripts/theme-contrast.mjs --builtin` 会如实列出,是否调整见 Q18。
- 它与用户主题走**同一条**校验 → 归一化 → 解析 → 应用链;`BUILTIN_THEMES` 数组里的主题在设置页以「· 内置」后缀列出,`source = "builtin"`,用户主题不能占用它们的 id。
- `index.css` 保留同一套变量作为 JS 之前的兜底;两处必须同步(改一处改另一处)。`npm run test:theme` 会逐个变量对照内置主题两种模式解析出的 CSS 变量与 `index.css` 的 `:root` / `.dark`,不一致即失败;`agentColor()` 对已知与未知 Agent 的取色与旧实现完全一致(改造时对照过)。
- 以后要加第二个内置主题:在 `builtin.ts` 追加一个清单并放进 `BUILTIN_THEMES` 即可,格式与第三方一样。

## 11. 实现文件说明

前端(`src/`):

| 文件 | 作用 |
|---|---|
| `theme/types.ts` | token 目录常量(`COLOR_TOKENS` 等)、后加 token 的回退链 `COLOR_FALLBACKS`、`ThemeManifest` / `ThemeTokens` / `ResolvedTheme` / `ThemeEntry` / `ThemeDiagnostic` 类型、`THEME_API_VERSION`、大小上限 |
| `theme/color.ts` | 颜色解析(`#hex` / `rgb()` / `hsl()` / 裸三元组)与 HSL/hex 归一化 |
| `theme/values.ts` | 字体族 / 长度 / 阴影的白名单语法(token 与 css 共用) |
| `theme/css.ts` | 受限自定义 CSS:钩子目录 `THEME_PARTS` / 状态 `THEME_STATES`、分词器、属性白名单与值语法(含 `font-size` / `line-height` 倍率与 `SCALE_PROPS`)、`validateCss` / `mergeCss` / `serializeThemeCss` / `selectorForKey` / `sanitizeCss` |
| `theme/validate.ts` | 清单校验与归一化(token 组 + `css` 表) |
| `theme/builtin.ts` | 内置默认主题(经校验归一化);`BUILTIN_THEMES` |
| `theme/resolve.ts` | 叠加合并 → `ResolvedTheme`(CSS 变量表 + 图表调色板 + css 表);`applyColorFallbacks`(后加 token 的回退链);`pickAgentColor` |
| `theme/apply.ts` | 写 DOM(class / color-scheme / CSS 变量 / 唯一受控 `<style id="otr-theme-css">`)、localStorage 缓存(含 css 表,读回时重新校验;`--otr-*` 不恢复)、生效 / 偏好模式的本机记忆、`resetThemeDom()`、`bootTheme()` |
| `theme/registry.ts` | 内置 + `api.listThemes()` → `ThemeEntry[]`(重复 id 处理) |
| `theme/ThemeProvider.tsx` | React context:当前主题、生效模式与偏好模式、条目列表、`selectTheme`(按偏好)/ `setMode`(改偏好)/ `reload` / `resetSeq` / `agentColor` / `chartPalette`;`decide()` 决定回退;监听 `theme://reset` |
| `theme/index.ts` | 统一导出 |
| `main.tsx` | 首帧 `bootTheme()`;用 `ThemeProvider` 包住主窗口与教程窗口 |
| `api/bindings.ts` | `Settings.themeId` / `preferredMode`;`api.listThemes` / `api.getThemesDir`;移除硬编码的 `AGENT_COLORS` / `agentColor` |
| `index.css` | 新增状态色 / 指标色 / 字体 / 圆角 / 阴影 / `*-label` / `switch-thumb*` 变量(兜底值 = 默认主题);`body` 字体改用变量 |
| `tailwind.config.js` | 新增 `overlay` / `success` / `warning` / `danger`(各含 `text` / `label`)/ `switch-thumb*` / `info` / `notice` / `stat-*` 颜色,`borderRadius` / `boxShadow` / `fontFamily` 改为引用变量;`fontSize` / `lineHeight` 每一档乘 `--otr-font-scale` / `--otr-line-scale`(默认 1),另加 `text-10px` / `text-11px` 两档代替任意值 |
| `components/*.tsx`、`App.tsx` | 硬编码 Tailwind 色 → 语义 class;独立状态文字用 `text-*-label`;图表 / 卡片 / chip 改用 `useTheme().agentColor` 与 `chartPalette`;关键元素带 `data-theme-part` 钩子(选中态另带 `data-theme-state="selected"`),图表 tooltip 改用 `bg-card border-border rounded-lg` 类而非内联样式,以便钩子生效;`text-[10px]` / `text-[11px]` → `text-10px` / `text-11px`;表格行 / 表头带与表格相同的字号类(让 `font-size` 倍率作用到单元格) |
| `components/Settings.tsx` | 「外观与启动」区块:主题下拉框、主题目录路径、重新扫描、诊断列表;深浅按钮改**偏好**模式,单模式主题下整体禁用并提示;切换时把 `themeId` / `preferredMode` / `theme` 写入设置;收到托盘复位后同步快照;所有开关统一为 `Toggle`(滑块 `switchThumb*`) |

后端(`src-tauri/src/`):

| 文件 | 作用 |
|---|---|
| `themes.rs` | `themes_dir()` / `ensure_dir()` / `discover()`:枚举两种布局、跳过符号链接与隐藏项、大小与数量上限、UTF-8 检查;`RESET_EVENT` / `RESET_SCRIPT`(托盘复位);附单测 |
| `commands.rs` | `list_themes()`(无参数)、`get_themes_dir()`;`save_settings` 补齐缺失的 `preferredMode` |
| `tray.rs` | 托盘菜单「恢复默认主题」(`reset-theme`)→ `reset_theme_to_default()`:改设置、对窗口执行复位脚本、发 `theme://reset`、显示主窗口 |
| `lib.rs` | 注册模块与命令 |
| `settings.rs` | `Settings.theme_id`(JSON `themeId`,默认 `"otr"`)、`preferred_mode`(JSON `preferredMode`,旧文件由 `theme` 推导:`normalize_preferred_mode`)、`reset_theme()`;附单测 |

其它:`docs/theme.schema.json`(JSON Schema,编辑器补全;颜色 token 列表、`css` 的钩子键模式、属性名列表与 `font-size` / `line-height` 的正则由自检脚本保证与代码一致)、`scripts/lib/load-theme.mjs`(脚本共用的 esbuild 打包导入)、`scripts/theme-lint.mjs`(命令行校验,`--print-css`;打印后加 token 的取值与来源)、`scripts/theme-contrast.mjs`(按界面真实用法算前景/背景对比度,分组、`--builtin`、`--only`)、`scripts/theme-css-selftest.mjs`(自检,`npm run test:theme`)、`examples/themes/`(示例主题,见 §12.5)。

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

- **只想做一个模式**:`modes` 里只写 `dark` 或 `light`。用户选你的主题时会临时切到这个模式,深浅按钮整体灰掉;用户原来的深浅偏好不会被改,换回双模式主题时自动恢复。
- **两个模式**:`modes.dark` 和 `modes.light` 各给一套 `colors`。任何没写的 token 都用默认主题**对应模式**的值,所以可以逐步补全。
- **和模式无关的东西**(字体、圆角、阴影、图表调色板、Agent 品牌色)放在顶层 `tokens` 里,两种模式共用;如果某个模式要不一样,再在那个模式里覆盖一次即可。
- **颜色写法**:`"#3b82f6"`、`"#38f"`、`"rgb(59, 130, 246)"`、`"hsl(217 91% 60%)"`、`"217 91% 60%"` 都行;**不要带透明度**(`#ff000080`、`rgba(...)` 会被当成无效并回退)。
- **Agent 品牌色**:`chart.agents` 的键是 Agent id:内置的 `dsh`、`claude-code`、`codex`、`zcode`、`opencode`、`pi`、`cursor`;自定义 Agent 的 id 在设置页的 id 规则是 `custom-<名字>`(可在 `settings.json` 的 `customAgents` 里看到)。只写你想改的。
- **字体**:`font.sans` 是逗号分隔的字体族,例如 `"\"JetBrains Mono\", \"PingFang SC\", sans-serif"`。只能用系统已安装的字体;不能引用文件或网址。
- **圆角**:`"0.5rem"`、`"8px"`、`"0"`;上限 128px / 8rem。
- **阴影**:每层 `[inset] 偏移x 偏移y [模糊] [扩散] [颜色]`,多层用逗号;颜色可以带透明度,例如 `"0 4px 12px rgba(0,0,0,0.35)"`;`"none"` 表示无阴影。
- **单模式主题的 token 放哪**:建议把带颜色的组(`colors`、`stat`、`chart`、`shadow`)都写在那个模式里,只把 `font`、`radius` 放顶层 `tokens`。这些颜色是针对这一种底色调的,以后补另一个模式时不会被误继承;将来若取消 `modes` 层(Q1),迁移也只是把 `modes.<模式>` 合并进 `tokens`。
- **哪些颜色会被当文字**(按正文 4.5:1 选色):`foreground`(放在 `background`、`card` 上)、`mutedForeground`(`background`、`card`、`muted` 上)、`primary`(`card` 上的链接与角标)、`primaryForeground`(`primary` 上)、`destructive`、三个 `*Text`、三个 `*Label`、`notice`、`stat` 的六个颜色。图形类(`ring`、进度条、开关滑块、`chart.palette`、`chart.agents`)按 3:1。逐项说明见 §3.1 / §3.2,`scripts/theme-contrast.mjs` 会把这些组合算一遍。
- **状态色分三类**:`success` / `warning` / `danger` 只画进度条和开关轨道,可以选得鲜艳;卡片上单独出现的状态字(额度剩余百分比、「缓存命中率」、保存成功、定价「手动」、⚠)用 `successLabel` / `warningLabel` / `dangerLabel`;角标淡底和说明文字用 `*Text`。不写 `*Label` 时它们沿用你写的 `success` / `warning` / `dangerText`(老主题因此不变),所以**填充色选得很亮时请把 `*Label` 写上**。
- **开关滑块**:设置页所有开关(额度来源、Agent 启用、启动时最小化到托盘)是同一个组件。滑块「开」态是 `switchThumb`、轨道 `success`;「关」态是 `switchThumbOff`、轨道 `mutedForeground` 30% 叠在卡片上。两态各按 3:1 选:轨道很亮的「开」态用深色滑块,很暗的「关」态用浅色滑块(两态不必同色,`theme-contrast.mjs` 分别检查)。不写时:「开」态沿用你写的 `primaryForeground`;「关」态沿用你写的 `switchThumb`,也没写就用默认主题该模式的「关」态色(暗色白、亮色 zinc-500)。`shadow.base` 仍作用于滑块,可以当装饰(例如发光),不必再靠它补救可见性。
- **hover 叠加色**:`overlay` 不必是纯黑 / 纯白;暖色主题用深褐、冷色主题用带色相的浅色,hover 时更协调(界面固定按 5% 叠加)。
- **token 表达不了的效果**(卡片渐变底、毛玻璃顶栏、标题字距、选中态描边):用 `css` 表,见 §12.6。

完整的 token 名单见 §3;想要编辑器补全,在文件里加 `"$schema": "https://raw.githubusercontent.com/otae-1204/OTR/main/docs/theme.schema.json"`(或指向本仓库 `docs/theme.schema.json` 的本地路径;注意 schema 里的 `$id` 只是标识,不是可下载地址)。§4.1 有一个两种模式齐全的完整示例,§12.5 有两个单模式示例,都可以直接抄。

### 12.3 怎么验证

1. **命令行**(推荐,与应用内逻辑一致;需在 OTR 仓库根目录先 `npm install`):
   ```bash
   node scripts/theme-lint.mjs path/to/theme.json
   ```
   通过时打印 `✓ 通过`,并列出每个模式解析后的关键颜色、调色板与 `css` 规则数;有 `[错误]` 表示应用会拒绝加载,有 `[警告]` 表示那一项会回退默认值(`css` 条目则是被丢弃)。退出码 0 / 1。加 `--print-css` 会把 `css` 表最终生成的样式文本打印出来,方便核对选择器与归一化后的值。
2. **对比度**(可选,同样要先 `npm install`):
   ```bash
   node scripts/theme-contrast.mjs path/to/theme.json
   ```
   按界面里真实的前景 / 背景组合(包括 `bg-success/10` 这类半透明底,按浏览器方式混合)计算 WCAG 对比度:文字按 4.5:1,图形 / 控件按 3:1,全部计入(开关滑块的开 / 关两态也是正式检查项,不再是「参考」)。每个模式最后一行汇总文字最低值、图形最低值和开关两态的对比度。有不达标项时退出码为 1。
   - 分组:`text`(正文、角标、说明)/ `label`(独立状态文字)/ `stat`(统计标签)/ `ui`(焦点环、进度条、额度条)/ `switch`(开关两态)/ `chart`(图表与 Agent 品牌色);`--only label,switch` 只看其中几组。
   - `--builtin` 检查内置默认主题 `otr`。默认主题有一些历史配色达不到阈值(§10、Q18),所以它的全表会失败;Q10 要求的新文字 token 用 `--builtin --only label` 检查(两种模式都应全部通过)。
3. **应用内**:设置页的主题一行下方会列出每个用户主题的错误与警告(带字段路径,如 `modes.dark.colors.prmary: 未知字段,已忽略`);无效主题在下拉框里灰显。
4. **肉眼核对清单**:仪表盘的 Hero 卡(含「缓存命中率」)、四张 Agent 卡、趋势图(线色与图例)、模型占比环、会话表;额度页的进度条与剩余百分比(绿 / 橙 / 红);设置页的开关(开和关都看)、角标、错误提示、定价表「手动」标签;两种模式都切一遍;hover 一下卡片与按钮看叠加色。
5. **弄坏了怎么办**:托盘菜单「恢复默认主题」会把设置里的主题改回内置 `otr` 并立刻生效(不需要能看清设置页),之后修好文件、重新扫描再选回来即可。

### 12.4 常见错误

| 现象 | 原因 |
|---|---|
| 下拉框里是「(无效)」 | 顶层结构错:缺 `apiVersion` / `id` / `name` / `modes`,或 `id` 用了大写、空格 |
| 某个颜色没生效,列表里是警告 | 拼错 token 名(如 `prmary`)、带了透明度、用了颜色关键字(`red`) |
| 改了 `success`,额度百分比 / 「缓存命中率」的颜色没跟着变 | 你写了 `successLabel`,这些文字用的是它;`success` 只管进度条与开关轨道(§3.1) |
| 额度百分比 / 「缓存命中率」的字看不清,而进度条很好看 | 没写 `*Label` 时这些字沿用你写的 `success` / `warning`;给文字单独写 `successLabel` / `warningLabel`(`theme-lint.mjs` 会显示 `(← success)`) |
| 开关「关」态的滑块看不见 | 只写了 `switchThumb`,「关」态沿用了它;单独给「关」态写一个与轨道(`mutedForeground` 30% 叠在卡片上)对比 ≥ 3:1 的 `switchThumbOff` |
| 字体没生效 | 字体族里有括号、斜杠等白名单外字符,或引号不成对 |
| 明明改了文件但没变化 | 没点「重新扫描」;或另一个文件用了同一个 `id`(按文件名排序靠前的那个生效) |
| 选了主题后深浅按钮都灰了 | 主题只提供了一个模式,这是预期行为;你的深浅偏好没被改,换回双模式主题时自动恢复(Q14) |
| 改了 `cardForeground` / `popover` / `secondary` / `accent` 等没有任何变化 | 这些 token 目前界面没有引用(§3.1);卡片文字要改 `foreground` |
| 升级后选回双模式主题,模式不是原来的 | 升级前选过单模式主题时旧版本已把模式改写掉,偏好只能从它推导;用深浅按钮再选一次即可,之后会被记住 |
| 主题把界面弄得看不见、点不到设置 | 托盘图标右键 →「恢复默认主题」 |
| 「不是合法 JSON」但内容看着没错 | JSON 不允许注释和末尾多余的逗号 |
| `css` 里某条没生效,列表里是警告 | 钩子名 / 状态拼错(目录见 §14.2)、属性不在白名单、值里有不允许的东西(颜色关键字 `red`、`url()`、`var(--别的变量)`、`!important`);用 `--print-css` 看实际生成了什么 |
| `font-size` / `line-height` 被丢弃 | 只接受倍率:字号 `0.8em`–`1.2em` 或 `80%`–`120%`,行高无单位 `0.8`–`1.2` 或 `80%`–`120%`;`14px`、`1rem`、`1.5`(行高)、`calc()`、`larger` 都会被拒(§14.3) |
| `font-size` 写了 `1.2em`,图表坐标轴的字没变大 | 坐标轴刻度是图表库按像素画在 SVG 里的,不受倍率影响(Q20);其余文字都按各自原字号缩放 |
| 加了 `css` 之后,选中态 / hover 的效果反而没了 | 主题规则与组件的基础 class **同权重且更靠后**,所以主题赢;组件用普通 class 表达的状态(Agent 卡选中时的 `border-primary`、分段按钮选中时的 `bg-background`、chip 选中时的 `bg-primary/15`)也就被盖掉了。给同一属性再写一条 `钩子:selected`(或 `:hover`)即可,它权重更高、同样在后面,见 §14.6 的例子(策略 Q17 已确认保持) |

### 12.5 示例主题

`examples/themes/` 里有两个完全按本指南制作的第三方主题,安装方法见该目录的 `README.md`:

| 文件 | id / 名称 | 模式 | 风格 |
|---|---|---|---|
| `warm-paper.json` | `warm-paper` / 暖纸 Warm Paper | 只有 `light` | 米白纸张底、墨褐正文、陶土色主色、大地色图表;衬线字体、较大圆角、暖褐阴影;开关「开」米白滑块 /「关」褐灰滑块;附带 8 个钩子的 `css` 演示(顶栏 / 卡片的纸张渐变、选中 Agent 卡的主色晕染、标题字距、页面标题放大 15%、角标描边、tooltip 毛玻璃),见 §12.6 |
| `neon-night.json` | `neon-night` / 霓虹夜 Neon Night | 只有 `dark` | 近黑紫底、青色霓虹主色(深色按钮文字)、品红焦点环;等宽字体、近直角、发光阴影;开关「开」深色滑块 /「关」浅色滑块,`shadow.base` 只作青色微光装饰 |

两者都覆盖了全部 33 个 `colors`(含 `*Label` 与 `switchThumb*`)、6 个 `stat`、`chart` 的三项,以及 `font` / `radius` / `shadow`;布局按 §12.2「单模式主题的 token 放哪」(暖纸的 `css` 也放在 `modes.light` 里,因为渐变里带颜色)。`theme-lint.mjs` 零错误零警告;`theme-contrast.mjs` 全部项目达标:文字最低 5.42:1(霓虹夜)/ 4.64:1(暖纸),开关「开 / 关」12.78 / 9.09:1(霓虹夜)与 5.38 / 3.50:1(暖纸)。都做成单模式;Q1 已确认保留 `modes`,以后可以给它们补另一个模式。

### 12.6 加一点 css(可选)

先看 §14 的钩子目录(§14.2)和属性白名单(§14.3),然后在 `modes.<模式>`(带颜色时)或顶层 `tokens`(只有字距之类时)里加一个 `css` 对象:

```json
"css": {
  "card": {
    "background-image": "linear-gradient(180deg, rgba(255, 253, 248, 0.9) 0%, rgba(255, 253, 248, 0) 55%)"
  },
  "card:hover": { "box-shadow": "0 10px 24px -12px rgba(74, 52, 30, 0.35)" },
  "agent-card:selected": {
    "background-image": "linear-gradient(180deg, hsl(var(--primary) / 0.08), hsl(var(--primary) / 0))"
  },
  "card-title": { "letter-spacing": "0.02em" },
  "page-title": { "font-size": "1.15em" },
  "header": { "backdrop-filter": "blur(12px)" }
}
```

要点:

- **键**是钩子名,可加一个状态:`:hover` / `:active` / `:focus` / `:disabled` / `:selected`(选中的 Agent 卡、分段按钮、chip、打开的开关)。一个键里写多条属性。
- **颜色**写法与 token 一样(`#hex` / `rgb()` / `hsl()`),这里**允许透明度**(渐变和阴影经常需要);还可以引用本主题的语义色:`hsl(var(--primary))`、`hsl(var(--primary) / 0.3)`,以及 `var(--chart-1)`、`var(--agent-dsh)`。不能用颜色关键字(`red`)。
- **渐变**只有 `linear-gradient()` / `radial-gradient()`,里面只能有颜色、长度 / 百分比、角度和 `to right` / `circle at center` 之类的方位词;至少两个色标。
- **想让主题跟着 token 走**:`border-radius: var(--radius-lg)`、`box-shadow: var(--shadow-md)`、`font-family: var(--font-mono)`。
- **字号 / 行高**:只能按组件原值缩放 ±20%:`"font-size": "1.1em"`(或 `"110%"`),`"line-height": "0.9"`(或 `"90%"`)。倍率作用于钩子内**每个文字自己的原字号**,不是父元素的字号;嵌套钩子各写各的,内层覆盖外层、不相乘(§14.3)。
- **改不了的**:绝对字号、间距、尺寸、位置、显示 / 隐藏、动画、`transform`、`content`、`url()`、外链字体。想要这些请提需求,不要绕(也绕不过去,见 §14.4)。
- **注意覆盖**:主题规则会盖掉组件同一属性的基础 class,但组件的 `hover:` 之类伪类 class 仍然更强;组件用普通 class 表达的状态(Agent 卡选中时的主色边框、分段按钮选中时的浅底)会被盖掉,所以给 `card` 写了 `border-color` 之后,记得给 `agent-card:selected` 再写一个(§14.6 有完整例子)。
- **验证**:`node scripts/theme-lint.mjs --print-css 你的主题.json`,每条不合法的声明都会带 JSON 路径列出来;应用设置页也会列出同样的诊断。

## 13. 待确认问题(Open questions)

每个问题都给出选项、我采用的默认方案与理由,并标注是否会**实质性改变主题文件格式**。

**Q1. 主题的粒度:一个主题同时含深浅两种模式,还是一个主题就是一种外观?** —— **已确认 (a)**:保留 `modes` 层与深浅切换,格式不变。
选项:(a) 一个主题可含 `dark` / `light` 两个模式,用户仍可独立切换深浅(**默认**);(b) 一个主题只描述一种外观,深浅切换概念被主题选择取代;(c) 主题只描述一种外观,但清单可声明"配对主题"的 id。
默认理由:现有 UI 已有深浅切换,用户习惯与 `settings.theme` 字段都保留;单模式主题也能表达(只写一个模式)。
改变主题格式:**是**(选 b/c 会移除 `modes` 层)。

**Q2. 内置默认主题的 id / 名称。** —— **已确认(默认 (a))**。
选项:(a) `otr` /「OTR 默认」(**默认**);(b) `default`;(c) `otr-dark` 与 `otr-light` 拆成两个内置主题。
默认理由:`otr` 与产品同名、不会与通用词冲突;拆成两个会让"深浅切换"和"主题切换"打架。
改变主题格式:**否**(只影响保留 id 列表)。

**Q3. 是否开放间距 / 密度 token。** —— **已确认(默认 (a),不开放)**。
选项:(a) 不开放(**默认**);(b) 加一个 `density` 标量(如 0.9–1.1)整体缩放 padding;(c) 开放 Tailwind spacing 的几个关键档位。
默认理由:间距牵涉对齐与可用性,现有布局用固定档位,开放后主题很容易把额度条、表格挤坏;需要时 (b) 是加法,不破坏旧主题。
改变主题格式:**否**(以后加是新增可选字段)。

**Q4. 是否允许主题携带受限的自定义 CSS。** —— **已确认 (b),已实现**(§14):`css` 字段为结构化表而非自由文本,选择器限于 `data-theme-part` 钩子,属性与值逐项白名单,分词校验 + 自检用例。
选项:(a) 不允许;(b) 允许一个 `css` 字段,只接受白名单属性(颜色 / 背景 / 边框 / 圆角 / 阴影)且选择器限于我们公开的 `data-*` 钩子(**已采用**);(c) 允许任意 CSS 但做正则清洗。
实现说明:在 `apiVersion = 1` 内以新增可选字段的方式加入,老主题与老应用行为不变(老应用按未知字段忽略并告警)。
改变主题格式:**否**(加法)。

**Q5. 是否增加 `minAppVersion` 字段。** —— **已确认(默认 (a))**。
选项:(a) 不加,靠 `apiVersion` + 未知字段忽略(**默认**);(b) 加可选 `minAppVersion`,低于时只告警仍加载;(c) 加且低于时拒绝加载。
默认理由:token 只做加法时老应用忽略新 token 即可;真正的破坏性变化用 `apiVersion` 表达更明确。
改变主题格式:**否**(以后加是新增可选字段)。

**Q6. `settings.theme`(模式)字段是否改名为 `colorMode`。** —— **已确认(默认 (a))**。
选项:(a) 保留 `theme` 表示模式、新增 `themeId`(**默认**);(b) 改名 `colorMode` 并做一次迁移;(c) 合并成 `appearance: { themeId, mode }`。
默认理由:改动最小、旧设置文件零迁移;字段含义已在类型注释与本文说明。
改变主题格式:**否**(只是应用设置文件)。

**Q7. 用户主题的文件布局。** —— **已确认(默认 (a))**。
选项:(a) 同时支持 `themes/<name>.json` 与 `themes/<name>/theme.json`(**默认**);(b) 只支持单文件;(c) 只支持目录。
默认理由:单文件最易分享;目录形式给以后放预览图 / 说明留位置;两者都只认一层深度,实现成本几乎为零。
改变主题格式:**否**(清单内容不变)。

**Q8. 主题被删除后设置里的 `themeId` 怎么处理。** —— **已确认(默认 (a))**。
选项:(a) 保留原值,运行时回退并提示(**默认**);(b) 自动改回 `otr`;(c) 提示用户选择。
默认理由:用户可能只是临时移走文件(或同步目录还没到),保留选择更友好;回退提示在设置页可见。
改变主题格式:**否**。

**Q9. 是否随应用附带第二个内置主题(如高对比)。** —— **已确认(默认 (a))**。
选项:(a) 只内置默认主题(**默认**);(b) 加一个高对比 / 无障碍内置主题;(c) 把示例主题作为内置。
默认理由:本次只搭接口;示例主题将按本指南另行制作,验证接口后再决定是否收编为内置。(示例主题已制作,见 §12.5。)
改变主题格式:**否**。

**Q10. 状态「填充色」`success` / `warning` 被直接当文字色用,是否改成对应的 `*Text`。** —— **已确认 (c),已实现**。
选项:(a) 保持现状,在文档里写明这两个填充色也要当文字可读;(b) 把这几处组件改成 `text-success-text` / `text-warning-text`;(c) 新增专门的 token(如 `successLabel`)(**已采用**)。
实现:新增可选颜色 token `successLabel` / `warningLabel` / `dangerLabel`(CSS 变量 `--*-label`,class `text-*-label`)。grep 出的五处「填充色当文字」全部改用它们:额度剩余百分比(≥50% / 25–50% / <25%,原来是 `success` / `warning` / `dangerText`)、「缓存命中率」标题、设置保存成功提示、定价表「手动」来源标签、Agent 卡 ⚠。填充色从此只画条和轨道。缺省回退:主题自己写了「原先那个 token」(`success` / `warning` / `dangerText`)就用它 —— 老主题逐位不变;都没写才用默认主题的值(§3.1「后加 token 的回退」)。选这个回退而不是回退到 `*Text`,是因为旧版指南明确要求 `success` / `warning` 两用可读,按指南做的老主题这几处本来就合格,不该被悄悄换色。默认主题两种模式下这三个 token 对卡片 ≥ 4.5:1(亮 5.49 / 5.01 / 4.83,暗 5.88 / 6.95 / 5.40),因此亮色下这几处、暗色下 <25% 的红字比以前深 / 浅了一档(§10)。`theme-contrast.mjs` 的 `label` 组、自检脚本、schema 同步更新。
改变主题格式:**否**(新增可选 token,加法)。

**Q11. 设置页开关的滑块颜色绑在 `primaryForeground` 上。** —— **已确认 (c),已实现**。
选项:(a) 保持现状,文档写明并建议用 `shadow.base` 描边;(b) 滑块改用 `background` 或 `card`;(c) 新增 `switchThumb` token(**已采用**);(d) 固定白色,不受主题控制。
实现:新增两个可选 token `switchThumb`(「开」态,轨道 `success`)与 `switchThumbOff`(「关」态,轨道 `mutedForeground` 30% 叠在卡片上)。拆成两个是因为一个颜色往往无法同时对两种轨道 ≥ 3:1:霓虹夜的「开」轨道是亮绿、「关」轨道近黑,单色滑块在数学上做不到两边都达标。缺省回退(只认主题自己写的):`switchThumb` ← `primaryForeground`(老主题「开」态不变);`switchThumbOff` ← `switchThumb`,**不**回退到 `primaryForeground` —— 「关」态沿用它正是本问题,所以老主题没写滑块色时「关」态直接用默认主题的值,旧版霓虹夜那种深色 `primaryForeground` 的主题不用改文件就修好了。默认主题:「开」白色(与以前相同),「关」暗色白色、亮色 zinc-500(以前白色在浅灰轨道上只有 1.48:1,现在 3.27:1)。「启动时最小化到托盘」开关以前是另一套实现(轨道 `primary` / `muted`、滑块 `background`、36×20),现在与其它开关统一为同一个 `Toggle` 组件。`theme-contrast.mjs` 把开关两态列为正式检查项(`switch` 组),不再是「参考」;`shadow.base` 仍作用于滑块,只作装饰。默认主题「开」态白滑块对 emerald-500 轨道仍是 2.54:1,见 Q18。
改变主题格式:**否**(新增可选 token,加法)。

**Q12. 当前没有被界面引用的 token 怎么处理。** —— **已确认 (a)**:保留为预留 token,文档注明目前未引用(§3.1),不改组件、不加警告。
选项:(a) 保留为"预留 token",文档注明目前未引用(**已采用**);(b) 让组件真正用起来;(c) 在下一个 `apiVersion` 删掉用不上的;(d) 校验时对这些 token 给出提示性 warning。
改变主题格式:**否**。

**Q13. `$schema` 的正式在线地址。** —— **已确认 (a)**:文档与示例主题用 `https://raw.githubusercontent.com/otae-1204/OTR/main/docs/theme.schema.json`(合并到 `main` 后可用);schema 文件里的 `$id` 保持现状未改。若以后出现 `apiVersion = 2`,再考虑按版本 tag 固定。
选项:(a) 用 `main` 分支的 raw 地址(**已采用**);(b) 按版本 tag 固定;(c) 发布到 GitHub Pages / 自有域名。
改变主题格式:**否**。

**Q14. 选了单模式主题后,深浅模式要不要"记住原来的偏好"。** —— **已确认 (b),已实现**。
选项:(a) 保持现状;(b) 单独记住用户的"偏好模式",选回支持该模式的主题时自动恢复(**已采用**);(c) 不写回设置,只在运行时临时切换。
实现:`settings.json` 新增 `preferredMode`(`settings.rs` 的 `preferred_mode`、`bindings.ts` 的 `Settings.preferredMode`);`theme` 字段保留,含义收窄为**生效**模式(旧版本应用只认它)。旧文件缺 `preferredMode` 时由 `theme` 推导:字段级 `serde(default)` 给空串以区分「缺字段」,`Settings::load` 调 `normalize_preferred_mode()` 补齐,`save_settings` 也补一次,前端再兜一层;Rust 单测覆盖推导、保留、非法值、`load` 与往返。前端 `ThemeProvider` 分开持有生效模式与偏好模式:`selectTheme` / `reload` 按**偏好**决定,`decide()` 在主题不支持时只临时换生效模式并原样带回偏好;深浅按钮改偏好,单模式主题下两个按钮都禁用,说明文字提示「你偏好的 X 会在换回双模式主题时恢复」。首帧防闪:`token-show-theme` 与缓存仍记**生效**模式(首帧要画的就是它),另加 `otr-preferred-mode` 记偏好,只在读不到设置文件时兜底。局限:升级前已经被单模式主题改写过的 `theme` 只能当作偏好(§5)。
改变主题格式:**否**(只影响应用设置与切换逻辑)。

**Q15. `css` 表是否开放字号 / 行高(`font-size` / `line-height`)。** —— **已确认 (c),已实现**:全部钩子开放,限组件原值 ±20%。
选项:(a) 不开放;(b) 只对少数钩子开放 `font-size`;(c) 全钩子开放但限幅(±20%)(**已采用**)。
实现:值只接受**倍率** —— `font-size`:`0.8em`–`1.2em` 或 `80%`–`120%`;`line-height`:无单位 `0.8`–`1.2` 或 `80%`–`120%`;分词器校验,`px` / `rem` / `vw` / `calc()` / `var()` / `clamp()` / 关键字 / 多值一律拒绝。关键在输出:字面的 `font-size: 1.1em` 相对的是**父元素**,会盖掉钩子元素自己的字号类(`text-xs` 12px 在 16px 的父元素里会变成 17.6px,+47%),嵌套时还会累乘,±20% 守不住。所以应用不写字面值,而是在钩子元素上设 `--otr-font-scale` / `--otr-line-scale`,由 `tailwind.config.js` 里每一档字号 / 行高乘上它(默认 1,与原值逐位相同):每个元素 = 自己原来的值 × 倍率;嵌套钩子是覆盖不是相乘。浏览器实测(所有钩子各设 1.2):没有任何文字超过 ×1.2;`card` ×1.2 里的 `card-title` ×0.8 得到 ×0.8。细节与局限见 §14.3。
改变主题格式:**否**(扩大白名单,加法)。

**Q16. 主题把界面弄坏时的兜底入口(「安全模式」)。** —— **已确认 (b),已实现**。
选项:(a) 保持现状,文档说明;(b) 托盘菜单加「恢复默认主题」(**已采用**);(c) 启动时按住某个键 / 连续两次崩溃后自动回退默认主题;(d) 对 `app` / `main` / `settings-section` 这几个钩子禁用 `opacity` 与 `color`。
实现:托盘菜单在「立即刷新」与「退出」之间新增「恢复默认主题」(`reset-theme`,与现有菜单项同样的 `MenuItem::with_id` + `on_menu_event` 分派)。点击后 Rust:`Settings::reset_theme()` 把 `themeId` 改回 `otr`、`theme` 改成偏好模式,其余设置不动,写盘(写盘失败也保留内存新值,本次运行立即生效);对每个窗口 `eval` 编译期常量 `themes::RESET_SCRIPT`(删首帧缓存、删 `<style id="otr-theme-css">`、清 `<html>` 内联变量 —— 前端脚本坏了、监听没挂上也生效);`emit("theme://reset")`,前端 `resetThemeDom()` + 重新 `bootstrap()`,设置页同步自己的快照(否则之后保存别的设置会把旧 `themeId` 写回去);最后显示主窗口。单测:`reset_theme` 只动主题字段、落盘往返、复位脚本内容;自检脚本对照脚本里的键名 / 事件名与前端常量。浏览器实测:先把页面弄成前景 = 背景 + `opacity: 0.02`,模拟托盘复位后恢复为默认主题,随后在设置页切一个开关,保存的 `themeId` 仍是 `otr`。
改变主题格式:**否**。

**Q17. `css` 规则与组件自身 class 的优先级策略。** —— **已确认 (a)**:保持现状;§14.6 与 §12.4 写明优先级规则,以及为什么要用 `:selected` / `:hover` 补写。
选项:(a) 保持现状,文档说明(**已采用**);(b) 主题基础规则改用 `:where()` 降到 0 权重;(c) 提升到 (0,2,0);(d) 把状态 class 改成 `data-theme-state` 驱动的 CSS。
改变主题格式:**否**。

**Q18. 默认主题 `otr` 其余的历史低对比度配色要不要修。**
现状:Q10 / Q11 只动了新 token 覆盖的位置。`node scripts/theme-contrast.mjs --builtin` 的全表里,默认主题仍有不达标项(配色是改造前就有的):暗色 11 项(`primaryForeground` / `primary` 按钮文字 3.27、主色角标 4.02、`destructive` 文字 2.47–2.78、`dangerText` 3.97、`notice` 角标 4.27、统计标签 input / output 3.96–4.27、额度条 success / danger 对轨道 1.87–2.78、开关「开」白滑块对 emerald-500 轨道 2.54);亮色 30 项(主色当链接文字 3.27、`*Text` 角标与说明 2.85–3.76、`notice` 角标 2.41、`mutedForeground` / `muted` 4.43、统计标签 2.15–3.96、进度条与额度条填充对轨道 1.25–2.40、9 个图表 / Agent 色 1.98–2.80、开关「开」2.54)。其中「额度条」几项是本轮检查脚本改为按额度页真实轨道(`foreground` 25%,见 commit 227e4f4)计算后才暴露的。
选项:(a) 保持现状,这是产品的既有外观(**默认**);(b) 只修文字类(`text` / `stat` 组)到 4.5:1,图形不动;(c) 全面按对比度脚本修到全部达标(含开关「开」态:加深 `success` 或给开关加独立的轨道 token);(d) 另做一个「高对比」内置主题(即 Q9 的 (b)),默认主题不动。
默认理由:这些都是默认外观的品牌色,改动面大、需要设计判断;本轮只按 Q10 / Q11 的授权动了新 token。验证时默认主题用 `--builtin --only label` 检查新文字 token(全部通过)。
改变主题格式:**否**((c) 若加开关轨道 token 是加法)。

**Q19. `notice` 同样是「填充 + 文字」两用,要不要也拆。**
现状:`notice` 既画「有新版本」小圆点,也当「可更新」角标的文字(底色是它自己的 15%),与 Q10 之前的 `success` / `warning` 同一类问题;默认主题亮色下这个角标只有 2.41:1(暗色 4.27:1)。Q10 的范围是 `success` / `warning`,本轮没动它。
选项:(a) 保持现状,文档已写明 `notice` 要当文字可读(**默认**);(b) 新增可选 `noticeText`,回退到本主题写了的 `notice`,再回退默认主题(与 Q10 相同的做法);(c) 角标文字改用 `foreground`,`notice` 只作圆点与淡底。
默认理由:只有一个角标、出现频率低;(b) 是加法,随时可做。
改变主题格式:**否**((b) 是新增可选 token)。

**Q20. 图表坐标轴文字要不要跟随 `font-size` 倍率。**
现状:趋势图的坐标轴刻度由图表库按像素(`fontSize: 12`)画进 SVG,不经过 Tailwind 字号类,所以 `css` 里给 `chart` / `chart-card` / `app` 写的 `font-size` 倍率对它无效(其余文字都有效,§14.3)。
选项:(a) 保持现状,文档说明(**默认**);(b) 组件读取生效的 `--otr-font-scale` 再传给图表库(需要在主题切换时重绘);(c) 刻度改用 CSS 控制字号。
默认理由:刻度是图表的一部分,放大后更容易重叠;真有需求再做 (b)。
改变主题格式:**否**。

## 14. 受限自定义 CSS(`css` 字段)

### 14.1 格式

`css` 是 `tokens` 对象里的一个可选字段(公共 `tokens.css`,或按模式 `modes.dark.css` / `modes.light.css`),结构是一张两层的表:

```json
"css": {
  "<钩子>": { "<属性>": "<值>", "...": "..." },
  "<钩子>:<状态>": { "<属性>": "<值>" }
}
```

- **键** = 钩子名(§14.2),可选地加一个状态后缀:`hover` / `active` / `focus` / `disabled` / `selected`。每个键最多 24 条声明,整张表最多 64 个键。
- **属性** = §14.3 白名单里的 CSS 属性名(小写,前后空白会被去掉)。
- **值** = 字符串,最长 256 字符,按该属性的语法校验后重新格式化。

应用把每个键翻译成一个选择器:`card` → `[data-theme-part~="card"]`,`card:hover` → `[data-theme-part~="card"]:hover`,`card:focus` → `…:focus-visible`,`card:disabled` → `…:disabled`,`card:selected` → `…[data-theme-state~="selected"]`。主题里不存在选择器这个概念,也就没有「选择器清洗」的问题。

**为什么是结构化表而不是自由 CSS 文本 + 解析器**:(1) 选择器根本不在输入里,钩子名只是白名单里的一个词;(2) 每个属性的值语法可以单独收紧(渐变里只准颜色和长度、`var()` 只准本主题变量),不需要一个完整的 CSS 语法解析器,也不引入依赖;(3) 诊断能精确到 JSON 路径(`modes.dark.css.card.position`),作者一眼能看到哪条被丢;(4) JSON Schema 能给编辑器做钩子名与属性名补全;(5) 与 token 一样按「键 → 属性」叠加合并。自由文本能表达的东西(嵌套选择器、`@media`、动画)恰恰是我们不想开放的。

### 14.2 钩子目录与稳定性承诺

组件上的 `data-theme-part` 属性是主题可以依赖的**稳定接口**。一个元素可以同时属于多个钩子(空格分隔,如 `data-theme-part="card stat-card"`),选择器按词匹配;有状态的元素另带 `data-theme-state="selected"`。

| 钩子 | 界面位置 | 状态 |
|---|---|---|
| `app` | 应用根容器(页面底色与正文色;Cookie 教程窗口的根也是它) | — |
| `header` | 固定顶栏(logo、工具组) | — |
| `main` | 顶栏下方的内容区 | — |
| `filter-bar` | 仪表盘的筛选栏(Agent chip + 日期范围) | — |
| `page-title` | 「额度」「设置」页的大标题 | — |
| `card` | **所有**卡片容器(下面每种卡片都同时带 `card`) | hover |
| `card-title` | 卡片标题行(统计卡的标题、趋势图 / 模型占比 / 会话明细的标题、设置分区标题、额度账号名) | — |
| `stat-card` | 仪表盘 Hero 统计卡 | hover |
| `mini-stat` | 统计卡里的六个指标小格 | — |
| `agent-card` | Agent 卡片(整张是按钮) | hover / active / focus / **selected** |
| `chart-card` | 趋势图卡、模型占比卡 | hover |
| `table-card` | 会话明细卡 | hover |
| `limit-card` | 额度页每一行账号卡 | hover |
| `settings-section` | 设置页的每个分区卡 | hover |
| `chart` | 图表绘图区容器(趋势图、环形图) | — |
| `tooltip` | 图表浮层(趋势图 / 环形图) | — |
| `legend` | 图例容器(趋势图底部、环形图右侧列表) | — |
| `table` / `table-head` / `table-row` | 会话表与定价表的 `<table>` / `<thead>` / 数据行 `<tr>` | `table-row`: hover |
| `button` | 描边按钮(刷新、重新扫描、保存路径、删除账号等) | hover / active / focus / disabled |
| `button-primary` | 实心主色按钮(保存、添加) | hover / active / focus / disabled |
| `segmented` | 分段控件轨道(顶栏工具组、趋势图「对比 / 堆叠」、深浅模式) | — |
| `segmented-button` | 分段控件里的按钮 | hover / focus / disabled / **selected** |
| `chip` | 筛选栏的 Agent / 日期范围 chip | hover / focus / **selected** |
| `badge` | 小角标(「N 次请求」「更新于」、套餐名、「待配置」、「可更新」、「松开合并」、数据源状态) | — |
| `progress` / `progress-fill` | 进度条轨道 / 填充(缓存命中率、Agent 占比、额度剩余) | — |
| `switch` / `switch-thumb` | 开关轨道 / 滑块(额度来源、Agent 启用、启动最小化 —— 现在是同一个组件;滑块颜色优先用 token `switchThumb` / `switchThumbOff`) | `switch`: hover / focus / disabled / **selected**(= 打开) |
| `input` | 文本输入框、下拉框、日期框 | hover / focus / disabled |
| `empty-state` | 「暂无数据」之类的虚线空状态框 | — |

未列出状态的钩子也能写 `:hover` 等,只是通常没有意义(例如 `page-title:hover`)。

**稳定性承诺**:

- 同一 `apiVersion` 内,钩子名**不改名、不删除**;只会新增。新增的钩子在旧应用里只是「没人匹配」,不报错。
- 钩子标记的是**语义**(「所有卡片」「主按钮」),不是具体 DOM 结构;界面重排时钩子会跟着元素走,但一个钩子对应的元素数量、嵌套层级可能变化。
- 状态名同样只增不减;`selected` 的含义固定为「当前选中 / 打开」。
- 不承诺元素的 Tailwind class、内联样式或子结构不变 —— 主题只能依赖钩子和 §14.3 的属性,这也是选择器不开放的原因。
- 钩子目录的唯一事实来源是 `src/theme/css.ts` 的 `THEME_PARTS`;JSON Schema 与本表由自检脚本对照(`npm run test:theme`)。

### 14.3 属性白名单与值语法

值先经过分词器:允许的字符只有字母、数字、空格与 `- + . % # ( ) , /`;括号必须成对且前面有函数名。然后按属性语法逐 token 匹配,最后由应用**重新拼出**字符串(颜色 → `#rrggbb` 或 `rgb(r g b / a)`,数字 → 最多三位小数 + 单位,关键字 → 白名单里的原词)。

| 属性 | 允许的值 |
|---|---|
| `color`、`background-color`、`border-color`(1–4 个)、`border-{top,right,bottom,left}-color`、`outline-color`、`text-decoration-color` | `<颜色>` |
| `background-image` | `none`,或最多 4 个 `linear-gradient()` / `radial-gradient()`;每个 2–16 个色标(`<颜色> [位置]{0,2}`),线性可带角度(`deg/turn/rad/grad`)或 `to <边> [<边>]`,径向可带 `circle/ellipse/closest-*/farthest-*`、长度和 `at <位置>` |
| `background-clip` | `border-box` / `padding-box` / `content-box` / `text`(同时写出 `-webkit-` 前缀,配合渐变 + `color: transparent` 做渐变文字) |
| `border`、`border-{top,right,bottom,left}` | `[宽度] [样式] [颜色]` 任意顺序各至多一个,或 `none` |
| `border-width`(1–4)、`border-*-width` | 长度 0–8px / 0–0.5rem/em |
| `border-style`(1–4)、`border-*-style` | `none` / `solid` / `dashed` / `dotted` / `double` |
| `border-radius`(1–4)、`border-*-*-radius`(1–2) | 长度或百分比:0–1000px / 0–64rem/em / 0–100%;`border-radius` 还可写 `var(--radius-md|lg|xl)` |
| `box-shadow` | 与 `shadow` token 相同的语法(每层 `[inset] 2–4 个长度 [颜色]`,长度 ≤ 128px / 8rem,颜色可带透明度但须是字面量),或 `var(--shadow|--shadow-sm|md|lg)` |
| `text-shadow` | 每层 `2–3 个长度 [颜色]`,不允许 `inset` |
| `opacity` | 0–1 或 0–100% |
| `font-family` | 与 `font` token 相同的字体族语法(这是唯一允许引号的属性,它不走分词器、走 token 的白名单),或 `var(--font-sans|--font-mono)` |
| `font-weight` | `normal` / `bold` / `lighter` / `bolder` 或 1–1000 的整数 |
| `font-style` | `normal` / `italic` / `oblique` |
| `letter-spacing` | `normal` 或长度 ±8px / ±0.5rem/em |
| `text-transform` | `none` / `uppercase` / `lowercase` / `capitalize` |
| `text-decoration-line` | `none` 或最多两个 `underline` / `overline` / `line-through` |
| `text-decoration-style` | `solid` / `double` / `dotted` / `dashed` / `wavy` |
| `backdrop-filter` | `none`,或最多 3 个 `blur(≤ 40px)` / `saturate()` / `brightness()` / `contrast()`(0–3 或 0–300%);同时写出 `-webkit-` 前缀 |
| `font-size` | 单个倍率:`0.8em`–`1.2em` 或 `80%`–`120%`(相对**组件原字号**,见下)。输出为 `--otr-font-scale: <倍率>` |
| `line-height` | 单个倍率:无单位 `0.8`–`1.2` 或 `80%`–`120%`(相对**组件原行高**,不是 CSS 里「字号的几倍」)。输出为 `--otr-line-scale: <倍率>` |

**字号 / 行高为什么是倍率变量(Q15)**

目标是「任何文字的字号都在组件原值的 ±20% 以内」。只限制写法(`em` / `%`)做不到这一点,因为字面的 `font-size: 1.1em` 有两个问题:

1. **相对的是父元素,不是元素自己**。主题规则与 Tailwind 字号类同权重且更靠后,会**替换**钩子元素自己的 `text-xs`;于是 `card-title`(12px,父元素 16px)写 `1.1em` 得到 17.6px,是原值的 147%。
2. **嵌套累乘**。`card` 写 `1.2em`、里面的 `card-title` 再写 `1.2em`,文字就是 1.44 倍;钩子一多就无从保证上限。

所以应用不按字面输出。校验通过的值被换算成纯倍率,写进钩子元素的一个自定义属性:

```css
[data-theme-part~="card-title"] { --otr-font-scale: 1.1; }
```

`tailwind.config.js` 里每一档字号与行高都乘这个变量(`text-xs` = `calc(0.75rem * var(--otr-font-scale, 1))`,行高同理乘 `--otr-line-scale`;变量默认 1,默认外观逐位不变)。于是:

- **每个元素 = 自己原来的字号 × 倍率**。倍率被限制在 0.8–1.2,结果就一定在原值 ±20% 内,不管它在哪个钩子里、父元素多大。
- **嵌套是覆盖,不是相乘**。自定义属性按继承取**最近**一个钩子设的值:`card` ×1.2 里的 `card-title` ×0.8 就是 ×0.8(浏览器实测 0.8,不是 0.96)。没写倍率的内层元素继承外层的倍率,也还是「原值 × 一个 0.8–1.2 的数」。
- 字号与行高分开:只放大字号时行高不变,行高本来就是固定长度的地方(大多数 `text-*` 档位)行盒高度不变,额度行、顶栏这些按行高排版的地方不会被撑开;可以另写 `line-height` 倍率。

局限与约定:

- 倍率只作用于**带字号类的元素**(界面文字几乎都有)。字号完全继承自钩子**外面**的文字不会缩放 —— 会缩小的是覆盖面而不是上限。为此表格行 / 表头这些「单元格字号靠继承」的钩子元素上重复写了与表格相同的字号类;实测所有钩子设 1.2 时,钩子内文字要么 ×1.2、要么 ×1,没有别的值。
- 图表坐标轴刻度由图表库按像素画进 SVG,不受影响(Q20)。
- 组件里不能再写 `text-[13px]`、`leading-[1.1]` 这类任意值(它们不乘倍率);自检脚本会扫描 `src/` 并对 `tailwind.config.js` 的每一档做检查。
- `--otr-font-scale` / `--otr-line-scale` 是应用自己的变量:主题不能用 `var()` 引用,也不能直接写;首帧缓存里出现也会被丢掉。

`<颜色>` 的写法:`#rgb` / `#rrggbb` / `#rrggbbaa`、`rgb()` / `rgba()` / `hsl()` / `hsla()`(纯数字参数)、`transparent`、`currentColor`,以及 **本主题体系的变量**:`hsl(var(--<语义色或 stat 色>))`、`hsl(var(--primary) / 0.3)`、`var(--chart-1…16)`、`var(--agent-<id>)`。语义色变量是 HSL 三元组,所以必须包在 `hsl()` 里;`--chart-N` / `--agent-*` 是 `#rrggbb`,直接用。这里的颜色**允许透明度**(与 token 不同),因为渐变、阴影、描边天然需要。

### 14.4 明确禁止

以下内容在分词或语法阶段就被拒绝,对应条目丢弃并告警,不会有「部分生效」:

- **任何 URL 与外部资源**:`url()`、`image-set()` / `-webkit-image-set()`、`element()`、`paint()`、`cross-fade()`、`@import`、`@font-face`、外链字体。分词器不接受引号、`@`、`:`,`url(` 也不在任何属性的语法里。
- **表达式与脚本残余**:`expression()`、`attr()`、`env()`、`calc()`、`color-mix()`、`conic-gradient()` / `repeating-*-gradient()`(未列入白名单的函数一律拒绝)。
- **`var()` 越权引用**:只允许 §14.3 列出的本主题变量;`var(--tw-*)`、`var(--anything-else)`、带默认值的 `var(--x, red)` 都拒绝。
- **破坏布局或可用性的属性**:`position`、`display`、`visibility`、`width` / `height` / `min-*` / `max-*`、`margin`、`padding`、`inset` / `top` / `left`、`z-index`、`transform`、`content`、`animation`、`transition`、`overflow`、`pointer-events`、`float`、`flex` / `grid` / `gap` / `order`、`filter`(只开放 `backdrop-filter`)、`clip-path`、`mask*`、`border-image`、`background`(简写)、`background-size` / `-position` / `-attachment`、`cursor`、`user-select`、`appearance`、`-webkit-text-fill-color`、`font`(简写)。属性名不在白名单就拒绝,不是靠列举黑名单。
- **绝对字号 / 行高**:`font-size` / `line-height` 只收 0.8–1.2 的倍率;`14px`、`1rem`、`1.3em`、`calc()`、`var()`、`min()` / `max()` / `clamp()`、`larger` / `smaller` / `inherit` 等关键字、`vw` / `ex` / `ch` 等单位、多个值都拒绝(自检用例覆盖)。
- **结构注入**:`;`、`{`、`}`、`!important`、`/* */` 注释、`\` 转义、`<`、`>`、控制字符、非 ASCII 字符(`font-family` 除外)。这些字符不在分词器的字符集里,整条值直接拒绝。
- **任意选择器**:键只能是钩子名 [+ 一个状态];`body`、`*`、`card, body`、`card > x`、`card:nth-child(1)`、`card::before`、`card:visited` 都是「未知钩子 / 未知状态」。
- **颜色关键字**(`red`、`inherit`、`initial`、`unset`)与 `lab()` / `lch()` / `oklch()` 等新色彩空间:与 token 一致,不接受。
- **超限**:值 > 256 字符、一个键 > 24 条声明、整表 > 64 个键(多出的丢弃)、渐变 > 4 个或 > 16 个色标、模糊 > 40px、边框 > 8px。

### 14.5 校验、诊断与注入方式

**校验**(`validateCss`,`src/theme/css.ts`):`css` 不是对象 → 整个字段忽略并 warning;每个键 → `parseCssKey`(钩子 + 状态);每条声明 → `normalizeDeclaration`(属性白名单 → 分词 → 属性语法 → 重新拼出)。任一环节失败只丢**那一条**,其余照常;整个主题**不会**因为 `css` 出错而被拒绝(级别永远是 warning)。诊断带完整 JSON 路径与固定文案,例如:

```
[警告] modes.dark.css.card.position: 不支持的属性,已忽略
[警告] modes.dark.css.card.background-image: 值不符合该属性的语法,已忽略
[警告] modes.dark.css.card-title.font-size: 值不符合该属性的语法(只接受相对组件原字号的倍率 0.8em–1.2em 或 80%–120%),已忽略
[警告] tokens.css.card:visited: 未知状态「visited」(可用:hover / active / focus / disabled / selected),已忽略
[警告] tokens.css.body: 未知钩子,已忽略(钩子目录见 docs/theme_interface.md §14.2)
```

**自检**:`npm run test:theme`(`scripts/theme-css-selftest.mjs`)对同一份代码跑一千二百余条断言:分词器字符集、每个白名单属性 × 十几种注入串、`var()` 越权、键 / 选择器、限额、诊断路径、合并、序列化输出形状(含字号 / 行高只输出倍率变量)、缓存清洗、端到端(清单 → 解析 → 文本)、字号 / 行高的越界与注入用例,以及 JSON Schema(颜色 token、钩子、属性列表、字号 / 行高正则)、`tailwind.config.js`、`index.css` 兜底变量、Rust 复位脚本与代码的一致性。

**注入**(`applyResolvedTheme`,`src/theme/apply.ts`):解析后的表(`ResolvedTheme.css`)由 `serializeThemeCss` 变成文本 —— 选择器由钩子名生成,属性只出白名单,每个值在序列化前**再归一化一遍**(幂等),再过一次字符集检查。文本放进**唯一一个** `<style id="otr-theme-css">`:不存在则创建,存在则整体替换 `textContent`,主题没有 css 时移除;每次都 `appendChild` 到 `<head>` 末尾,保证在 Vite 注入的样式之后。切换主题 / 模式 / 重新扫描都会走这一步,不会残留上一个主题的规则。首帧防闪的 localStorage 缓存里存的是**表**而不是文本,读回时整张重新校验(`sanitizeCss`)后再序列化。CSP 的 `style-src 'self' 'unsafe-inline'` 本来就允许内联样式;本功能没有改动 CSP。

### 14.6 优先级与叠加

策略已确认保持(Q17 = (a)),规则如下。

- **叠加**:与 token 一样,公共 `tokens.css` → 该模式 `modes.<mode>.css`,按「键 → 属性」粒度合并;内置主题没有 css。
- **与组件样式的关系**:主题规则的选择器权重是 (0,1,0),与 Tailwind 工具类相同;因为 `<style>` 在最后,同权重时**主题规则赢**。所以 `card { background-image }` 能盖在 `bg-card` 之上,`card { border-color }` 会盖掉 `border-border`。
- **组件的伪类工具类更强**:`hover:border-primary/60`、`disabled:opacity-50` 是 (0,2,0),仍然生效;想连 hover 也接管,给 `card:hover` 再写一次(权重 (0,2,0) 且更靠后,主题赢)。
- **陷阱**:组件用普通 class 表达的状态(Agent 卡选中时的 `border-primary`、分段按钮选中时的 `bg-background`、chip 选中时的 `bg-primary/15`)也是 (0,1,0),与主题的基础规则同权重,于是**被主题盖掉** —— 选中的 Agent 卡看起来和没选中一样。补法是给同一属性再写一条 `:selected`:`钩子:selected` 生成 `[data-theme-part~="agent-card"][data-theme-state~="selected"]`,权重 (0,2,0),比组件的普通 class 高,又在最后,所以一定生效:

  ```json
  "css": {
    "card":                { "border-color": "#d9cbb2" },
    "agent-card:selected": { "border-color": "hsl(var(--primary))" },
    "segmented-button":          { "background-color": "transparent" },
    "segmented-button:selected": { "background-color": "hsl(var(--card))" }
  }
  ```

  规律:**给某个钩子写了一个属性,就检查带这个钩子的元素(例如 Agent 卡同时带 `card` 与 `agent-card`)在选中时组件是否也改同一个属性**;是的话补写 `:selected`。`:hover` 不存在这个问题:组件的 hover 类本来就是 (0,2,0),不会被基础规则盖掉;补写 `钩子:hover` 只是为了换成自己的 hover 效果。
- **字号 / 行高**不参与这场比较:它们输出的是倍率变量,组件的字号类照常生效,只是乘上了倍率(§14.3)。
- **内联样式不受影响**:少数元素用内联样式(Agent 卡图标底色、进度条宽度、图表 SVG 里的颜色)—— 这些由 token(`chart.agents` 等)控制,`css` 表改不了,也不应该改。

