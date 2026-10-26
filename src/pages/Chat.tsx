import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ContextMeter } from "../components/ContextMeter";
import { ModelStateBadge } from "../components/ModelStateBadge";
import {
  abort,
  chat,
  deleteSession,
  getSettings,
  listLocalModels,
  listRouterModels,
  listSessions,
  newRequestId,
  onModelState,
  onToken,
  onUsage,
  saveSession,
  setPinned as setModelPinned,
  type ChatMessage,
  type LocalModel,
  type RouterModel,
  type Session,
  type ThinkingMode,
  type UsagePayload,
} from "../lib/ipc";

const controlStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const controlClass =
  "h-8 rounded-lg border px-2 text-[12px] outline-none focus:border-[var(--brand)]";

function newId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

interface Props {
  /** 设置抽屉里改动后递增，用于触发重新扫描。 */
  refreshSignal?: number;
  /** 从翻译页带过来的待处理文本，会填进输入框。 */
  draft?: string;
  /** 取走草稿后通知父级清空，避免重复填入。 */
  onDraftTaken?: () => void;
}

/** 对话页的快捷指令：翻译之后用它来调整语意，而不是从头翻。 */
const QUICK_ACTIONS = [
  {
    key: "polish",
    label: "润色",
    prompt: "请润色下面这段内容，让表达更自然流畅，只输出润色后的结果：",
  },
  {
    key: "formal",
    label: "更正式",
    prompt: "请把下面内容改写成正式、书面的表达，只输出结果：",
  },
  {
    key: "concise",
    label: "更简洁",
    prompt: "请把下面内容压缩得更简洁，保留原意，只输出结果：",
  },
  {
    key: "compare",
    label: "对照原文",
    prompt:
      "下面是我要处理的译文。请先指出它与原意可能的偏差、漏译或生硬之处，再给出改进版本：",
  },
  {
    key: "term",
    label: "统一术语",
    prompt: "请检查下面内容的术语是否前后一致，给出统一术语后的版本：",
  },
];

/** 思考档位，与 llama.cpp Web UI 一致。needCtx 为可用此档所需的最小上下文。 */
const THINKING_OPTIONS: { value: ThinkingMode; label: string; needCtx: number }[] = [
  { value: "default", label: "默认", needCtx: 0 },
  { value: "off", label: "不思考", needCtx: 0 },
  { value: "low", label: "思考·低", needCtx: 1024 },
  { value: "medium", label: "思考·中", needCtx: 4096 },
  { value: "high", label: "思考·高", needCtx: 16384 },
  { value: "max", label: "思考·不限", needCtx: 0 },
];

