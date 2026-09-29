import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { CookieGuide, cookieGuideProvider } from "./components/CookieGuide";
import { ThemeProvider } from "./theme/ThemeProvider";
import { bootTheme } from "./theme/apply";
import "./index.css";

// 首帧:同步应用本机记忆的深浅模式与上次主题的变量缓存,避免闪一下默认配色;
// 真正的主题(设置里选的 + 主题目录里的文件)由 ThemeProvider 异步加载后覆盖。
bootTheme();

const guide = cookieGuideProvider();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider>
      {guide ? <CookieGuide provider={guide} /> : <App />}
    </ThemeProvider>
  </React.StrictMode>,
);
