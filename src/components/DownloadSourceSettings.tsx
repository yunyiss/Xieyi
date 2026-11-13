import { useCallback, useEffect, useState } from "react";
import {
  getDownloadSourceConfig,
  setDownloadSourceConfig,
  type DownloadSourceConfig,
} from "../lib/ipc";
import { Select } from "./Select";

/**
 * 下载源设置。
 *
 * ## 为什么需要给用户这个开关
 *
 * 引擎与模型的下载地址在不同的服务上，而**各服务在不同网络环境下
 * 可达性差异极大**。实测（2026-11-13）：
 *
 * | 目标 | 结果 |
 * |---|---|
 * | `huggingface.co` | ❌ 完全不可达 |
 * | `github.com` 直连 | ⚠️ 时通时不通（4 次里 3 次超时） |
 * | `gh-proxy.com` | ✅ 稳定 |
 * | `modelscope.cn` | ✅ 快（0.38s） |
 *
 * 我们内置了默认顺序与自动回退，但**公共代理的可用性会变**——今天能用
 * 的明天可能挂。所以留一个用户可填的镜像入口：用户有自建反代、公司
 * 内网镜像，或知道某个当时可用的代理，填进来即可。
 *
 * 这比自己维护一串随时会失效的内置代理可靠得多。
 */
export function DownloadSourceSettings({ kind }: { kind: "engine" | "model" }) {
  const [cfg, setCfg] = useState<DownloadSourceConfig | null>(null);
  const [mirror, setMirror] = useState("");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      const c = await getDownloadSourceConfig(kind);
      setCfg(c);
      setMirror(c.customMirror ?? "");
    } catch (e) {
      setError(String(e));
    }
  }, [kind]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = async () => {
    setError("");
    setSaved(false);
    try {
      await setDownloadSourceConfig(kind, cfg?.preferred ?? null, mirror);
      setSaved(true);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  // 两类都提供选择，但候选不同。
  //
  // 模型页早先不给选，理由是"两者都能拿到同一个文件，不必让用户关心"。
  // 但**海外用户可能访问不了魔搭**——那正是默认首选。留一个手选
  // HuggingFace 的出口，比让用户在那边卡住强。
  const preferredOptions =
    kind === "engine"
      ? [
          { value: "", label: "自动（推荐）" },
          { value: "github-mirror", label: "加速代理优先" },
          { value: "github", label: "官方源优先" },
        ]
      : [
          { value: "", label: "自动（推荐）" },
          { value: "modelscope", label: "魔搭优先" },
          { value: "huggingface", label: "HuggingFace 优先" },
        ];

  // 标题必须能区分"这是哪一类下载的源"。
  //
  // 早先两节都叫「下载源」，标题如下重复出现两次，用户根本分不清
  // 哪个管引擎、哪个管模型——尤其两块的控件还长得一模一样。
  const title = kind === "engine" ? "推理引擎" : "翻译模型";
  const subtitle =
    kind === "engine"
      ? "约 31 MB。从 GitHub 下载，网络不通时可用加速代理或自建镜像。"
      : "数百 MB 到数 GB。优先从魔搭下载，失败自动改用 HuggingFace。";

  return (
    <section className="mb-4">
      <div className="mb-1.5 flex items-baseline gap-2">
        <span className="section-title">{title}</span>
        <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
          {kind === "engine" ? "引擎下载源" : "模型下载源"}
        </span>
      </div>
      <p className="m-0 mb-1.5 text-[10px] leading-relaxed" style={{ color: "var(--text-muted)" }}>
        {subtitle}
      </p>

      <div
        className="rounded-[10px] border border-l-2 p-2.5"
        style={{
          background: "var(--surface-muted)",
          borderColor: "var(--border)",
          // 左侧色条区分两类：引擎用品牌色，模型用中性灰。
          // 两块控件长得一样，纯靠标题区分在滚动时容易看岔。
          borderLeftColor:
            kind === "engine" ? "var(--brand)" : "var(--border-strong)",
        }}
      >
        <div className="mb-2 flex items-center gap-2">
            <span
              className="w-[64px] shrink-0 whitespace-nowrap text-[11px]"
              style={{ color: "var(--text-muted)" }}
            >
              优先使用
            </span>
            <Select
              className="min-w-0 flex-1"
              size="sm"
              value={cfg?.preferred ?? ""}
              options={preferredOptions}
              onChange={(v) =>
                setCfg((prev) => ({
                  kind,
                  preferred: v || null,
                  customMirror: prev?.customMirror ?? null,
                }))
              }
            />
        </div>

        <div className="flex items-center gap-2">
          <span
            className="w-[64px] shrink-0 whitespace-nowrap text-[11px]"
            style={{ color: "var(--text-muted)" }}
          >
            自定义镜像
          </span>
          <input
            className="field min-w-0 flex-1 px-2 py-1 text-[12px]"
            placeholder="https://your-mirror.example.com"
            value={mirror}
            onChange={(e) => {
              setMirror(e.target.value);
              setSaved(false);
            }}
          />
        </div>

        <div className="mt-2 flex items-center gap-2">
          <button
            className="btn h-7 rounded-md border px-2 text-[11px] transition-colors"
            style={btnStyle}
            onClick={save}
          >
            保存
          </button>
          {saved && (
            <span className="text-[10px]" style={{ color: "var(--brand-text)" }}>
              已保存
            </span>
          )}
          {error && (
            <span className="text-[10px]" style={{ color: "var(--danger)" }}>
              {error}
            </span>
          )}
        </div>

        <p className="m-0 mt-1.5 text-[10px] leading-relaxed" style={{ color: "var(--text-muted)" }}>
          镜像需以 http:// 或 https:// 开头，格式为「镜像地址 + 完整原始地址」。
          留空则不使用。
        </p>
      </div>
    </section>
  );
}

/** 与 ModelDrawer / StorageInfo 里的小按钮保持同一套样式。 */
const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};
