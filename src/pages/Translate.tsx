import { useState } from "react";

const LANGUAGES = [
  { code: "auto", label: "自动检测" },
  { code: "zh", label: "中文" },
  { code: "en", label: "英语" },
  { code: "ja", label: "日语" },
  { code: "ko", label: "韩语" },
  { code: "fr", label: "法语" },
  { code: "de", label: "德语" },
  { code: "es", label: "西班牙语" },
  { code: "ru", label: "俄语" },
];

const selectClass =
  "h-9 rounded-lg border px-2.5 text-[13px] outline-none transition-colors focus:border-[var(--brand)]";

/** 翻译页：语言对 + 原文 + 译文，引擎按需加载。 */
export function Translate() {
  const [source, setSource] = useState("auto");
  const [target, setTarget] = useState("zh");
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 语言对 */}
      <div className="flex items-center gap-2">
        <select
          className={selectClass}
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text)",
          }}
          value={source}
          onChange={(e) => setSource(e.target.value)}
        >
          {LANGUAGES.map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>

        <span style={{ color: "var(--text-muted)" }}>→</span>

        <select
          className={selectClass}
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text)",
          }}
          value={target}
          onChange={(e) => setTarget(e.target.value)}
        >
          {LANGUAGES.filter((l) => l.code !== "auto").map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>

        <div className="ml-auto flex items-center gap-2">
          <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
            按需加载 · 停手后自动释放
          </span>
        </div>
      </div>

      {/* 输入 / 输出 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
        <section
          className="flex min-h-0 flex-col overflow-hidden rounded-xl border"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <header
            className="flex items-center justify-between border-b px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
          >
            <span>原文</span>
            <span>{input.length} 字</span>
          </header>
          <textarea
            className="min-h-0 flex-1 resize-none bg-transparent p-3 outline-none"
            placeholder="粘贴或输入要翻译的内容…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </section>

        <section
          className="flex min-h-0 flex-col overflow-hidden rounded-xl border"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <header
            className="flex items-center justify-between border-b px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
          >
            <span>译文</span>
            <span>Hy-MT2 · 1.8B</span>
          </header>
          <div className="min-h-0 flex-1 overflow-auto p-3">
            {output ? (
              <p className="m-0 whitespace-pre-wrap">{output}</p>
            ) : (
              <p className="m-0" style={{ color: "var(--text-muted)" }}>
                译文会显示在这里
              </p>
            )}
          </div>
        </section>
      </div>

      {/* 操作区 */}
      <div className="flex items-center gap-3">
        <button
          className="h-10 rounded-lg px-5 text-[13px] font-medium text-white transition-colors"
          style={{ background: "var(--brand)" }}
          onMouseEnter={(e) =>
            (e.currentTarget.style.background = "var(--brand-hover)")
          }
          onMouseLeave={(e) =>
            (e.currentTarget.style.background = "var(--brand)")
          }
          title="推理链路将在下一步接入"
        >
          运行翻译
        </button>
        <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
          下一步接入：加载模型 → 流式翻译 → 空闲卸载
        </span>
        <button
          className="ml-auto h-10 rounded-lg border px-4 text-[13px]"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text)",
          }}
          onClick={() => {
            setInput("");
            setOutput("");
          }}
        >
          清空
        </button>
      </div>
    </div>
  );
}
