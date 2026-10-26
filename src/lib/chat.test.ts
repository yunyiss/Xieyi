import { describe, expect, it } from "vitest";
import type { ChatMessage } from "./ipc";
import {
  canRegenerateAt,
  ensureMessageIds,
  indexOfMessage,
  makeMessage,
  newMessageId,
  normalizeHistory,
} from "./chat";

describe("newMessageId", () => {
  it("每次生成都不重复", () => {
    const ids = new Set(Array.from({ length: 200 }, () => newMessageId()));
    expect(ids.size).toBe(200);
  });
});

describe("makeMessage", () => {
  it("默认带唯一 id 与 ready 状态", () => {
    const m = makeMessage("user", "你好");
    expect(m.id).toBeTruthy();
    expect(m.role).toBe("user");
    expect(m.content).toBe("你好");
    expect(m.status).toBe("ready");
  });

  it("允许覆盖状态等附加字段", () => {
    const m = makeMessage("assistant", "", { status: "pending" });
    expect(m.status).toBe("pending");
    expect(m.content).toBe("");
  });
});

describe("ensureMessageIds", () => {
  it("旧消息按顺序补稳定 id", () => {
    const legacy = [
      { role: "user" as const, content: "问" },
      { role: "assistant" as const, content: "答" },
    ];
    const fixed = ensureMessageIds(legacy);
    expect(fixed.map((m) => m.id)).toEqual(["legacy-0", "legacy-1"]);

    // 再执行一次要得到同样的 id，避免列表抖动
    expect(ensureMessageIds(fixed).map((m) => m.id)).toEqual([
      "legacy-0",
      "legacy-1",
    ]);
  });

  it("已有 id 原样保留", () => {
    const messages: ChatMessage[] = [makeMessage("user", "问")];
    expect(ensureMessageIds(messages)[0].id).toBe(messages[0].id);
  });
});

describe("normalizeHistory", () => {
  it("合并相邻同角色，丢弃思维链与状态", () => {
    const messages: ChatMessage[] = [
      makeMessage("user", "第一句"),
      makeMessage("user", "第二句"),
      makeMessage("assistant", "回答", { reasoning: "想了很多", status: "stopped" }),
    ];

    const out = normalizeHistory(messages);
    expect(out).toHaveLength(2);
    expect(out[0].content).toBe("第一句\n\n第二句");
    // 思维链只用于显示，绝不进请求上下文
    expect(out[1].reasoning).toBeUndefined();
    expect(out[1].status).toBeUndefined();
  });

  it("角色交替时保持原样", () => {
    const messages: ChatMessage[] = [
      makeMessage("user", "问"),
      makeMessage("assistant", "答"),
      makeMessage("user", "再问"),
    ];
    expect(normalizeHistory(messages).map((m) => m.role)).toEqual([
      "user",
      "assistant",
      "user",
    ]);
  });
});

describe("indexOfMessage / canRegenerateAt", () => {
  const answer = makeMessage("assistant", "答");
  const messages = [makeMessage("user", "问"), answer];

  it("按 id 定位，找不到返回 -1", () => {
    expect(indexOfMessage(messages, answer.id)).toBe(1);
    expect(indexOfMessage(messages, "missing")).toBe(-1);
  });

  it("只有前面紧挨着提问的助手回复才能重新生成", () => {
    expect(canRegenerateAt(messages, answer.id)).toBe(true);
    expect(canRegenerateAt(messages, messages[0].id)).toBe(false);

    // 两条连续助手回复时，后面那条不能作为锚点（否则会把上下文弄乱）
    const merged: ChatMessage[] = [
      makeMessage("user", "问"),
      makeMessage("assistant", "答"),
      makeMessage("assistant", "再答"),
    ];
    expect(canRegenerateAt(merged, merged[2].id)).toBe(false);
  });
});
