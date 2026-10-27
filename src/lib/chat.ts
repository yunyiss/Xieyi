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
