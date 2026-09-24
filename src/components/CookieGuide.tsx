import { useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type CookieGuideProvider = "qwen" | "stepfun";

declare global {
  interface Window {
    __OTR_GUIDE__?: string;
  }
}

function asProvider(value: string | null | undefined): CookieGuideProvider | null {
  return value === "qwen" || value === "stepfun" ? value : null;
}

/** 教程窗口才有值。主窗口返回 null,页面不会自己弹出来。 */
export function cookieGuideProvider(): CookieGuideProvider | null {
  const fromScript = asProvider(window.__OTR_GUIDE__);
  if (fromScript) return fromScript;
  const fromQuery = asProvider(new URLSearchParams(window.location.search).get("guide"));
  if (fromQuery) return fromQuery;
  try {
    const label = getCurrentWindow().label;
    if (label === "guide-qwen") return "qwen";
    if (label === "guide-stepfun") return "stepfun";
  } catch {
    // 浏览器预览没有 Tauri
  }
  return null;
}

function UrlLine({ url }: { url: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="mt-1.5 flex items-center gap-2">
      <code className="min-w-0 flex-1 truncate rounded-md bg-muted px-2 py-1 font-mono text-[11px]">
        {url}
      </code>
      <button
        type="button"
        onClick={() => {
          void navigator.clipboard.writeText(url).then(
            () => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1200);
            },
            () => undefined,
          );
        }}
        className="shrink-0 text-[11px] font-medium text-primary"
      >
        {copied ? "已复制" : "复制"}
      </button>
    </div>
  );
}

function Steps({
  steps,
}: {
  steps: { text: string; url?: string }[];
}) {
  return (
    <ol className="mt-4 space-y-3">
      {steps.map((step, i) => (
        <li key={step.text} className="flex gap-3">
          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[11px] font-semibold text-primary">
            {i + 1}
          </span>
          <div className="min-w-0 pt-0.5 text-sm leading-relaxed">
            {step.text}
            {step.url ? <UrlLine url={step.url} /> : null}
          </div>
        </li>
      ))}
    </ol>
  );
}

function Notes({ children }: { children: ReactNode }) {
  return (
    <div className="mt-5 rounded-xl border border-border bg-card px-3.5 py-3">
      <div className="text-xs font-medium">注意</div>
      <ul className="mt-1.5 list-disc space-y-1.5 pl-4 text-xs leading-relaxed text-muted-foreground">
        {children}
      </ul>
    </div>
  );
}

function QwenGuide() {
  return (
    <>
      <h1 className="text-base font-semibold">获取百炼控制台 Cookie</h1>
      <Steps
        steps={[
          {
            text: "用浏览器打开百炼 Coding Plan 页面,并登录。",
            url: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
          },
          { text: "按 F12 打开开发者工具,切到「网络 / Network」。" },
          { text: "刷新页面,在筛选框输入 bailian-cs。" },
          {
            text: "点开发往 bailian-cs.console.aliyun.com 的那条请求,在请求头里复制整段 Cookie。",
          },
          { text: "回到设置,贴进「百炼控制台 Cookie」,点保存。" },
        ]}
      />
      <Notes>
        <li>复制网络面板里那条请求的 Cookie。从页面上复制,或在控制台执行 document.cookie,网关会拒绝。</li>
        <li>Coding Plan API Key 查不了订阅额度,可以不填。</li>
        <li>
          国际站打开 Model Studio 的 Coding Plan,同样从网络面板复制请求头 Cookie。
          <UrlLine url="https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=globalset#/efm/coding_plan" />
        </li>
      </Notes>
    </>
  );
}

function StepFunGuide() {
  return (
    <>
      <h1 className="text-base font-semibold">获取 StepFun 控制台 Cookie</h1>
      <Steps
        steps={[
          {
            text: "用浏览器打开 StepFun 控制台,并登录。",
            url: "https://platform.stepfun.com",
          },
          { text: "按 F12 打开开发者工具,切到「网络 / Network」,然后刷新页面。" },
          {
            text: "在任意一条发往 platform.stepfun.com 的请求上右键,选「复制」→「以 cURL 格式复制 / Copy as cURL」。",
          },
          { text: "把整段粘贴到「控制台 Cookie」,点保存。" },
        ]}
      />
      <Notes>
        <li>整段 cURL、Cookie 请求头,或 Oasis-Token 都可以。</li>
        <li>Oasis-Token 中间的三个点是内容的一部分,不要删掉。</li>
        <li>只贴一个 JWT 大约 30 分钟就会失效。用 cURL 会带上刷新令牌,可以自动续期。</li>
        <li>API Key 只查按量余额,不填也能看订阅额度。</li>
      </Notes>
    </>
  );
}

export function CookieGuide({ provider }: { provider: CookieGuideProvider }) {
  return (
    <div className="min-h-screen overflow-auto bg-background px-5 py-5 text-foreground">
      {provider === "qwen" ? <QwenGuide /> : <StepFunGuide />}
    </div>
  );
}
