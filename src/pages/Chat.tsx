import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ContextMeter } from "../components/ContextMeter";
import { ModelPicker } from "../components/ModelPicker";
import { SessionPicker } from "../components/SessionPicker";
import {
  abort,
  chat,
  compactHistory,
  deleteAllSessions,
  deleteSession,
  listLocalModels,
  listSessions,
  newRequestId,
  onModelState,
  onReasoning,
  onToken,
  onUsage,
  saveSession,
  setModelParams,
  type ChatMessage,
  type LocalModel,
  type RouterModel,
  type Session,
  type ThinkingMode,
  type UsagePayload,
} from "../lib/ipc";
import {
  activeVersion,
  appendVersion,
  applyCompaction,
  canRegenerateAt,
  hydrateMessages,
  indexOfMessage,
  makeMessage,
  neighborVersion,
  normalizeHistory,
  planCompaction,
  switchVersion,
  versionCount,
} from "../lib/chat";
import { useTauriEvent } from "../lib/useTauriEvent";
import { mergeModelStates } from "../lib/modelState";

/** 会话 id 与消息 id 用同一套生成方式，但两者互不相干。 */
function newSessionId(): string {
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

/**
 * 对话页的快捷指令：拿到译文后再加工，而不是从头翻一遍。
 *
 * 两条设计约定，都是实测踩出来的：
 *
 * 1. **指令要并进用户消息**，不能作为独立的 system 消息插在对话中间。
 *    Qwen / Hy-MT2 这类模型的对话模板只认开头那条 system，中途出现的
 *    system 会被直接忽略——表现就是"点了按钮和没点一样"。
 * 2. **必须显式要求保留排版**。小模型改文字时很容易顺手把列表压成
 *    一段话，或多加原来没有的 Markdown 标题。
 *
 * 提示词写成"角色 + 任务 + 硬性规则"的结构：小模型对祈使句的服从度
 * 明显好于一句话概括。
 */
const QUICK_ACTIONS = [
  {
    key: "polish",
    label: "润色",
    prompt: `你是中文文字编辑。润色用户给出的文字：让表达自然通顺、去掉翻译腔与冗余，但不改变原意、语气和信息量。

要求：
- 原样保留 Markdown 标记、列表符号、编号、缩进、空行与代码块；行数不得增减
- 原样保留专有名词、数字、单位、链接
- 已经是通顺的文字就不要改，原样输出
- 只输出结果，不要解释、不要加引号

待润色内容：`,
  },
  {
    key: "formal",
    label: "更正式",
    prompt: `你是中文公文编辑。把用户给出的文字改写为正式、书面的语体，用于文档、报告或对外的正式材料。

要求：
- 去掉口语词、语气词、网络用语；用词准确、句式完整
- 保持原意不变，不添加原文没有的信息，也不做扩写
- 原样保留 Markdown 结构、列表、数字、专有名词
- 只输出结果，不要解释

待改写内容：`,
  },
  {
    key: "concise",
    label: "更简洁",
    prompt: `你是中文编辑。压缩用户给出的文字，使其更短，但信息不丢失。

硬性规则（违反即视为失败）：
- 先看输入的结构：有几行就输出几行，每行一个对应
- 输入每行以「- 」或数字开头，输出也必须逐行保留同样的符号
- 严禁把多行合并成一行，严禁新增原来没有的标题或列表符号
- 不丢事实：数字、条件、对象、结论一个都不能少
- 只输出压缩后的正文，不加评论

示例一（段落）：
输入：由于天气的原因，所以我们决定把原定于今天下午举行的会议推迟到明天上午十点再举行。
输出：因天气原因，今日下午的会议推迟至明日上午十点举行。

示例二（列表，逐行对应）：
输入：
- 打开设置菜单，然后把里面的保存按钮点击一下
- 重启这个应用程序
输出：
- 打开设置菜单，点击保存
- 重启应用

待压缩内容：`,
  },
  {
    key: "term",
    label: "统一术语",
    prompt: `你是中文技术文档编辑。用户给出的译文可能对同一个概念使用了不一致的译法。

任务：
1. 找出这些指代同一事物、但措辞不一致的地方
2. 选定其中一个最准确、最常用的说法，全文统一
3. 输出统一后的完整正文

规则：
- 只统一"同一概念的多种叫法"，不要为了统一而改变原意
- 不要改动本来就不相关的近义词
- 原样保留 Markdown 结构、代码、专有名词
- 只输出结果正文，不要列出你改了什么

待统一内容：`,
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
  const [sessionId, setSessionId] = useState(() => newSessionId());
  const [load, setLoad] = useState<LoadState | null>(null);
  const [usage, setUsage] = useState<UsagePayload | null>(null);
  /** 本次会话累计用量 */
  const [totals, setTotals] = useState({ prompt: 0, generated: 0 });
  const [localModels, setLocalModels] = useState<LocalModel[]>([]);
  const [thinking, setThinking] = useState<ThinkingMode>("off");
  /** 正在编辑并准备重发的用户消息（只允许同时编辑一条） */
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  /** 本次请求的思维链正文（只在思考型模型 + 思考开启时出现） */
  const [reasoning, setReasoning] = useState("");
  /** 本次发送要附加的快捷指令（不写进会话历史，只作用于这一轮） */
  const [instruction, setInstruction] = useState<typeof QUICK_ACTIONS[number] | null>(
    null,
  );

  const listRef = useRef<HTMLDivElement>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const messagesRef = useRef<ChatMessage[]>([]);
  /** 本次请求已经流出来的文本，用于失败时判断"要不要保留半截回复"。 */
  const streamedRef = useRef("");
  /** 当前正在进行的请求 id：思维链事件按它过滤 */
  const activeIdRef = useRef("");
  /** 用户点了停止：请求失败时要区分"主动中断"与"真出错" */
  const stoppedRef = useRef(false);
  /** 思维链正文的实时副本：请求结束时把它写进这条回复，之后还能回看 */
  const reasoningRef = useRef("");

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  const refresh = useCallback(async () => {
    try {
      // **只扫目录，不碰引擎。**
      //
      // 早先这里先调 `listRouterModels()` 取模型和状态，再调
      // `listLocalModels()` 取画像——前者内部是 `ensure()`，会把引擎
      // 拉起来。而三个页面都是常驻挂载的，于是启动瞬间就启动了引擎，
      // 那 6 秒里界面是空的。
      //
      // 现在只用本地扫描：`id` 够填下拉，画像本来也在这一份里。
      // 状态一律先给 `unloaded`（引擎没起，不可能有模型驻留），
      // 之后由 `model://state-changed` 事件更新。
      let local: LocalModel[] = [];
      try {
        local = await listLocalModels();
      } catch (e) {
        // 画像只用于过滤与提示，读失败不该把整页拖垮：
        // 拿不到就当作"都能对话"，让用户至少能继续用现有模型。
        console.error("读取模型画像失败，本次不过滤", e);
      }

      // 保留已有状态，别一律重置成 unloaded（见 lib/modelState.ts）
      const list: RouterModel[] = mergeModelStates(
        models,
        local.map((m) => m.id),
      );

      // 翻译专用模型不进对话页：实测它们会把用户的问题本身翻译掉。
      // 判定依据是后端读 GGUF 元数据得到的 chatCapable，不是靠名字猜。
      // 画像是空的时候（读取失败）放行全部，避免"一个模型都没有"。
      const usable =
        local.length > 0
          ? new Set(local.filter((m) => m.chatCapable).map((m) => m.id))
          : null;
      const chatModels = usable ? list.filter((m) => usable.has(m.id)) : list;

      setModels(chatModels);
      setLocalModels(local);
      // 选中的模型必须也是能对话的，否则切到本页会带着一个翻译模型
      setModel((prev) =>
        prev && (!usable || usable.has(prev)) ? prev : chatModels[0]?.id || "",
      );

      setSessions(await listSessions());
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
  useTauriEvent(onUsage, (p) => {
    setUsage(p);
    setTotals((t) => ({
      prompt: t.prompt + p.promptTokens,
      generated: t.generated + p.completionTokens,
    }));
  });

  // 思维链增量：跟着当前请求走，避免别的请求或上一轮的残留混进来
  useTauriEvent(onReasoning, (p) => {
    if (p.requestId !== activeIdRef.current) return;
    reasoningRef.current += p.token;
    setReasoning((prev) => prev + p.token);
  });

  // 切换模型时，思考档位跟随该模型保存的值
  useEffect(() => {
    const m = localModels.find((x) => x.id === model);
    if (m) setThinking(m.thinking);
  }, [model, localModels]);

  /**
   * 思考强度现在只在对话页选（设置抽屉里那份已删）。
   * 选完就写回该模型的参数，下次切回来还是这个档位。
   */
  const changeThinking = async (mode: ThinkingMode) => {
    setThinking(mode);
    if (!model) return;

    const ctx = localModels.find((m) => m.id === model)?.ctxSize ?? 4096;
    try {
      const saved = await setModelParams(model, ctx, mode);
      setLocalModels((prev) =>
        prev.map((m) => (m.id === model ? { ...m, thinking: saved.thinking } : m)),
      );
    } catch (e) {
      // 存不上不影响本次对话（请求里已经带了这个档位），只在控制台留一条
      console.error("保存思考档位失败", e);
    }
  };

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  // 模型状态实时同步
  useTauriEvent(onModelState, (p) => {
    const mapped =
      p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
    setModels((prev) =>
      prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
    );
    setLoad(toLoadState(p));
  });

  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, busy, reasoning]);

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
   * `display` 用于渲染与持久化（已包含一条 pending 的助手占位消息），
   * `outbound` 是真正发给模型的版本，`replyId` 指向那条占位消息。
   *
   * 两者只在带了快捷指令时不同：指令作为系统消息只作用于这一轮，不写进会话历史。
   */
  /**
   * `branchFrom` 指向"正在被重新生成的那条旧回复"：它的内容要作为旧版本保留下来。
   * 普通发送 / 编辑重发 / 重试都传 null——那些是产生新消息，不是给已有回复加版本。
   */
  const sendWith = async (
    display: ChatMessage[],
    outbound: ChatMessage[],
    replyId: string,
    branchFrom: ChatMessage | null = null,
  ) => {
    if (!model) {
      setError("请先选择模型");
      return;
    }

    setError("");
    setBusy(true);
    stoppedRef.current = false;
    streamedRef.current = "";
    // 新请求的思维链重新开始，默认展开——要让用户看到"模型在想，没卡住"
    reasoningRef.current = "";
    setReasoning("");

    const id = newRequestId();
    activeIdRef.current = id;
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId !== id) return;
      streamedRef.current += p.token;
      // 按消息 id 更新，不再假设"最后一条就是回复"——重试/编辑时更稳
      setMessages((prev) =>
        prev.map((m) =>
          m.id === replyId ? { ...m, content: m.content + p.token } : m,
        ),
      );
    });

    try {
      // 历史太长时先压缩：把最早的几轮换成一份摘要，避免后端的硬裁剪
      // 把关键信息（定好的译法、术语）整段丢掉。
      // 压缩失败不该挡住这次对话——退回让后端按轮次裁，至少还能用。
      // 取不到模型参数时按 4096 兜底：宁可压缩得早一点，
      // 也不要因为"本地还没读到 ctx"就把整段历史直接甩给后端硬裁。
      const ctx = localModels.find((m) => m.id === model)?.ctxSize ?? 4096;
      let outboundFinal = outbound;
      // 摘要也要写回会话，否则下一次发送会把同样几轮再压一遍，
      // 每次对话都白烧一次模型调用。
      let compactedDisplay: ChatMessage[] | null = null;

      const plan = planCompaction(outbound, ctx);
      if (plan) {
        try {
          const summary = await compactHistory(model, plan.toCompact);
          outboundFinal = applyCompaction(plan.toKeep, summary);
          // 注意：toKeep 不含那条 pending 占位（占位只存在于 display 里），
          // 所以要把占位单独接回来，否则下面的 replyId 匹配不上、回复会丢。
          const pending = display.find((m) => m.id === replyId);
          compactedDisplay = [
            ...applyCompaction(plan.toKeep, summary),
            ...(pending ? [pending] : []),
          ];
        } catch (e) {
          console.error("压缩历史失败，本次按原样发送", e);
        }
      }

      const full = await chat({
        requestId: id,
        model,
        messages: normalizeHistory(outboundFinal),
        thinking,
      });
      // 思维链随这条回复一起存下来，之后还能点开回看
      const think = reasoningRef.current.trim();
      // 压缩过就把"摘要 + 保留的消息"作为新的显示基准，回复追加在它后面
      const base = compactedDisplay ?? display;
      const finalMessages: ChatMessage[] = base.map(
        (m): ChatMessage => {
          if (m.id !== replyId) return m;

          // 重新生成：把"旧回复"作为上一版留下，新内容作为当前版。
          // 这样用户可以来回切换对比，而不是生成一次就永久丢掉上一次的结果。
          if (branchFrom) {
            return appendVersion(branchFrom, full, think || undefined);
          }

          return {
            ...m,
            content: full,
            status: "ready",
            ...(think ? { reasoning: think } : {}),
          };
        },
      );
      setMessages(finalMessages);
      await persist(finalMessages, model, sessionId);
    } catch (e) {
      // 主动停止不算错误：气泡自己会显示"已停止"，不再弹红条
      const stopped = stoppedRef.current;
      if (!stopped) setError(String(e));

      // 已经流出一部分就留着，标成 error/stopped 让用户知道它不完整；
      // 一个字都没有时收掉空回复气泡，只保留提问（操作条上的"重试"仍然可用）。
      //
      // 例外：正在重新生成的那条（branchFrom 非空）本来就有旧版本，
      // 更不能删——它的 id 被新占位复用了，删掉等于把用户之前的译文也抹了。
      setMessages((prev) => {
        if (!streamedRef.current.trim()) {
          if (branchFrom) {
            // 恢复成发起重新生成之前的样子：旧内容与旧版本都在
            return prev.map((m) =>
              m.id === replyId
                ? { ...branchFrom, status: stopped ? "stopped" : "error" }
                : m,
            );
          }
          return prev.filter((m) => m.id !== replyId);
        }
        return prev.map((m) =>
          m.id === replyId ? { ...m, status: stopped ? "stopped" : "error" } : m,
        );
      });
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      activeIdRef.current = "";
      // 清掉"进行中"的副本；已完成的思维链已经挂在消息上了
      reasoningRef.current = "";
      setReasoning("");
      setBusy(false);
    }
  };

  const send = async () => {
    const text = input.trim();
    if (!text || busy) return;

    setInput("");
    const history: ChatMessage[] = [...messages, makeMessage("user", text)];
    // 先插入一条 pending 的占位回复：流式 token 按 id 往这条上追加
    const reply = makeMessage("assistant", "", { status: "pending" });
    setMessages([...history, reply]);

    // 快捷指令**并进用户消息**，不单独发一条 system：
    // 多数本地模型的对话模板只认开头那条 system，中途插入的会被忽略，
    // 那样点了按钮和没点是一样的。并进来则一定看得见。
    // 这里只改发给模型的 outbound，界面上显示的仍是用户原文。
    const outbound: ChatMessage[] = instruction
      ? [
          ...history.slice(0, -1),
          makeMessage("user", `${instruction.prompt}\n${text}`),
        ]
      : history;

    setInstruction(null);
    await sendWith([...history, reply], outbound, reply.id);
  };

  /**
   * 从某条助手回复重新生成。
   *
   * 两个要点：
   * 1. 保留它之前的全部上下文，这条及之后的消息不再发给模型（"回滚到这里再问"）。
   * 2. **沿用原消息的 id**，新结果作为它的新版本追加——这样同一条消息能
   *    攒下多版译文，来回切换对比。丢掉旧版会让"试试另一种译法"变成不可逆操作。
   */
  const regenerateAt = async (id: string) => {
    if (busy) return;
    const at = indexOfMessage(messagesRef.current, id);
    const before = at > 0 ? messagesRef.current.slice(0, at) : [];
    if (before.length === 0 || before[before.length - 1].role !== "user") return;

    const previous = messagesRef.current[at];
    // 沿用 id（这样版本记在同一处），内容先清空当作占位
    const reply: ChatMessage = {
      id: previous.id,
      role: "assistant",
      content: "",
      status: "pending",
    };

    setMessages([...before, reply]);
    await sendWith([...before, reply], before, reply.id, previous);
  };

  /** 删掉单条消息（用户提问或助手回复都可以），其余上下文保持不变。 */
  const deleteAt = async (id: string) => {    if (busy) return;
    const next = messagesRef.current.filter((m) => m.id !== id);
    setMessages(next);
    // 全删光了就把会话文件一起删掉，别在列表里留一条空壳
    if (next.length === 0) {
      try {
        await deleteSession(sessionId);
        setSessions(await listSessions());
        newSession();
      } catch (e) {
        setError(String(e));
      }
      return;
    }
    await persist(next, model, sessionId);
  };

  /** 在同一条回复的不同版本之间切换（只改显示，不重新请求模型）。 */
  const changeVersion = async (id: string, delta: -1 | 1) => {
    if (busy) return;
    const next = messagesRef.current.map((m) => {
      if (m.id !== id) return m;
      const target = neighborVersion(m, delta);
      return target === null ? m : switchVersion(m, target);
    });
    setMessages(next);
    // 版本也是会话内容的一部分，切完顺手存一次
    await persist(next, model, sessionId);
  };

  /** 进入"编辑并重发"：只允许改用户消息。 */
  const startEdit = (id: string) => {
    const target = messagesRef.current.find((m) => m.id === id);
    if (!target || target.role !== "user") return;
    setEditing({ id, text: target.content });
  };

  /** 重试：最后一条提问没有拿到回复时（失败/中断），原样再发一次。 */
  const resendAt = async (id: string) => {
    if (busy) return;
    const at = indexOfMessage(messagesRef.current, id);
    const target = at >= 0 ? messagesRef.current[at] : undefined;
    if (!target || target.role !== "user") return;

    const history = messagesRef.current.slice(0, at + 1);
    const reply = makeMessage("assistant", "", { status: "pending" });
    setMessages([...history, reply]);
    await sendWith([...history, reply], history, reply.id);
  };

  /**
   * 提交编辑：把这条用户消息换成新内容，它之后的所有消息作废，
   * 然后重新请求回答——等同于"撤回并重发"。
   */
  const submitEdit = async () => {
    const draft = editing;
    if (!draft) return;
    const text = draft.text.trim();
    if (!text) return;

    setEditing(null);
    const at = indexOfMessage(messagesRef.current, draft.id);
    if (at < 0) return;
    const before = messagesRef.current.slice(0, at);
    const history: ChatMessage[] = [...before, makeMessage("user", text)];
    const reply = makeMessage("assistant", "", { status: "pending" });

    setMessages([...history, reply]);
    await sendWith([...history, reply], history, reply.id);
  };

  const switchSession = (id: string) => {
    const target = sessions.find((s) => s.id === id);
    if (!target) return;

    setSessionId(id);
    setMessages(hydrateMessages(target.messages));
    // 会话里记的模型可能是翻译模型（或早已被移除），直接 setModel 会把
    // 一个不能对话的模型塞回输入区。只在它确实可用时才恢复。
    if (target.model && models.some((m) => m.id === target.model)) {
      setModel(target.model);
    }
    setTotals({ prompt: 0, generated: 0 });
    setError("");
    setEditing(null);
  };

  const newSession = () => {
    setSessionId(newSessionId());
    setMessages([]);
    setTotals({ prompt: 0, generated: 0 });
    setError("");
    setEditing(null);
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
            ) : models.length === 0 && localModels.length > 0 ? (
              // 目录里有模型，但没有一个能用来说话——这种情况必须说清楚，
              // 否则用户会以为对话页坏了。
              <div className="max-w-[460px] text-center">
                <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                  当前模型目录里只有翻译专用模型，对话页用不了
                </p>
                <p
                  className="m-0 mt-1.5 text-[12px] leading-relaxed"
                  style={{ color: "var(--text-muted)", opacity: 0.75 }}
                >
                  翻译模型会把你的问题本身翻译掉，无法正常对话。
                  请放一个通用对话模型（如 Qwen、Llama、Gemma）到模型目录，或继续用「翻译」页做译后加工。
                </p>
              </div>
            ) : (
              <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                选好模型后直接输入，发送时才会加载模型
              </p>
            )}
          </div>
        )}

        <div className="flex flex-col gap-3">
          {messages.map((m, i) => {
            const isUser = m.role === "user";
            const isLast = i === messages.length - 1;
            const isEditing = editing?.id === m.id;

            return (
              <div key={m.id} className="group flex flex-col gap-1">
                <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
                  {isEditing ? (
                    // 编辑态：换成可改的输入框，保存即"替换这条并重发"
                    <div className="w-full max-w-[78%]">
                      <textarea
                        autoFocus
                        className="field w-full resize-none px-3 py-2 text-[13px] leading-relaxed"
                        rows={Math.min(8, Math.max(2, editing.text.split("\n").length + 1))}
                        value={editing.text}
                        onChange={(e) =>
                          setEditing((prev) => (prev ? { ...prev, text: e.target.value } : prev))
                        }
                        onKeyDown={(e) => {
                          if (e.key === "Escape") {
                            e.preventDefault();
                            setEditing(null);
                          }
                          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                            e.preventDefault();
                            submitEdit();
                          }
                        }}
                      />
                      <div className="mt-1 flex items-center justify-end gap-2 text-[11px]">
                        <button
                          className="btn rounded px-1 py-0.5 opacity-70 hover:opacity-100"
                          style={{ color: "var(--text-muted)" }}
                          onClick={() => setEditing(null)}
                          title="取消（Esc）"
                        >
                          取消
                        </button>
                        <button
                          className="btn rounded px-1 py-0.5 font-medium disabled:opacity-40"
                          style={{ color: "var(--brand-text)" }}
                          disabled={!editing.text.trim() || busy}
                          onClick={submitEdit}
                          title="保存并重新请求回答（Ctrl+Enter）"
                        >
                          保存并重发
                        </button>
                      </div>
                    </div>
                  ) : isUser ? (
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
                    <div className="flex max-w-[94%] flex-col gap-1.5">
                      {/* 思维链：思考中实时显示；答完之后仍折叠保留，可以点开回看 */}
                      {(() => {
                        const think = isLast && busy ? reasoning : m.reasoning;
                        if (!think) return null;
                        return (
                          <ThinkingPanel
                            text={think}
                            live={Boolean(busy && isLast)}
                            collapse={Boolean(m.content)}
                          />
                        );
                      })()}
                      <div
                        className="whitespace-pre-wrap px-0.5 text-[13px] leading-relaxed"
                        style={{ color: "var(--text)" }}
                      >
                        {m.content || (busy && isLast && !reasoning ? "…" : "")}
                      </div>
                      {/* 失败/中断时明确标出来，半截回复不会被误当成完整答案 */}
                      {(m.status === "error" || m.status === "stopped") && (
                        <span className="px-0.5 text-[11px]" style={{ color: "var(--text-muted)" }}>
                          {m.status === "stopped" ? "已停止，回复可能不完整" : "生成中断，回复可能不完整"}
                        </span>
                      )}
                    </div>
                  )}
                </div>

                {/* 每条消息的动作条：平时隐形，指上来才出现，不干扰阅读 */}
                {!isEditing && !busy && (
                  <div
                    className={`flex gap-3 text-[11px] opacity-0 transition-opacity duration-150 group-hover:opacity-100 focus-within:opacity-100 ${
                      isUser ? "justify-end pr-1" : "justify-start pl-0.5"
                    }`}
                  >
                    {isUser ? (
                      <>
                        <MsgAction onClick={() => startEdit(m.id)} title="改这条并重新请求回答">
                          编辑
                        </MsgAction>
                        {/* 最后一条提问没有回复（失败/中断）时，给一个原样重试 */}
                        {isLast && (
                          <MsgAction onClick={() => resendAt(m.id)} title="原样再发一次">
                            重试
                          </MsgAction>
                        )}
                        <MsgAction onClick={() => deleteAt(m.id)} title="从上下文里移除这条提问">
                          删除
                        </MsgAction>
                      </>
                    ) : (
                      <>
                        {/* 版本切换：只有重新生成过（多版）才出现，平时不占位置 */}
                        {versionCount(m) > 1 && (
                          <span
                            className="flex items-center gap-1 tabular-nums"
                            style={{ color: "var(--text-muted)" }}
                          >
                            <button
                              className="btn rounded px-0.5 disabled:opacity-30"
                              disabled={neighborVersion(m, -1) === null}
                              onClick={() => changeVersion(m.id, -1)}
                              title="上一版"
                              aria-label="上一版"
                            >
                              ‹
                            </button>
                            <span title="这条回复有多个版本，可来回切换对比">
                              {activeVersion(m) + 1}/{versionCount(m)}
                            </span>
                            <button
                              className="btn rounded px-0.5 disabled:opacity-30"
                              disabled={neighborVersion(m, 1) === null}
                              onClick={() => changeVersion(m.id, 1)}
                              title="下一版"
                              aria-label="下一版"
                            >
                              ›
                            </button>
                          </span>
                        )}
                        {m.content && (
                          <MsgAction onClick={() => copyText(m.content)} title="复制这条回复">
                            复制
                          </MsgAction>
                        )}
                        {canRegenerateAt(messages, m.id) &&
                          (m.content || m.status === "error" || m.status === "stopped") && (
                            <MsgAction
                              onClick={() => regenerateAt(m.id)}
                              title="从这条之前重新生成，之后的对话会丢弃"
                            >
                              {m.status === "error" || m.status === "stopped"
                                ? "重试"
                                : "重新生成"}
                            </MsgAction>
                          )}
                        <MsgAction
                          danger
                          onClick={() => deleteAt(m.id)}
                          title="从上下文里移除这条回复"
                        >
                          删除
                        </MsgAction>
                      </>
                    )}
                  </div>
                )}
              </div>
            );
          })}
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
                onThinking={changeThinking}
                thinkingSupported={supportsThinking}
                disabled={busy}
              />

              <button
                className="flex h-9 w-9 items-center justify-center rounded-full transition-colors disabled:opacity-40"
                style={{
                  background: busy ? "var(--text-muted)" : "var(--accent-btn)",
                  color: busy ? "var(--surface)" : "var(--accent-btn-text)",
                }}
                onClick={() => {
                  if (!busy) {
                    send();
                    return;
                  }
                  // 标记为主动停止：失败路径据此关掉红条、把消息标成 stopped
                  stoppedRef.current = true;
                  abort(model).catch((e) => setError(String(e)));
                }}
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

