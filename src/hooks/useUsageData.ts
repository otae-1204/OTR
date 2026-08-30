import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type AgentStatus,
  type UsageSummary,
} from "../api/bindings";

const POLL_INTERVAL_MS = 30_000;
const EVENT_DEBOUNCE_MS = 300;

/**
 * 全局用量数据:一次拉取 summary + agents;
 * - loading 仅在首次加载为 true(骨架屏),refresh 为后台静默刷新;
 * - 订阅 "usage://updated" 事件 + 30s 兜底轮询,卸载时全部清理。
 */
export function useUsageData() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [agents, setAgents] = useState<AgentStatus[]>([]);
  const [loading, setLoading] = useState(true);

  const mountedRef = useRef(true);
  const debounceRef = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextSummary, nextAgents] = await Promise.all([
        api.getSummary(),
        api.listAgents(),
      ]);
      if (!mountedRef.current) return;
      setSummary(nextSummary);
      setAgents(nextAgents);
    } catch (err) {
      console.error("[useUsageData] 拉取数据失败", err);
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  /** 事件可能短时间连发,做尾沿防抖 */
  const scheduleRefresh = useCallback(() => {
    if (debounceRef.current != null) window.clearTimeout(debounceRef.current);
    debounceRef.current = window.setTimeout(() => {
      debounceRef.current = null;
      void refresh();
    }, EVENT_DEBOUNCE_MS);
  }, [refresh]);

  useEffect(() => {
    mountedRef.current = true;
    void refresh();

    let unlisten: (() => void) | undefined;
    let active = true;
    const setupListener = async () => {
      try {
        const stop = await listen("usage://updated", () => scheduleRefresh());
        if (!active) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (err) {
        console.error("[useUsageData] 订阅 usage://updated 失败", err);
      }
    };
    void setupListener();

    const timer = window.setInterval(() => void refresh(), POLL_INTERVAL_MS);

    return () => {
      active = false;
      mountedRef.current = false;
      window.clearInterval(timer);
      if (debounceRef.current != null) {
        window.clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
      unlisten?.();
    };
  }, [refresh, scheduleRefresh]);

  return { summary, agents, loading, refresh };
}