/** 聊天页：多轮对话 + 本地会话持久化，不做工具调用。 */
export function Chat({
  refreshSignal = 0,
  draft = "",
  onDraftTaken,
}: Props) {
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [pinned, setPinned] = useState<string[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionId, setSessionId] = useState(() => newId());
  const [load, setLoad] = useState<LoadState | null>(null);
  const [usage, setUsage] = useState<UsagePayload | null>(null);
  /** 本次会话累计用量 */
  const [totals, setTotals] = useState({ prompt: 0, generated: 0 });
  const [localModels, setLocalModels] = useState<LocalModel[]>([]);
  const [thinking, setThinking] = useState<ThinkingMode>("off");
  /** 本次发送要附加的快捷指令（不写进会话历史，只作用于这一轮） */
  const [instruction, setInstruction] = useState<typeof QUICK_ACTIONS[number] | null>(
    null,
  );

  const listRef = useRef<HTMLDivElement>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const messagesRef = useRef<ChatMessage[]>([]);
  /** 本次请求已经流出来的文本，用于失败时判断"要不要保留半截回复"。 */
  const streamedRef = useRef("");

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  const refresh = useCallback(async () => {
    try {
      const settings = await getSettings();
      setPinned(settings.pinned);

      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");

      setSessions(await listSessions());
      setLocalModels(await listLocalModels());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, refreshSignal]);

  // 从翻译页带过来的文本：填进输入框后立即通知父级清空
  useEffect(() => {
    if (!draft) return;
    setInput((prev) => (prev.trim() ? `${draft}\n\n${prev}` : draft));
    onDraftTaken?.();
  }, [draft, onDraftTaken]);

  // 每次补全的真实 token 用量，用来显示"上下文用了多少"
  useEffect(() => {
    let un: UnlistenFn | null = null;
    onUsage((p) => {
      setUsage(p);
      setTotals((t) => ({
        prompt: t.prompt + p.promptTokens,
        generated: t.generated + p.completionTokens,
      }));
    }).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

  // 切换模型时，思考档位跟随该模型在设置里保存的值
  useEffect(() => {
    const m = localModels.find((x) => x.id === model);
    if (m) setThinking(m.thinking);
  }, [model, localModels]);

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  // 模型状态实时同步
  useEffect(() => {
    let un: UnlistenFn | null = null;
    onModelState((p) => {
      const mapped =
        p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
      setModels((prev) =>
        prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
      );
      setLoad(toLoadState(p));
    }).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, busy]);

  const persist = async (msgs: ChatMessage[], currentModel: string, id: string) => {
    if (msgs.length === 0) return;
    const firstUser = msgs.find((m) => m.role === "user");
    const title = firstUser
      ? firstUser.content.slice(0, 24).replace(/\s+/g, " ")
      : "新对话";

    try {
      await saveSession({
        id,
        title,
        model: currentModel,
        updatedAt: 0,
        messages: msgs,
      });
      setSessions(await listSessions());
    } catch (e) {
      console.error("保存会话失败", e);
    }
  };

  /**
   * `display` 用于渲染与持久化，`outbound` 是真正发给模型的版本。
   * 两者只在带了快捷指令时不同：指令作为系统消息只作用于这一轮，不写进会话历史。
   */
  const sendWith = async (display: ChatMessage[], outbound: ChatMessage[]) => {
    if (!model) {
      setError("请先选择模型");
      return;
    }

    setError("");
    setBusy(true);
    streamedRef.current = "";

    const id = newRequestId();
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId !== id) return;
      streamedRef.current += p.token;
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
      const full = await chat({ requestId: id, model, messages: outbound, thinking });
      const finalMessages: ChatMessage[] = [
        ...display,
        { role: "assistant", content: full },
      ];
      setMessages(finalMessages);
      await persist(finalMessages, model, sessionId);
    } catch (e) {
      setError(String(e));
      // 已经流出一部分就留着，别因为报错把用户看到的内容一起抹掉；
      // 一个字都没有时才收掉空回复气泡，只保留提问。
      if (!streamedRef.current.trim()) {
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last?.role === "assistant" && !last.content) return prev.slice(0, -1);
          return prev;
        });
      }
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setBusy(false);
    }
  };

  const send = async () => {
    const text = input.trim();
    if (!text || busy) return;

    setInput("");
    const history: ChatMessage[] = [
      ...messages,
      { role: "user", content: text },
    ];
    setMessages([...history, { role: "assistant", content: "" }]);

    // 快捷指令作为系统消息插入，只影响这一轮，不污染会话历史
    const outbound: ChatMessage[] = instruction
      ? [
          ...messages,
          { role: "system", content: instruction.prompt },
          { role: "user", content: text },
        ]
      : history;

    setInstruction(null);
    await sendWith(history, outbound);
  };

  /** 丢掉最后一条助手回复，用同样的用户输入重新生成。 */
  const regenerate = async () => {
    if (busy) return;
    const current = messagesRef.current;
    if (current.length < 2) return;

    const withoutLast = current.slice(0, -1);
    if (withoutLast[withoutLast.length - 1].role !== "user") return;

    setMessages([...withoutLast, { role: "assistant", content: "" }]);
    await sendWith(withoutLast, withoutLast);
  };

  const switchSession = (id: string) => {
    const target = sessions.find((s) => s.id === id);
    if (!target) return;

    setSessionId(id);
    setMessages(target.messages);
    if (target.model) setModel(target.model);
    setTotals({ prompt: 0, generated: 0 });
    setError("");
  };

  const newSession = () => {
    setSessionId(newId());
    setMessages([]);
    setTotals({ prompt: 0, generated: 0 });
    setError("");
  };

  const removeSession = async () => {
    try {
      await deleteSession(sessionId);
      setSessions(await listSessions());
      newSession();
    } catch (e) {
      setError(String(e));
    }
  };

  const copyText = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      setError("复制失败");
    }
  };

  const isPinned = pinned.includes(model);
  const sessionTitle =
    sessions.find((s) => s.id === sessionId)?.title ?? "新对话";
  const currentModel = localModels.find((m) => m.id === model);
  const supportsThinking = currentModel?.supportsThinking ?? false;

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 顶部：模型与常驻（单行） */}
      <div className="flex flex-nowrap items-center gap-2">
        <select
          className={`${controlClass} min-w-0 flex-1`}
          style={controlStyle}
          value={model}
          onChange={(e) => setModel(e.target.value)}
          disabled={models.length === 0}
          title={model || "未发现模型"}
        >
          {models.length === 0 && <option value="">未发现模型</option>}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.id}
            </option>
          ))}
        </select>

        <ModelStateBadge
          state={models.find((m) => m.id === model)?.state ?? "unloaded"}
        />

        <label className="flex shrink-0 cursor-pointer items-center gap-1.5 text-[12px]">
          <input
            type="checkbox"
            checked={isPinned}
            disabled={!model}
            onChange={async (e) => {
              try {
                setPinned(await setModelPinned(model, e.target.checked));
              } catch (err) {
                setError(String(err));
              }
            }}
            className="accent-[var(--brand)]"
          />
          常驻
        </label>

        <span
          className="hidden shrink-0 text-[12px] xl:inline"
          style={{ color: "var(--text-muted)" }}
        >
          {isPinned ? "常驻中，不会被自动卸载" : "按需模式，空闲后自动释放"}
        </span>
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 上下文用量：数据来自模型返回的真实 usage */}
      {usage && <ContextMeter usage={usage} totals={totals} />}

      {/* 会话栏 */}
      <div className="flex flex-nowrap items-center gap-2">
        <select
          className={`${controlClass} min-w-0 max-w-[240px] flex-1`}
          style={controlStyle}
          value={sessionId}
          onChange={(e) => switchSession(e.target.value)}
        >
          <option value={sessionId}>{sessionTitle}</option>
          {sessions
            .filter((s) => s.id !== sessionId)
            .map((s) => (
              <option key={s.id} value={s.id}>
                {s.title}
              </option>
            ))}
        </select>

        <button
          className="h-8 shrink-0 rounded-lg border px-2.5 text-[12px]"
          style={controlStyle}
          onClick={newSession}
        >
          新对话
        </button>
        <button
          className="h-8 shrink-0 rounded-lg border px-2.5 text-[12px]"
          style={controlStyle}
          onClick={removeSession}
          disabled={!sessions.some((s) => s.id === sessionId)}
        >
          删除
        </button>

        <span className="ml-auto truncate text-[11px]" style={{ color: "var(--text-muted)" }}>
          会话自动保存
        </span>
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
            const isLast = i === messages.length - 1;
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
                    (busy && isLast ? "…" : "")}
                </div>
              </div>
            );
          })}

          {!busy && messages.length > 0 && messages[messages.length - 1].role === "assistant" && (
            <div className="flex justify-start gap-2">
              <button
                className="h-7 rounded-md border px-2 text-[11px]"
                style={controlStyle}
                onClick={regenerate}
              >
                重新生成
              </button>
              <button
                className="h-7 rounded-md border px-2 text-[11px]"
                style={controlStyle}
                onClick={() => copyText(messages[messages.length - 1].content)}
              >
                复制
              </button>
            </div>
          )}
        </div>
      </section>

      {/* 快捷指令：翻译之后再调整语意，比重新翻一遍省事 */}
      <div className="flex flex-nowrap items-center gap-1.5 overflow-x-auto">
        <span className="shrink-0 text-[11px]" style={{ color: "var(--text-muted)" }}>
          处理
        </span>
        {QUICK_ACTIONS.map((action) => {
          const active = instruction?.key === action.key;
          return (
            <button
              key={action.key}
              className="h-7 shrink-0 rounded-full border px-2.5 text-[11px] transition-colors disabled:opacity-50"
              style={{
                background: active ? "var(--brand-soft)" : "var(--surface)",
                borderColor: active ? "var(--brand)" : "var(--border)",
                color: active ? "var(--brand-text)" : "var(--text)",
              }}
              title={action.prompt}
              disabled={busy}
              onClick={() => setInstruction(active ? null : action)}
            >
              {action.label}
            </button>
          );
        })}
        {instruction && (
          <span className="shrink-0 text-[11px]" style={{ color: "var(--brand-text)" }}>
            下一句生效
          </span>
        )}

        {/* 思考档位：与 llama.cpp Web UI 六档一致，模型不支持时禁用 */}
        <label
          className="ml-auto flex shrink-0 items-center gap-1"
          title={
            supportsThinking
              ? "思考越久通常越慢；档位越高，正文可用的输出配额也会同步放大"
              : "该模型的对话模板不支持思考"
          }
        >
          <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
            💡
          </span>
          <select
            className="h-7 rounded-md border px-1 text-[11px] outline-none focus:border-[var(--brand)] disabled:opacity-40"
            style={controlStyle}
            value={thinking}
            disabled={!supportsThinking || busy}
            onChange={(e) => setThinking(e.target.value as ThinkingMode)}
          >
            {THINKING_OPTIONS.map((o) => {
              const tooSmall =
                o.needCtx > 0 && (currentModel?.ctxSize ?? 0) < o.needCtx;
              return (
                <option key={o.value} value={o.value}>
                  {o.label}
                  {tooSmall ? "（上下文不足）" : ""}
                </option>
              );
            })}
          </select>
        </label>
      </div>

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
          {load ? "加载中…" : busy ? "生成中…" : "发送"}
        </button>

        {busy && (
          <button
            className="h-10 rounded-lg border px-4 text-[13px]"
            style={controlStyle}
            onClick={() => abort(model)}
          >
            {load ? "停止加载" : "停止"}
          </button>
        )}
      </div>

      {error && (
        <p className="m-0 text-[12px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      )}
    </div>
  );
}
