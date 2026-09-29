/** @type {import('tailwindcss').Config} */
// 颜色 / 圆角 / 阴影 / 字体全部引用 CSS 变量:变量的值由主题决定(src/theme/),
// index.css 里是 JS 跑起来之前的兜底(= 内置默认主题)。token 清单见 docs/theme_interface.md。

// 字号与行高都乘一个倍率变量(默认 1,与原值逐位相同)。主题的 css 表写 `font-size` /
// `line-height` 时,应用只在钩子元素上设置这两个变量(限 0.8–1.2,见 docs §14.3),
// 所以每个元素最终是「自己原来的字号 × 倍率」;嵌套钩子是覆盖不是相乘。
// 组件里不要再写 `text-[Npx]` / `leading-[…]` 这类任意值(不会跟着缩放),自检脚本会查。
const fs = (size) => `calc(${size} * var(--otr-font-scale, 1))`;
const lh = (height) => `calc(${height} * var(--otr-line-scale, 1))`;

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["selector", ".dark"],
  theme: {
    extend: {
      colors: {
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        /** hover 叠加基色:用法 `hover:bg-overlay/5` */
        overlay: "hsl(var(--overlay))",
        /** 状态色:DEFAULT 用于填充,text 用于角标 / 说明文字,label 用于卡片上的独立状态文字 */
        success: {
          DEFAULT: "hsl(var(--success))",
          text: "hsl(var(--success-text))",
          label: "hsl(var(--success-label))",
        },
        warning: {
          DEFAULT: "hsl(var(--warning))",
          text: "hsl(var(--warning-text))",
          label: "hsl(var(--warning-label))",
        },
        danger: {
          DEFAULT: "hsl(var(--danger))",
          text: "hsl(var(--danger-text))",
          label: "hsl(var(--danger-label))",
        },
        /** 设置页开关的滑块:开 / 关 */
        switch: {
          thumb: "hsl(var(--switch-thumb))",
          "thumb-off": "hsl(var(--switch-thumb-off))",
        },
        info: "hsl(var(--info))",
        notice: "hsl(var(--notice))",
        /** 统计卡各项指标的强调色 */
        stat: {
          input: "hsl(var(--stat-input))",
          output: "hsl(var(--stat-output))",
          "cache-read": "hsl(var(--stat-cache-read))",
          "cache-write": "hsl(var(--stat-cache-write))",
          calls: "hsl(var(--stat-calls))",
          cost: "hsl(var(--stat-cost))",
        },
      },
      borderRadius: {
        md: "var(--radius-md)",
        lg: "var(--radius-lg)",
        xl: "var(--radius-xl)",
      },
      boxShadow: {
        sm: "var(--shadow-sm)",
        DEFAULT: "var(--shadow)",
        md: "var(--shadow-md)",
        lg: "var(--shadow-lg)",
      },
      fontFamily: {
        sans: "var(--font-sans)",
        mono: "var(--font-mono)",
      },
      // Tailwind 3.4 的默认字号 / 行高,只是乘上了倍率变量;另加两档小字号代替原来的 text-[10px] / text-[11px]
      fontSize: {
        "10px": fs("0.625rem"),
        "11px": fs("0.6875rem"),
        xs: [fs("0.75rem"), lh("1rem")],
        sm: [fs("0.875rem"), lh("1.25rem")],
        base: [fs("1rem"), lh("1.5rem")],
        lg: [fs("1.125rem"), lh("1.75rem")],
        xl: [fs("1.25rem"), lh("1.75rem")],
        "2xl": [fs("1.5rem"), lh("2rem")],
        "3xl": [fs("1.875rem"), lh("2.25rem")],
        "4xl": [fs("2.25rem"), lh("2.5rem")],
      },
      lineHeight: {
        none: lh("1"),
        tight: lh("1.25"),
        snug: lh("1.375"),
        normal: lh("1.5"),
        relaxed: lh("1.625"),
        loose: lh("2"),
        3: lh(".75rem"),
        4: lh("1rem"),
        5: lh("1.25rem"),
        6: lh("1.5rem"),
        7: lh("1.75rem"),
        8: lh("2rem"),
        9: lh("2.25rem"),
        10: lh("2.5rem"),
      },
      keyframes: {
        "fade-in": {
          from: { opacity: "0", transform: "translateY(4px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
        "slide-up": {
          from: { opacity: "0", transform: "translateY(12px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
      },
      animation: {
        "fade-in": "fade-in 0.25s ease-out",
        "slide-up": "slide-up 0.3s ease-out",
      },
    },
  },
  plugins: [],
};
