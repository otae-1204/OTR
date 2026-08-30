import type { ReactNode } from "react";

/** 加载骨架:虚线边框 + 脉冲动画,默认 h-28,可用 className 覆盖高度 */
export function Skeleton({ className }: { className?: string }) {
  return (
    <div
      className={`animate-pulse rounded-xl border border-dashed border-muted-foreground/30 bg-muted/30 ${
        className ?? "h-28"
      }`}
    />
  );
}

/** 空状态:虚线边框 + 居中提示文案 */
export function EmptyState({
  message,
  icon,
  children,
}: {
  message: string;
  icon?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="rounded-xl border border-dashed border-border px-6 py-10 text-center text-muted-foreground">
      {icon ? <div className="mb-2 flex justify-center">{icon}</div> : null}
      <p className="text-sm">{message}</p>
      {children ? <div className="mt-3">{children}</div> : null}
    </div>
  );
}
