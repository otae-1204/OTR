import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type AgentStatus,
  type Settings,
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
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(true);

  const mountedRef = useRef(true);
  const debounceRef = useRef<number | null>(null);
  // in-flight 去重 + 尾沿重跑:30 秒轮询和 usage://updated 事件经常撞在一起,
  // 并发拉取除了白跑两遍,还会让先发的旧响应覆盖后发的新响应。
  const inFlightRef = useRef(false);
  const pendingRef = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlightRef.current) {
      pendingRef.current = true;
      return;
    }
    inFlightRef.current = true;
    try {
      do {
        pendingRef.current = false;
        try {
          const [nextSummary, nextAgents, nextSettings] = await Promise.all([
            api.getSummary(),
            api.listAgents(),
            api.getSettings(),
          ]);
          if (!mountedRef.current) return;
          setSummary(nextSummary);
          setAgents(nextAgents);
          setSettings(nextSettings);
        } catch (err) {
          console.error("[useUsageData] 拉取数据失败", err);
        } finally {
          if (mountedRef.current) setLoading(false);
        }
      } while (pendingRef.current && mountedRef.current);
    } finally {
      inFlightRef.current = false;
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

  return { summary, agents, settings, loading, refresh };
}
