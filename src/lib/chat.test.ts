import { describe, expect, it } from "vitest";
import type { ChatMessage } from "./ipc";
import {
  activeVersion,
  appendVersion,
  canRegenerateAt,
  ensureMessageIds,
  indexOfMessage,
  makeMessage,
  neighborVersion,
  newMessageId,
  normalizeHistory,
  switchVersion,
  versionCount,
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

describe("消息版本分支", () => {
  it("普通消息视为只有 1 版", () => {
    const m = makeMessage("assistant", "初版");
    expect(versionCount(m)).toBe(1);
    expect(activeVersion(m)).toBe(0);
    expect(neighborVersion(m, -1)).toBeNull();
    expect(neighborVersion(m, 1)).toBeNull();
  });

  /** 核心行为：重新生成是"新增一版"，不是覆盖。 */
  it("追加版本时保留旧内容，并自动切到新版", () => {
    const first = makeMessage("assistant", "第一版译文");
    const second = appendVersion(first, "第二版译文");

    expect(versionCount(second)).toBe(2);
    expect(activeVersion(second)).toBe(1);
    expect(second.content).toBe("第二版译文");
    // 旧版本还在，能切回去对比
    expect(second.versions?.[0].content).toBe("第一版译文");
    expect(second.versions?.[1].content).toBe("第二版译文");
  });

  it("切换版本会同步 content 与 reasoning", () => {
    let m = makeMessage("assistant", "甲", { reasoning: "想甲" });
    m = appendVersion(m, "乙", "想乙");
    expect(m.content).toBe("乙");

    const back = switchVersion(m, 0);
    expect(back.content).toBe("甲");
    expect(back.reasoning).toBe("想甲");
    expect(activeVersion(back)).toBe(0);
  });

  it("切换越界时原样返回，不会把消息弄坏", () => {
    const m = appendVersion(makeMessage("assistant", "甲"), "乙");
    expect(switchVersion(m, 5)).toBe(m);
    expect(switchVersion(m, -1)).toBe(m);
  });

  it("相邻版本下标越界时返回 null（用于禁用按钮）", () => {
    const m = appendVersion(makeMessage("assistant", "甲"), "乙");
    expect(neighborVersion(m, 1)).toBeNull(); // 已在最后一版
    expect(neighborVersion(m, -1)).toBe(0);

    const atFirst = switchVersion(m, 0);
    expect(neighborVersion(atFirst, -1)).toBeNull();
    expect(neighborVersion(atFirst, 1)).toBe(1);
  });

  it("多个版本可以连续追加", () => {
    let m = makeMessage("assistant", "v0");
    m = appendVersion(m, "v1");
    m = appendVersion(m, "v2");

    expect(versionCount(m)).toBe(3);
    expect(activeVersion(m)).toBe(2);
    expect(m.versions?.map((v) => v.content)).toEqual(["v0", "v1", "v2"]);
  });
});
