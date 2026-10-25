import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  chat,
  getSettings,
  listRouterModels,
  newRequestId,
  onToken,
  setPinned as setModelPinned,
  type ChatMessage,
  type RouterModel,
} from "../lib/ipc";

const controlStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const controlClass =
  "h-8 rounded-lg border px-2 text-[12px] outline-none focus:border-[var(--brand)]";

/** 聊天页：常规对话，复用同一套按需加载与常驻机制。 */
export function Chat() {
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [pinned, setPinned] = useState<string[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const listRef = useRef<HTMLDivElement>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);

  const refresh = useCallback(async () => {
    try {
      const settings = await getSettings();
      setPinned(settings.pinned);

      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, busy]);

  const togglePin = async (next: boolean) => {
    if (!model) return;
    try {
      setPinned(await setModelPinned(model, next));
    } catch (e) {
      setError(String(e));
    }
  };

  const send = async () => {
    const text = input.trim();
    if (!text || busy) return;
    if (!model) {
      setError("请先选择模型");
      return;
    }

    setError("");
    setInput("");
    setBusy(true);

    const history: ChatMessage[] = [...messages, { role: "user", content: text }];
    setMessages([...history, { role: "assistant", content: "" }]);

    const id = newRequestId();
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId !== id) return;
      setMessages((prev) => {
        if (prev.length === 0) return prev;
        const last = prev[prev.length - 1];
        return [
          ...prev.slice(0, -1),
          { ...last, content: last.content + p.token },
        ];
      });
    });

    try {
      const full = await chat({ requestId: id, model, messages: history });
      setMessages((prev) => {
        if (prev.length === 0) return prev;
        return [...prev.slice(0, -1), { role: "assistant", content: full }];
      });
    } catch (e) {
      setError(String(e));
      setMessages((prev) => prev.slice(0, -1));
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setBusy(false);
    }
  };

  const isPinned = pinned.includes(model);

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 顶部：模型与常驻 */}
      <div className="flex flex-wrap items-center gap-2">
        <select
          className={controlClass}
          style={{ ...controlStyle, minWidth: 200 }}
          value={model}
          onChange={(e) => setModel(e.target.value)}
          disabled={models.length === 0}
        >
          {models.length === 0 && <option value="">未发现模型</option>}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.id} {m.state === "loaded" ? "（已加载）" : ""}
            </option>
          ))}
        </select>

        <label className="flex cursor-pointer items-center gap-1.5 text-[12px]">
          <input
            type="checkbox"
            checked={isPinned}
            disabled={!model}
            onChange={(e) => togglePin(e.target.checked)}
            className="accent-[var(--brand)]"
          />
          常驻此模型
        </label>

        <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
          {isPinned ? "常驻中，不会被自动卸载" : "按需模式，空闲后自动释放"}
        </span>

        <button
          className="ml-auto h-8 rounded-lg border px-3 text-[12px]"
          style={controlStyle}
          onClick={() => {
            setMessages([]);
            setError("");
          }}
        >
          清空对话
        </button>
      </div>

      {/* 消息列表 */}
      <section
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto rounded-xl border p-3"
        style={{ background: "var(--surface)", borderColor: "var(--border)" }}
      >
        {messages.length === 0 && (
          <div className="flex h-full items-center justify-center">
            <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
              选好模型后直接输入，发送时才会加载模型
            </p>
          </div>
        )}

        <div className="flex flex-col gap-3">
          {messages.map((m, i) => {
            const isUser = m.role === "user";
            return (
              <div
                key={i}
                className={`flex ${isUser ? "justify-end" : "justify-start"}`}
              >
                <div
                  className="max-w-[80%] whitespace-pre-wrap rounded-xl px-3 py-2 text-[13px]"
                  style={{
                    background: isUser ? "var(--brand-soft)" : "var(--surface-muted)",
                    color: isUser ? "var(--brand-text)" : "var(--text)",
                  }}
                >
                  {m.content ||
                    (busy && i === messages.length - 1 ? "…" : "")}
                </div>
              </div>
            );
          })}
        </div>
      </section>

      {/* 输入区 */}
      <div className="flex items-end gap-2">
        <textarea
          className="min-h-[64px] flex-1 resize-none rounded-xl border p-2.5 text-[13px] outline-none focus:border-[var(--brand)]"
          style={controlStyle}
          placeholder="输入消息，Enter 发送，Shift+Enter 换行"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <button
          className="h-10 rounded-lg px-4 text-[13px] font-medium text-white disabled:opacity-50"
          style={{ background: "var(--brand)" }}
          onClick={send}
          disabled={busy || !model}
        >
          {busy ? "生成中…" : "发送"}
        </button>
      </div>

      {error && (
        <p className="m-0 text-[12px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      )}
    </div>
  );
}
