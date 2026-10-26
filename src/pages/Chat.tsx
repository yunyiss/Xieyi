import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ContextMeter } from "../components/ContextMeter";
import { ModelPicker } from "../components/ModelPicker";
import { SessionPicker } from "../components/SessionPicker";
import {
  abort,
  chat,
  deleteAllSessions,
  deleteSession,
  listLocalModels,
  listRouterModels,
  listSessions,
  newRequestId,
  onModelState,
  onToken,
  onUsage,
  saveSession,
  type ChatMessage,
  type LocalModel,
  type RouterModel,
  type Session,
  type ThinkingMode,
  type UsagePayload,
} from "../lib/ipc";

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

/** 聊天页：多轮对话 + 本地会话持久化，不做工具调用。 */
export function Chat({
  refreshSignal = 0,
  draft = "",
  onDraftTaken,
}: Props) {
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  /** 发送 / 生成阶段的错误：浮在输入框上方提示，不占排版 */
  const [error, setError] = useState("");
  /** 环境类问题（运行时没装、引擎起不来、模型列表拉不到）：走空状态区，不当"报错"砸出来 */
  const [setupError, setSetupError] = useState("");
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
      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");

      setSessions(await listSessions());
      setLocalModels(await listLocalModels());
      setSetupError("");
    } catch (e) {
      // 运行时未安装 / 引擎未启动属于"还没准备好"，不是"出错了"：
      // 交给列表空状态说明，别弹成一条红色错误
      setSetupError(String(e));
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

  const removeSession = async (id: string) => {
    try {
      await deleteSession(id);
      // 删掉的正是当前这条，就顺手开一个新的，避免停在一个已经不存在的会话上
      if (id === sessionId) newSession();
      setSessions(await listSessions());
    } catch (e) {
      setError(String(e));
    }
  };

  const removeAllSessions = async () => {
    try {
      await deleteAllSessions();
      newSession();
      setSessions(await listSessions());
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

  const sessionTitle =
    sessions.find((s) => s.id === sessionId)?.title ?? "新对话";
  const currentModel = localModels.find((m) => m.id === model);
  const supportsThinking = currentModel?.supportsThinking ?? false;

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 会话栏：一个选择器就能切换、删除任意一条历史 */}
      <div className="flex flex-nowrap items-center gap-2">
        <SessionPicker
          sessions={sessions}
          currentId={sessionId}
          currentTitle={sessionTitle}
          onPick={switchSession}
          onNew={newSession}
          onDelete={removeSession}
          onDeleteAll={removeAllSessions}
          disabled={busy}
        />

        <span className="ml-auto truncate text-[11px]" style={{ color: "var(--text-muted)" }}>
          对话自动保存在本地
        </span>
      </div>

      {/* 消息列表 */}
      <section ref={listRef} className="card min-h-0 flex-1 overflow-auto p-3.5">
        {messages.length === 0 && (
          <div className="flex h-full items-center justify-center px-4">
            {setupError ? (
              // 环境没就绪时给个"该怎么办"，而不是一行红字
              <div className="max-w-[420px] text-center">
                <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                  {setupError}
                </p>
                <p
                  className="m-0 mt-1.5 text-[12px] leading-relaxed"
                  style={{ color: "var(--text-muted)", opacity: 0.75 }}
                >
                  到「翻译」页安装推理运行时，或在右上角「设置」里确认模型目录已放入
                  .gguf 模型。就绪后这里会自动出现模型。
                </p>
              </div>
            ) : (
              <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                选好模型后直接输入，发送时才会加载模型
              </p>
            )}
          </div>
        )}

        <div className="flex flex-col gap-4">
          {messages.map((m, i) => {
            const isUser = m.role === "user";
            const isLast = i === messages.length - 1;
            return (
              <div
                key={i}
                className={`flex ${isUser ? "justify-end" : "justify-start"}`}
              >
                {isUser ? (
                  // 用户消息用小气泡，助手回复直接铺开，贴近 Codex / DeepSeek 的克制感
                  <div
                    className="max-w-[78%] whitespace-pre-wrap rounded-2xl px-3 py-1.5 text-[13px]"
                    style={{
                      background: "var(--brand-soft)",
                      color: "var(--brand-text)",
                    }}
                  >
                    {m.content}
                  </div>
                ) : (
                  <div
                    className="max-w-[94%] whitespace-pre-wrap px-0.5 text-[13px] leading-relaxed"
                    style={{ color: "var(--text)" }}
                  >
                    {m.content || (busy && isLast ? "…" : "")}
                  </div>
                )}
              </div>
            );
          })}

          {!busy && messages.length > 0 && messages[messages.length - 1].role === "assistant" && (
            <div className="flex justify-start gap-3 text-[11px]">
              <button
                className="rounded px-0 py-0.5 transition-opacity hover:opacity-70"
                style={{ color: "var(--text-muted)" }}
                onClick={regenerate}
              >
                重新生成
              </button>
              <button
                className="rounded px-0 py-0.5 transition-opacity hover:opacity-70"
                style={{ color: "var(--text-muted)" }}
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
              className="h-7 shrink-0 rounded-full px-2.5 text-[11px] transition-colors hover:bg-[var(--surface-muted)] disabled:opacity-50"
              style={{
                background: active ? "var(--brand-soft)" : "transparent",
                border: `1px solid ${active ? "var(--brand)" : "transparent"}`,
                color: active ? "var(--brand-text)" : "var(--text-muted)",
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
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 输入区：整块作为一个输入容器，模型与发送都在底部一行。
          错误提示浮在输入框上方，不再参与排版，避免出错时把输入框顶上去一行 */}
      <div className="relative">
        {error && (
          <div
            className="absolute bottom-full left-0 right-0 z-20 mb-2 flex max-h-[32vh] items-start gap-2 overflow-auto rounded-lg border px-3 py-2 text-[12px] leading-relaxed shadow-lg"
            style={{
              background: "var(--surface)",
              borderColor: "color-mix(in srgb, var(--danger) 38%, var(--border))",
              color: "var(--danger)",
            }}
            role="alert"
          >
            <svg
              className="mt-0.5 h-3.5 w-3.5 shrink-0"
              viewBox="0 0 24 24"
              fill="none"
              aria-hidden
            >
              <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="1.8" />
              <path
                d="M12 7.5v6M12 16.5v.01"
                stroke="currentColor"
                strokeWidth="1.9"
                strokeLinecap="round"
              />
            </svg>
            <span className="min-w-0 flex-1 break-words">{error}</span>
            <button
              className="shrink-0 rounded px-1 text-[14px] leading-none opacity-50 transition-opacity hover:opacity-100"
              onClick={() => setError("")}
              aria-label="关闭提示"
              title="关闭"
            >
              ×
            </button>
          </div>
        )}

        <div className="field flex flex-col px-2 py-2">
          <textarea
            className="min-h-[52px] w-full resize-none bg-transparent px-1 py-1 text-[13px] leading-relaxed outline-none"
            placeholder="输入消息…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />

          <div className="flex items-center gap-1.5 pt-1">
            <span
              className="hidden shrink-0 text-[11px] sm:inline"
              style={{ color: "var(--text-muted)", opacity: 0.7 }}
            >
              Enter 发送 · Shift+Enter 换行
            </span>

            {/* 右侧一组：上下文圆环 → 模型/思考 → 发送/停止 */}
            <div className="ml-auto flex min-w-0 shrink items-center gap-1">
              <ContextMeter
                usage={usage}
                ctxSize={currentModel?.ctxSize ?? 0}
                totals={totals}
              />

              <ModelPicker
                models={models}
                model={model}
                onModel={setModel}
                thinking={thinking}
                onThinking={setThinking}
                thinkingSupported={supportsThinking}
                disabled={busy}
              />

              <button
                className="flex h-9 w-9 items-center justify-center rounded-full text-white transition-colors disabled:opacity-40"
                style={{ background: busy ? "var(--text-muted)" : "var(--brand)" }}
                onClick={() => (busy ? abort(model) : send())}
                disabled={!busy && !model}
                aria-label={busy ? "停止" : "发送"}
                title={busy ? (load ? "停止加载" : "停止生成") : "发送（Enter）"}
              >
                {busy ? (
                  // 停止：一个方块，和发送的箭头区分开
                  <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden>
                    <rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" />
                  </svg>
                ) : (
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
                    <path
                      d="M12 19V5M5.5 11.5 12 5l6.5 6.5"
                      stroke="currentColor"
                      strokeWidth="1.9"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
