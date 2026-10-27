import type { ChatMessage, MessageVersion } from "./ipc";

/**
 * 消息状态。对齐 jan `MessageStatus` 的四态：
 * - ready   正常完成
 * - pending 正在生成（占位气泡）
 * - error   请求失败（可能保留了半截内容）
 * - stopped 用户主动中断
 */
export type MessageStatus = "ready" | "pending" | "error" | "stopped";

/** 生成一条消息的唯一 id。消息定位、动作、React key 全部靠它，不再用数组下标。 */
export function newMessageId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `m-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

/** 新建一条消息，id 与状态自动补全。 */
export function makeMessage(
  role: ChatMessage["role"],
  content: string,
  extra: Partial<Omit<ChatMessage, "id" | "role" | "content">> = {},
): ChatMessage {
  return { id: newMessageId(), role, content, status: "ready", ...extra };
}

/**
 * 给没有 id 的旧消息补 id（旧会话文件里没有这个字段）。
 *
 * 补的时候保持顺序稳定：同一次加载得到同样的 id，避免列表抖动；
 * 下一次保存就会把 id 写进会话文件。
 */
export function ensureMessageIds(
  messages: (Omit<ChatMessage, "id"> & { id?: string })[],
): ChatMessage[] {
  return messages.map((m, i) =>
    m.id ? { ...m, id: m.id } : { ...m, id: `legacy-${i}` },
  );
}

/**
 * 合并相邻的同角色消息。
 *
 * 删掉某条回复后，上下文里可能出现连续两条 user（或连续 assistant）。
 * 不少对话模板（Qwen 一类）要求角色严格交替，遇到不交替会直接报错，
 * 所以发请求前把相邻同角色合并成一条，用空行分隔原文。
 * 同时只保留 role/content —— 思维链只用于显示，不参与请求。
 */
export function normalizeHistory(messages: ChatMessage[]): ChatMessage[] {
  const out: ChatMessage[] = [];
  for (const m of messages) {
    const clean: ChatMessage = { id: m.id, role: m.role, content: m.content };
    const last = out[out.length - 1];
    if (last && last.role === clean.role) {
      out[out.length - 1] = {
        ...last,
        content: `${last.content}\n\n${clean.content}`.trim(),
      };
    } else {
      out.push(clean);
    }
  }
  return out;
}

/** 找到某条消息的位置；找不到返回 -1。 */
export function indexOfMessage(messages: ChatMessage[], id: string): number {
  return messages.findIndex((m) => m.id === id);
}

/** 这条消息是否还能作为"重新生成"的锚点（助手回复，且前面有提问）。 */
export function canRegenerateAt(messages: ChatMessage[], id: string): boolean {
  const at = indexOfMessage(messages, id);
  if (at <= 0) return false;
  return messages[at].role === "assistant" && messages[at - 1]?.role === "user";
}

/**
 * 这条消息有几个版本。没有 `versions` 时视为 1 版。
 *
 * 旧会话文件、以及从未重新生成过的消息，都没有这个字段——
 * 所以所有版本逻辑都要能接受"只有 content"的普通消息。
 */
export function versionCount(message: ChatMessage): number {
  return message.versions?.length ?? 1;
}

/** 当前显示第几版（从 0 开始），越界时收敛到 0。 */
export function activeVersion(message: ChatMessage): number {
  const count = versionCount(message);
  const at = message.active ?? 0;
  return at >= 0 && at < count ? at : 0;
}

/**
 * 把一次新生成的结果写成"这条消息的新版本"。
 *
 * 语义是**分支而非覆盖**：旧版本留在 `versions` 里，随时可以切回去对比。
 * 第一次重新生成时，现有 `content` 会被收进 `versions[0]`。
 */
export function appendVersion(
  message: ChatMessage,
  content: string,
  reasoning?: string,
): ChatMessage {
  const existing: MessageVersion[] =
    message.versions ??
    // 首次产生分支：把当前内容作为第 0 版
    [{ content: message.content, ...(message.reasoning ? { reasoning: message.reasoning } : {}) }];

  const versions: MessageVersion[] = [
    ...existing,
    { content, ...(reasoning ? { reasoning } : {}) },
  ];

  return {
    ...message,
    content,
    reasoning,
    status: "ready",
    versions,
    active: versions.length - 1,
  };
}

/**
 * 切换到指定版本。返回新消息对象（不改原对象）。
 *
 * 同时把 `content` / `reasoning` 同步成该版本，这样界面渲染与
 * 发请求时用的都是当前版本的文本。
 */
export function switchVersion(message: ChatMessage, index: number): ChatMessage {
  const versions = message.versions ?? [];
  if (index < 0 || index >= versions.length) return message;

  const version = versions[index];
  return {
    ...message,
    content: version.content,
    reasoning: version.reasoning,
    active: index,
  };
}

/** 取当前版本之前/之后的版本下标；越界返回 null（用于禁用按钮）。 */
export function neighborVersion(
  message: ChatMessage,
  delta: -1 | 1,
): number | null {
  const count = versionCount(message);
  const next = activeVersion(message) + delta;
  return next >= 0 && next < count ? next : null;
}

// ============================================================================
// 上下文压缩
// ============================================================================

/**
 * 估算一段文本占多少 token。
 *
 * 用"字符数 / 2"：中文为主时 1 token ≈ 1.5–2 字，取偏保守的 2。
 * 宁可用力过猛早一点压缩，也不要在请求到达时才发现超了上下文——
 * 后者用户看到的是一次失败，前者只是多一次摘要请求。
 */
export function estimateTokens(text: string): number {
  return Math.ceil(text.length / 2);
}

/** 一条消息（含它的所有版本？不含——版本只有当前的会进上下文）的 token 估算。 */
export function historyTokens(messages: ChatMessage[]): number {
  return messages.reduce((sum, m) => sum + estimateTokens(m.content), 0);
}

/**
 * 判断是否该压缩：历史估算 token 超过 ctx 的某个比例。
 *
 * 0.6 是留了余量的——剩下的要留给输出预算和本轮提问。
 * 触发点比"真的放不下"早很多，是为了在用户察觉之前就悄悄处理掉。
 */
export const COMPACT_TRIGGER_RATIO = 0.6;

export function shouldCompact(
  messages: ChatMessage[],
  ctxSize: number,
): boolean {
  if (ctxSize <= 0) return false;
  return historyTokens(messages) > ctxSize * COMPACT_TRIGGER_RATIO;
}

/**
 * 挑出要压缩的那一段，以及其余要原样保留的消息。
 *
 * 规则与后端裁剪一致：**按轮次切**，绝不在半轮中间下刀。
 * 从最新一轮往回数，保留最近 `keepTurns` 轮；更早的整段交出去做摘要。
 *
 * 返回 `toCompact` 为空表示"还没到值得压缩的程度"（比如总共就两三句话）。
 *
 * 注意 `keepTurns` 不能太大：如果"保留的那几轮"本身就超过触发阈值，
 * 压缩完还超阈值，下一次发送会再压一遍——每轮都白烧一次模型调用。
 * 所以真正该保留多少轮，由 [`planCompaction`] 结合预算算。
 */
export function splitForCompaction(
  messages: ChatMessage[],
  keepTurns = 3,
): { toCompact: ChatMessage[]; toKeep: ChatMessage[] } {
  // 以 user 为界分组（和 normalizeHistory 的分轮方式保持一致）
  const turns: ChatMessage[][] = [];
  for (const m of messages) {
    if (m.role === "user" || turns.length === 0) turns.push([m]);
    else turns[turns.length - 1].push(m);
  }

  if (turns.length <= keepTurns) {
    return { toCompact: [], toKeep: messages };
  }

  const splitAt = turns.length - keepTurns;
  return {
    toCompact: turns.slice(0, splitAt).flat(),
    toKeep: turns.slice(splitAt).flat(),
  };
}

/**
 * 决定这次要不要压缩、压缩到哪。
 *
 * 关键约束：**压缩完必须真的低于触发阈值**，否则下一次发送会立刻再压一遍，
 * 变成"每轮都调一次模型写摘要"的灾难。所以保留轮数从少往多试，
 * 取"能装下且轮数最多"的那一档。
 *
 * 返回 null 表示不需要压缩（还没超阈值，或总共就没几轮）。
 */
export function planCompaction(
  messages: ChatMessage[],
  ctxSize: number,
  maxKeepTurns = 4,
): { toCompact: ChatMessage[]; toKeep: ChatMessage[] } | null {
  if (!shouldCompact(messages, ctxSize)) return null;

  const budget = ctxSize * COMPACT_TRIGGER_RATIO;

  // 保留轮数从少到多试：最少的保底（1 轮）通常都能装下；
  // 多留一轮更连贯，但前提是压缩后确实低于阈值。
  const totalTurns = countTurns(messages);
  const upper = Math.min(maxKeepTurns, Math.max(1, totalTurns - 1));

  for (let keep = 1; keep <= upper; keep++) {
    const { toCompact, toKeep } = splitForCompaction(messages, keep);
    // 没有可压缩的内容（轮数不够）就换更小的保留数；都不行就放弃
    if (toCompact.length === 0) continue;

    // 压缩后的体积 ≈ 摘要占位 + 保留部分。摘要本身很短，按 100 token 估算。
    const after = historyTokens(toKeep) + 100;
    if (after <= budget) return { toCompact, toKeep };
  }

  return null;
}

/** 数一下有几轮（以 user 为界）。 */
function countTurns(messages: ChatMessage[]): number {
  let turns = 0;
  for (const m of messages) {
    if (m.role === "user" || turns === 0) turns++;
  }
  return turns;
}

/**
 * 把摘要替进历史：被压缩的那几轮换成一条 system 消息。
 *
 * 用 `system` 而不是 `assistant`：摘要描述的是"到目前为止发生了什么"，
 * 对模型来说是背景资料，不是它自己的发言。角色也不该和后面真正的
 * 用户提问合并（`normalizeHistory` 只合并同角色，system 不会污染 user）。
 */
export function applyCompaction(
  toKeep: ChatMessage[],
  summary: string,
): ChatMessage[] {
  const note: ChatMessage = {
    id: `summary-${Date.now().toString(36)}`,
    role: "system",
    content: `以下是我们之前翻译对话的要点（已压缩）：\n\n${summary}`,
    status: "ready",
  };
  return [note, ...toKeep];
}
