import { useMemo, useState } from "react";
import {
  api,
  type PeakTier,
  type PriceEntry,
  type Settings as AppSettings,
} from "../../api/bindings";
import { fetchUsdCnyRate } from "../../lib/remote";
import type { ConfirmOptions } from "../ConfirmDialog";
import { SelectMenu } from "../SelectMenu";
import { CoinsIcon, RefreshIcon, TrashIcon, XIcon } from "../icons";
import {
  ModelProviderPicker,
  type ModelsDevCandidate,
} from "./ModelProviderPicker";
import { SectionCard } from "./SettingsUI";

const CANDIDATES_CACHE_KEY = "token-show-models-dev-candidates";

type PricingFilter = "all" | "multi" | "unpriced";

function readCachedCandidates(): Record<string, ModelsDevCandidate[]> {
  try {
    const raw = localStorage.getItem(CANDIDATES_CACHE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, ModelsDevCandidate[]> = {};
    for (const [model, list] of Object.entries(parsed)) {
      if (!Array.isArray(list)) continue;
      const valid = list.filter(
        (c): c is ModelsDevCandidate =>
          !!c &&
          typeof c === "object" &&
          typeof (c as ModelsDevCandidate).provider === "string" &&
          !!(c as ModelsDevCandidate).entry &&
          typeof (c as ModelsDevCandidate).entry.input === "number",
      );
      if (valid.length > 0) out[model] = valid;
    }
    return out;
  } catch {
    return {};
  }
}

function writeCachedCandidates(map: Record<string, ModelsDevCandidate[]>) {
  try {
    localStorage.setItem(CANDIDATES_CACHE_KEY, JSON.stringify(map));
  } catch {
    // 写不进去就只在当前会话里保留
  }
}

type BasePriceField = "input" | "output" | "cacheRead" | "cacheWrite";

function withoutKey<T>(map: Record<string, T>, key: string): Record<string, T> {
  if (!(key in map)) return map;
  const copy = { ...map };
  delete copy[key];
  return copy;
}

function roundPrice(n: number): number {
  return Math.round(n * 1e9) / 1e9;
}

function buildPeakTier(
  base: Pick<PriceEntry, BasePriceField>,
  multiplier: number,
): PeakTier {
  return {
    input: roundPrice(base.input * multiplier),
    output: roundPrice(base.output * multiplier),
    cacheRead: roundPrice(base.cacheRead * multiplier),
    cacheWrite: roundPrice(base.cacheWrite * multiplier),
  };
}

function inferPeakMultiplier(p: PriceEntry | undefined): number | null {
  if (!p?.peak) return null;
  const pairs: [number, number][] = [
    [p.input, p.peak.input],
    [p.output, p.peak.output],
    [p.cacheRead, p.peak.cacheRead],
    [p.cacheWrite, p.peak.cacheWrite],
  ];
  for (const [off, on] of pairs) {
    if (off > 0 && Number.isFinite(on) && on > 0) {
      return Math.round((on / off) * 1e4) / 1e4;
    }
  }
  return null;
}

function familyProvider(model: string): string | null {
  const m = model.toLowerCase();
  if (/^(gpt|o\d|chatgpt)/.test(m)) return "openai";
  if (/^claude/.test(m)) return "anthropic";
  if (/^deepseek/.test(m)) return "deepseek";
  if (/^(gemini|gemma)/.test(m)) return "google";
  if (/^qwen/.test(m)) return "qwen";
  if (/^kimi/.test(m)) return "moonshotai";
  if (/^glm/.test(m)) return "zai";
  if (/^grok/.test(m)) return "xai";
  if (/^minimax/.test(m)) return "minimax";
  return null;
}

export function PricingSection({
  settings,
  models,
  setSettings,
  persist,
  confirm,
}: {
  settings: AppSettings | null;
  models: string[];
  setSettings: (next: AppSettings) => void;
  persist: (next: AppSettings) => Promise<void>;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
}) {
  const [fxState, setFxState] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [fxMsg, setFxMsg] = useState("");
  const [fetchState, setFetchState] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [fetchMsg, setFetchMsg] = useState("");
  const [candidatesByModel, setCandidatesByModel] = useState<
    Record<string, ModelsDevCandidate[]>
  >(readCachedCandidates);
  const [filter, setFilter] = useState<PricingFilter>("all");
  const [query, setQuery] = useState("");
  const [peakDrafts, setPeakDrafts] = useState<Record<string, string>>({});
  const [pendingMultipliers, setPendingMultipliers] = useState<Record<string, number>>({});

  /** 更新某个模型的单价字段(输入/输出/缓存读/缓存写,$/M),仅改本地状态;若已设峰谷倍率则同步重算 peak */
  const updatePrice = (model: string, field: BasePriceField, raw: string) => {
    if (!settings) return;
    const cur: PriceEntry = settings.pricing[model] ?? {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
    };
    const multiplier = inferPeakMultiplier(cur) ?? pendingMultipliers[model] ?? null;
    const v = parseFloat(raw);
    const next: PriceEntry = { ...cur, [field]: Number.isFinite(v) && v >= 0 ? v : 0 };
    if (multiplier && multiplier > 0) {
      next.peak = buildPeakTier(next, multiplier);
    }
    setSettings({
      ...settings,
      pricing: { ...settings.pricing, [model]: next },
      pricingSource: { ...settings.pricingSource, [model]: "manual" },
    });
  };

  /**
   * 峰价倍率:输入倍率 m(> 0)后统一按平价 × m 生成整套高峰档(输入/输出/缓存读/写),
   * 留空(或 <= 0)则清除峰谷档、退回纯平价。
   */
  const updatePeakMultiplier = (model: string, raw: string) => {
    setPeakDrafts((prev) => ({ ...prev, [model]: raw }));
    if (!settings) return;
    const cur = settings.pricing[model] ?? {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
    };
    const v = parseFloat(raw);
    const valid = Number.isFinite(v) && v > 0;
    setPendingMultipliers((prev) =>
      valid ? { ...prev, [model]: v } : withoutKey(prev, model),
    );
    const peak = valid ? buildPeakTier(cur, v) : null;
    const next: PriceEntry = { ...cur, peak };
    setSettings({
      ...settings,
      pricing: { ...settings.pricing, [model]: next },
      pricingSource: { ...settings.pricingSource, [model]: "manual" },
    });
  };

  const commitPeakMultiplier = (model: string) => {
    setPeakDrafts((prev) => withoutKey(prev, model));
    persistNow();
  };

  const persistNow = () => {
    if (!settings) return;
    void api.saveSettings(settings).catch(() => undefined);
  };

  const clearPrice = (model: string) => {
    if (!settings) return;
    setPeakDrafts((prev) => withoutKey(prev, model));
    setPendingMultipliers((prev) => withoutKey(prev, model));
    const pricing = { ...settings.pricing };
    delete pricing[model];
    const pricingSource = { ...settings.pricingSource };
    delete pricingSource[model];
    const next = { ...settings, pricing, pricingSource };
    setSettings(next);
    void api.saveSettings(next).catch(() => undefined);
  };

  const selectCandidate = (model: string, cand: ModelsDevCandidate) => {
    if (!settings) return;
    setPeakDrafts((prev) => withoutKey(prev, model));
    setPendingMultipliers((prev) => withoutKey(prev, model));
    const next: AppSettings = {
      ...settings,
      pricing: { ...settings.pricing, [model]: { ...cand.entry } },
      pricingSource: {
        ...(settings.pricingSource ?? {}),
        [model]: `models.dev:${cand.provider}`,
      },
    };
    void persist(next);
  };

  const updateCurrency = (currency: string) => {
    if (!settings) return;
    const next = { ...settings, currency };
    setSettings(next);
    void api.saveSettings(next).catch(() => undefined);
  };

  const updateExchangeRate = (raw: string) => {
    if (!settings) return;
    const v = parseFloat(raw);
    const next = {
      ...settings,
      exchangeRate: Number.isFinite(v) && v > 0 ? v : settings.exchangeRate,
    };
    setSettings(next);
  };

  const fetchFxRate = async () => {
    if (!settings) return;
    setFxState("loading");
    setFxMsg("正在获取实时汇率…");
    try {
      const res = await fetchUsdCnyRate();
      if (!res) throw new Error("unavailable");
      const next = { ...settings, exchangeRate: res.rate };
      setSettings(next);
      await api.saveSettings(next);
      setFxState("ok");
      setFxMsg(`已更新为 ¥${res.rate.toFixed(4)}/$(来源:${res.source})`);
    } catch {
      setFxState("error");
      setFxMsg("汇率获取失败,请检查网络后手动填写");
    }
  };

  /** 从 models.dev 拉取全量目录,保留每个本地模型的所有候选供应商供用户后续切换 */
  const fetchModelsDev = async () => {
    if (!settings) return;
    setFetchState("loading");
    setFetchMsg("正在拉取 models.dev 目录…");
    try {
      const res = await fetch("https://models.dev/api.json");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const toEntry = (m: any): PriceEntry | null => {
        const c = m?.cost;
        if (!c || typeof c.input !== "number") return null;
        return {
          input: c.input ?? 0,
          output: c.output ?? 0,
          cacheRead: c.cache_read ?? 0,
          cacheWrite: c.cache_write ?? 0,
        };
      };

      const catalog = new Map<string, Map<string, PriceEntry>>();
      for (const [pid, prov] of Object.entries<any>(data ?? {})) {
        for (const [id, m] of Object.entries<any>(prov?.models ?? {})) {
          const entry = toEntry(m);
          if (!entry) continue;
          const key = id.toLowerCase();
          let byProv = catalog.get(key);
          if (!byProv) {
            byProv = new Map();
            catalog.set(key, byProv);
          }
          if (!byProv.has(pid)) byProv.set(pid, entry);
        }
      }

      const matchModel = (
        model: string,
      ): { chosen: ModelsDevCandidate; candidates: ModelsDevCandidate[] } | null => {
        const fullKey = model.toLowerCase();
        const bareKey = model.split("/").pop()?.toLowerCase() ?? "";
        const prefix = model.includes("/") ? model.split("/")[0].toLowerCase() : null;
        const keys = [fullKey, bareKey].filter(
          (k, i, all) => k.length > 0 && all.indexOf(k) === i,
        );

        for (const k of keys) {
          const byProv = catalog.get(k);
          if (!byProv || byProv.size === 0) continue;
          const fam = familyProvider(k);
          const list: ModelsDevCandidate[] = [...byProv.entries()]
            .map(([provider, entry]) => ({
              provider,
              entry,
              official: fam === provider,
            }))
            .sort((a, b) => {
              if (!!a.official !== !!b.official) return a.official ? -1 : 1;
              if (prefix) {
                const pa = a.provider.toLowerCase() === prefix;
                const pb = b.provider.toLowerCase() === prefix;
                if (pa !== pb) return pa ? -1 : 1;
              }
              return a.provider.localeCompare(b.provider);
            });

          const prevSrc = (settings.pricingSource ?? {})[model];
          const prevProv = prevSrc?.startsWith("models.dev:")
            ? prevSrc.slice("models.dev:".length)
            : null;
          const chosen =
            (prevProv && list.find((x) => x.provider === prevProv)) ||
            (prefix && list.find((x) => x.provider.toLowerCase() === prefix)) ||
            list.find((x) => x.official) ||
            list[0];
          return { chosen, candidates: list };
        }
        return null;
      };

      const samePrice = (a: PriceEntry, b: PriceEntry) =>
        a.input === b.input &&
        a.output === b.output &&
        a.cacheRead === b.cacheRead &&
        a.cacheWrite === b.cacheWrite;

      const nextCandidates: Record<string, ModelsDevCandidate[]> = {};
      const picked: { model: string; found: ModelsDevCandidate }[] = [];
      for (const model of models) {
        const matched = matchModel(model);
        if (!matched) continue;
        nextCandidates[model] = matched.candidates;
        picked.push({ model, found: matched.chosen });
      }
      setCandidatesByModel(nextCandidates);
      writeCachedCandidates(nextCandidates);

      const conflicts = picked.filter(({ model, found }) => {
        const cur = settings.pricing[model];
        if (!cur) return false;
        const src = (settings.pricingSource ?? {})[model];
        if (src && src.startsWith("models.dev:")) return false;
        return !samePrice(cur, found.entry);
      });
      let keepMine = false;
      if (conflicts.length > 0) {
        const names = conflicts
          .slice(0, 12)
          .map((c) => c.model)
          .join("\n");
        const more =
          conflicts.length > 12 ? `\n…还有 ${conflicts.length - 12} 个` : "";
        const overwrite = await confirm({
          title: `覆盖 ${conflicts.length} 个已填写的模型定价？`,
          description:
            `以下模型你已填写过定价，且与官方价不同：\n\n${names}${more}\n\n` +
            "选择「用官方价覆盖」会替换这些模型；选择「保留手动定价」只同步其余模型。",
          confirmLabel: "用官方价覆盖",
          cancelLabel: "保留手动定价",
          tone: "primary",
        });
        keepMine = !overwrite;
      }
      const skip = new Set(keepMine ? conflicts.map((c) => c.model) : []);
      const next = { ...settings.pricing };
      const nextSource = { ...(settings.pricingSource ?? {}) };
      let matchedCount = 0;
      for (const { model, found } of picked) {
        if (skip.has(model)) continue;
        next[model] = found.entry;
        nextSource[model] = `models.dev:${found.provider}`;
        matchedCount++;
      }
      await persist({ ...settings, pricing: next, pricingSource: nextSource });
      const multiTotal = Object.values(nextCandidates).filter((l) => l.length >= 2).length;
      setFetchState("ok");
      setFetchMsg(
        `已匹配 ${matchedCount} / ${models.length} 个模型` +
          (multiTotal > 0 ? `（其中 ${multiTotal} 个有多家供应商可选）` : "") +
          (skip.size > 0 ? `，保留了 ${skip.size} 个手动定价` : ""),
      );
    } catch (err) {
      console.error("[Settings] models.dev 获取失败", err);
      setFetchState("error");
      setFetchMsg("获取失败,请检查网络后重试(也可手动填写)");
    }
  };

  const multiCount = useMemo(
    () => models.filter((m) => (candidatesByModel[m]?.length ?? 0) >= 2).length,
    [models, candidatesByModel],
  );
  const unpricedCount = useMemo(
    () => models.filter((m) => !settings?.pricing[m]).length,
    [models, settings],
  );

  const visibleModels = useMemo(() => {
    const q = query.trim().toLowerCase();
    return models.filter((m) => {
      if (filter === "multi" && (candidatesByModel[m]?.length ?? 0) < 2) return false;
      if (filter === "unpriced" && settings?.pricing[m]) return false;
      if (q && !m.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [models, filter, query, candidatesByModel, settings]);

  return (
    <SectionCard
      icon={<CoinsIcon className="h-4 w-4 text-primary" />}
      title="成本定价"
      summary={
        settings
          ? `${models.length} 个模型 · ${Object.keys(settings.pricing).length} 个已定价`
          : undefined
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={() => void fetchModelsDev()}
            disabled={fetchState === "loading" || !settings}
            data-theme-part="button-primary"
            className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${fetchState === "loading" ? "animate-spin" : ""}`}
            />
            从 models.dev 自动获取
          </button>
          <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
            币种
            <SelectMenu
              value={settings?.currency ?? "CNY"}
              onChange={updateCurrency}
              ariaLabel="币种"
              className="h-7"
              options={[
                { value: "CNY", label: "¥ 人民币" },
                { value: "USD", label: "$ 美元" },
              ]}
            />
          </label>
          <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
            汇率(¥/$)
            <input
              type="number"
              step="0.1"
              min="0"
              value={settings?.exchangeRate ?? 7.2}
              onChange={(e) => updateExchangeRate(e.target.value)}
              onBlur={persistNow}
              data-theme-part="input"
              className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-xs tabular-nums outline-none focus:border-primary"
            />
          </label>
          <button
            type="button"
            onClick={() => void fetchFxRate()}
            disabled={fxState === "loading" || !settings}
            title="从 er-api / frankfurter 获取实时 USD→CNY 汇率"
            data-theme-part="button"
            className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
          >
            获取实时汇率
          </button>
          {fxMsg ? (
            <span
              className={`text-xs ${
                fxState === "error" ? "text-destructive" : "text-muted-foreground"
              }`}
            >
              {fxMsg}
            </span>
          ) : null}
          {fetchMsg ? (
            <span
              className={`text-xs ${
                fetchState === "error" ? "text-destructive" : "text-muted-foreground"
              }`}
            >
              {fetchMsg}
            </span>
          ) : null}
        </div>

        {models.length > 0 ? (
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div data-theme-part="segmented" className="flex items-center gap-1 rounded-xl bg-muted p-1">
              {(
                [
                  { id: "all", label: `全部 ${models.length}` },
                  { id: "multi", label: `多供应商可选 ${multiCount}` },
                  { id: "unpriced", label: `未定价 ${unpricedCount}` },
                ] as const
              ).map((item) => (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setFilter(item.id)}
                  data-theme-part="segmented-button"
                  data-theme-state={filter === item.id ? "selected" : undefined}
                  className={`h-7 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 ${
                    filter === item.id
                      ? "bg-background shadow-sm text-foreground"
                      : "text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                  }`}
                >
                  {item.label}
                </button>
              ))}
            </div>
            <div className="relative flex items-center">
              <input
                type="text"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="搜索模型名…"
                aria-label="搜索模型名"
                data-theme-part="input"
                className="h-8 w-48 rounded-lg border border-border bg-background px-2.5 pr-7 text-xs outline-none focus:border-primary"
              />
              {query ? (
                <button
                  type="button"
                  onClick={() => setQuery("")}
                  aria-label="清空搜索"
                  className="absolute right-1.5 flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                >
                  <XIcon className="h-3 w-3" />
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        <div className="overflow-x-auto scroll-thin">
          <table data-theme-part="table" className="w-full table-fixed text-xs">
            {/* 表头 / 行上重复写 text-xs:单元格的字号是继承来的,钩子元素自己带字号类,
                主题 css 的 font-size 倍率(docs §14.3)才作用得到 */}
            <thead data-theme-part="table-head" className="text-xs">
              <tr className="border-b border-border/60 text-left text-muted-foreground">
                <th className="py-1.5 pr-2 font-medium">模型</th>
                <th className="w-[78px] whitespace-nowrap py-1.5 pr-1.5 text-right font-medium">输入 $/M</th>
                <th className="w-[78px] whitespace-nowrap py-1.5 pr-1.5 text-right font-medium">输出 $/M</th>
                <th className="w-[78px] whitespace-nowrap py-1.5 pr-1.5 text-right font-medium">缓存读 $/M</th>
                <th className="w-[78px] whitespace-nowrap py-1.5 pr-1.5 text-right font-medium">缓存写 $/M</th>
                <th
                  className="w-[78px] whitespace-nowrap py-1.5 pr-1.5 text-right font-medium"
                  title="峰谷分时计价倍率（如填 2 表示北京时间工作日 9-12、14-18 点按平价的 2 倍计费）。留空表示无峰谷"
                >
                  高峰倍率
                </th>
                <th className="w-9 whitespace-nowrap py-1.5 text-right font-medium">操作</th>
              </tr>
            </thead>
            <tbody>
              {models.length === 0 ? (
                <tr>
                  <td colSpan={7} className="py-3 text-muted-foreground">
                    暂无模型数据,先用 Agent 跑几轮再回来配置
                  </td>
                </tr>
              ) : visibleModels.length === 0 ? (
                <tr>
                  <td colSpan={7} className="py-4 text-center text-muted-foreground">
                    {filter === "multi" && multiCount === 0
                      ? "先点击上方「从 models.dev 自动获取」即可查看并切换各模型的候选供应商"
                      : "没有匹配的模型"}
                  </td>
                </tr>
              ) : (
                visibleModels.map((model) => {
                  const p: PriceEntry | undefined = settings?.pricing[model];
                  const src = settings?.pricingSource?.[model];
                  const candidates = candidatesByModel[model] ?? [];
                  const selectedProvider = src?.startsWith("models.dev:")
                    ? src.slice("models.dev:".length)
                    : null;
                  const sourceLabel = !p
                    ? "无定价 · 用自带成本或 0"
                    : src === "manual"
                      ? "手动填写"
                      : src?.startsWith("curated:")
                        ? `内置修正价 · ${src.slice("curated:".length)}`
                        : src?.startsWith("models.dev:")
                          ? `官方 · ${src.slice("models.dev:".length)}`
                          : "来源未知";
                  const canPickProvider =
                    candidates.length >= 2 ||
                    (candidates.length === 1 &&
                      selectedProvider !== candidates[0].provider);
                  const pillLabel =
                    candidates.length >= 2
                      ? `${!p ? "可选供应商" : sourceLabel} · ${candidates.length}家可选`
                      : !p
                        ? `从 models.dev 选用 (${candidates[0]?.provider ?? ""})`
                        : sourceLabel;
                  const inferredMultiplier =
                    inferPeakMultiplier(p) ?? pendingMultipliers[model] ?? null;
                  const peakValue =
                    model in peakDrafts
                      ? peakDrafts[model]
                      : inferredMultiplier !== null
                        ? String(inferredMultiplier)
                        : "";
                  const hasPeakDisplay = peakValue !== "";
                  const cell = (field: BasePriceField) => (
                    <input
                      type="number"
                      step="0.01"
                      min="0"
                      value={p ? String(p[field] ?? 0) : ""}
                      placeholder="0"
                      onChange={(e) => updatePrice(model, field, e.target.value)}
                      onBlur={persistNow}
                      data-theme-part="input"
                      className="no-spinner h-7 w-16 rounded-lg border border-border bg-background px-2 text-right tabular-nums outline-none focus:border-primary"
                    />
                  );
                  return (
                    <tr
                      key={model}
                      data-theme-part="table-row"
                      className="border-b border-border/30 text-xs"
                    >
                      <td className="min-w-0 py-1.5 pr-2" title={model}>
                        <div className="truncate">{model}</div>
                        {canPickProvider ? (
                          <ModelProviderPicker
                            model={model}
                            label={pillLabel}
                            tone={src === "manual" ? "warning" : "primary"}
                            selectedProvider={selectedProvider}
                            candidates={candidates}
                            onPick={(cand) => selectCandidate(model, cand)}
                          />
                        ) : (
                          <div
                            className={`mt-0.5 truncate text-10px ${
                              !p
                                ? "text-muted-foreground/50"
                                : src === "manual"
                                  ? "text-warning-label"
                                  : "text-muted-foreground/70"
                            }`}
                          >
                            {sourceLabel}
                          </div>
                        )}
                      </td>
                      <td className="py-1.5 pr-1.5 text-right">{cell("input")}</td>
                      <td className="py-1.5 pr-1.5 text-right">{cell("output")}</td>
                      <td className="py-1.5 pr-1.5 text-right">{cell("cacheRead")}</td>
                      <td className="py-1.5 pr-1.5 text-right">{cell("cacheWrite")}</td>
                      <td className="py-1.5 pr-1.5 text-right">
                        <div className="relative inline-flex items-center">
                          <input
                            type="number"
                            step="0.01"
                            min="0"
                            value={peakValue}
                            placeholder="—"
                            title="高峰倍率（如填 2 表示高峰时段输入/输出/缓存单价均为平价 × 2）；留空 = 无峰谷"
                            onChange={(e) => updatePeakMultiplier(model, e.target.value)}
                            onBlur={() => commitPeakMultiplier(model)}
                            data-theme-part="input"
                            className={`no-spinner h-7 w-16 rounded-lg border border-border bg-background text-right tabular-nums outline-none focus:border-primary ${
                              hasPeakDisplay ? "pl-2 pr-5" : "px-2"
                            }`}
                          />
                          {hasPeakDisplay ? (
                            <span className="pointer-events-none absolute right-2 text-muted-foreground/70">
                              ×
                            </span>
                          ) : null}
                        </div>
                      </td>
                      <td className="py-1.5 text-right">
                        <button
                          type="button"
                          onClick={() => clearPrice(model)}
                          disabled={!p}
                          title="清除该模型定价"
                          className="inline-flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-30"
                        >
                          <TrashIcon className="h-3.5 w-3.5" />
                        </button>
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </div>
    </SectionCard>
  );
}
