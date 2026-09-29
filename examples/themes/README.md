# 示例主题

两个按[主题作者指南](../../docs/theme_interface.md#12-主题作者指南author-guide)制作的第三方主题,用来验证主题接口,也可以当模板改。两个都是**单模式**主题(只提供一种外观)。

| 文件 | id | 名称 | 模式 | 风格 |
|---|---|---|---|---|
| `warm-paper.json` | `warm-paper` | 暖纸 Warm Paper | 只有亮色 | 米白纸张底、墨褐正文、陶土色主色、大地色图表;衬线字体、较大圆角、暖褐色柔和阴影;开关「开」米白 /「关」褐灰滑块;附带少量 `css` 演示(顶栏 / 卡片的纸张渐变、选中 Agent 卡的主色晕染、标题字距、页面标题放大 15%、角标描边、tooltip 毛玻璃) |
| `neon-night.json` | `neon-night` | 霓虹夜 Neon Night | 只有暗色 | 近黑紫底、青色霓虹主色、品红焦点环、高饱和图表;等宽字体、近直角、发光阴影;开关「开」深色 /「关」浅色滑块 |

两个主题都写全了 33 个 `colors`,包括后加的 `successLabel` / `warningLabel` / `dangerLabel`(卡片上的独立状态文字)与 `switchThumb` / `switchThumbOff`(开关滑块的开 / 关两态),可以当这些 token 的写法参考。

## 安装

1. 打开 OTR → 设置 → 「外观与启动」,「主题」一行下方显示的就是主题目录。Windows 上一般是 `%APPDATA%\com.otae.radar\themes\`(目录不存在时应用会自动创建;以设置页显示的路径为准)。
2. 把 `.json` 文件复制进去(只认 `.json`,本 README 不用复制):

   ```powershell
   # Windows PowerShell,在仓库根目录执行
   Copy-Item examples\themes\*.json "$env:APPDATA\com.otae.radar\themes\"
   ```

   ```bash
   # macOS / Linux:把目标换成设置页显示的目录
   cp examples/themes/*.json "<主题目录>/"
   ```

3. 回到设置页点「重新扫描」,在「主题」下拉框里选「暖纸 Warm Paper」或「霓虹夜 Neon Night」,立即生效,不用重启。

注意:

- 这两个主题都只有一种模式。选中后应用会临时切到该模式(暖纸 → 亮色,霓虹夜 → 暗色),深浅按钮灰掉;你原来的深浅偏好不会被改,再选回「OTR 默认」时自动恢复。
- 字体只用系统已安装的字体,不会下载:暖纸的西文优先用 Charter / Sitka Text / Cambria 等衬线字体,中文在 macOS 上用宋体(Songti SC)、Windows 上用微软雅黑;霓虹夜的西文优先用 JetBrains Mono / Cascadia Code / Consolas 等等宽字体,中文用苹方 / 微软雅黑。列出的字体都没有时退回系统默认字体。
- 不想要了:删掉文件后点「重新扫描」,应用会回退默认主题并提示「找不到主题」,再选一次「OTR 默认」即可。改坏了看不清界面时,用托盘菜单的「恢复默认主题」。

## 校验

在仓库根目录(先 `npm install`):

```bash
node scripts/theme-lint.mjs examples/themes/*.json       # 格式校验:应为零错误、零警告
node scripts/theme-lint.mjs --print-css examples/themes/warm-paper.json   # 顺便打印 css 表生成的样式
node scripts/theme-contrast.mjs examples/themes/*.json   # 对比度:文字 ≥ 4.5:1,图形与开关两态 ≥ 3:1
```

## 拿来改

复制一份,**先改 `id`**(小写字母、数字、`-` `_` `.`;不能与其它主题重复,也不能是 `otr`)和 `name`,再改颜色。没写的 token 会回退到默认主题同模式的值,所以删掉不关心的部分也没关系。完整的 token 名单与各 token 在界面上的用处见设计文档 §3 与 §12。

想加自定义样式(卡片渐变、标题字距、标题略放大之类),看设计文档 §12.6 与 §14:`css` 表只认公开的钩子名和白名单属性,暖纸主题里的那几条可以直接照抄。字号 / 行高只能按组件原值 ±20% 缩放(`"font-size": "1.15em"`)。

`$schema` 指向仓库 `main` 分支上的 `docs/theme.schema.json`,在 VS Code 等编辑器里可获得补全与校验;离线时可改成本地路径(例如在本仓库内用 `../../docs/theme.schema.json`)。应用本身忽略这个字段。
