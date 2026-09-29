/** @type {import('tailwindcss').Config} */
// 颜色 / 圆角 / 阴影 / 字体全部引用 CSS 变量:变量的值由主题决定(src/theme/),
// index.css 里是 JS 跑起来之前的兜底(= 内置默认主题)。token 清单见 docs/theme_interface.md。
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
        /** 状态色:DEFAULT 用于填充,text 用于浅色底上的文字 */
        success: {
          DEFAULT: "hsl(var(--success))",
          text: "hsl(var(--success-text))",
        },
        warning: {
          DEFAULT: "hsl(var(--warning))",
          text: "hsl(var(--warning-text))",
        },
        danger: {
          DEFAULT: "hsl(var(--danger))",
          text: "hsl(var(--danger-text))",
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
