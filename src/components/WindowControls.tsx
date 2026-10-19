import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MinusIcon, RestoreIcon, SquareIcon, XIcon } from "./icons";

/** macOS 用系统红绿灯,不画这组 Windows 按钮。 */
export function isMacPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const platform = nav.userAgentData?.platform ?? nav.platform ?? "";
  return /mac/i.test(platform) || /Macintosh/.test(nav.userAgent);
}

/** 无边框窗口的最小化 / 最大化 / 关闭。macOS 不挂载,交给系统红绿灯。 */
export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let alive = true;
    const win = getCurrentWindow();
    win
      .isMaximized()
      .then((v) => {
        if (alive) setMaximized(v);
      })
      .catch(() => undefined);
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => undefined);
      })
      .then((stop) => {
        if (!alive) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  const btn =
    "flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-overlay/10 hover:text-foreground";

  return (
    <div className="mr-3 flex h-full shrink-0 items-center gap-0.5">
      <button
        type="button"
        title="最小化"
        aria-label="最小化"
        className={btn}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={() => getCurrentWindow().minimize()}
      >
        <MinusIcon className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        title={maximized ? "还原" : "最大化"}
        aria-label={maximized ? "还原" : "最大化"}
        className={btn}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={() => getCurrentWindow().toggleMaximize()}
      >
        {maximized ? (
          <RestoreIcon className="h-3.5 w-3.5" />
        ) : (
          <SquareIcon className="h-3.5 w-3.5" />
        )}
      </button>
      <button
        type="button"
        title="关闭"
        aria-label="关闭"
        className={`${btn} hover:bg-destructive/15 hover:text-destructive`}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={() => getCurrentWindow().close()}
      >
        <XIcon className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