/**
 * 思维链面板：模型在"想"的时候显示，让人知道它没卡住；
 * 答完之后折叠保留（挂在消息上），随时可以点开回看。
 *
 * 展开时最多占 180px 高度、内部滚动——思考可能很长，不能把答案挤出视野；
 * 正文一开始输出就自动收起（除非用户自己点开过）。
 */
function ThinkingPanel({
  text,
  live,
  collapse,
}: {
  text: string;
  /** 正在思考中：标题转成"模型正在思考"并带脉冲点 */
  live: boolean;
  /** 正文已经开始输出：自动收起（用户手动展开过则不收） */
  collapse: boolean;
}) {
  const [open, setOpen] = useState(live);
  const touched = useRef(false);

  useEffect(() => {
    if (live) {
      setOpen(true);
      touched.current = false;
    }
  }, [live]);

  useEffect(() => {
    if (collapse && !touched.current) setOpen(false);
  }, [collapse]);

  return (
    <div
      className="rounded-[10px] border"
      style={{
        borderColor: "var(--border)",
        background: "color-mix(in srgb, var(--surface-muted) 60%, transparent)",
      }}
    >
      <button
        className="row flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[11px]"
        style={{ color: "var(--text-muted)" }}
        onClick={() => {
          touched.current = true;
          setOpen((v) => !v);
        }}
        title={open ? "收起思考过程" : "展开思考过程"}
        aria-expanded={open}
      >
        <span
          className={`h-1.5 w-1.5 shrink-0 rounded-full ${live ? "animate-pulse" : ""}`}
          style={{ background: live ? "var(--brand)" : "var(--border-strong)" }}
        />
        <span className="shrink-0">{live ? "模型正在思考" : "思考过程"}</span>
        <span className="min-w-0 flex-1 truncate opacity-70">
          {open ? "" : text.slice(-40)}
        </span>
        <span className="shrink-0 tabular-nums opacity-70">{text.length} 字</span>
        <svg
          className="h-3 w-3 shrink-0 opacity-60 transition-transform"
          style={{ transform: open ? "rotate(180deg)" : "none" }}
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden
        >
          <path
            d="M6 9l6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>

      {open && (
        <div
          className="max-h-[180px] overflow-auto whitespace-pre-wrap px-2.5 pb-2 text-[12px] leading-relaxed"
          style={{ color: "var(--text-muted)" }}
        >
          {text}
        </div>
      )}
    </div>
  );
}

/** 消息下方的小动作：平时隐形，悬停整条消息时出现。 */
function MsgAction({
  children,
  onClick,
  title,
  danger = false,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  danger?: boolean;
}) {
  return (
    <button
      className="btn rounded px-0.5 py-0.5 opacity-70 hover:opacity-100"
      style={{ color: danger ? "var(--danger)" : "var(--text-muted)" }}
      onClick={onClick}
      title={title}
    >
      {children}
    </button>
  );
}
