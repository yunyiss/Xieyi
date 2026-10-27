import { describe, expect, it } from "vitest";
import type { ChatMessage } from "./ipc";
import {
  activeVersion,
  appendVersion,
  applyCompaction,
  canRegenerateAt,
  COMPACT_TRIGGER_RATIO,
  ensureMessageIds,
  estimateTokens,
  historyTokens,
  indexOfMessage,
  makeMessage,
  neighborVersion,
  newMessageId,
  normalizeHistory,
  planCompaction,
  shouldCompact,
  splitForCompaction,
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

describe("上下文压缩", () => {
  /** 造一段 n 轮的对话，每轮各 `size` 个字符。 */
  function conversation(turns: number, size = 100): ChatMessage[] {
    const out: ChatMessage[] = [];
    for (let i = 0; i < turns; i++) {
      out.push(makeMessage("user", "问".repeat(size)));
      out.push(makeMessage("assistant", "答".repeat(size)));
    }
    return out;
  }

  it("token 估算按字符数保守折算", () => {
    expect(estimateTokens("")).toBe(0);
    expect(estimateTokens("字".repeat(100))).toBe(50);
    // 中文按 1 字 ≈ 0.5 token 估，宁可高估也不能低估
    expect(estimateTokens("字".repeat(4000))).toBe(2000);
  });

  it("历史 token 累加各条内容", () => {
    const messages = [makeMessage("user", "字".repeat(200)), makeMessage("assistant", "字".repeat(200))];
    expect(historyTokens(messages)).toBe(200);
  });

  it("超过 ctx 的 60% 才触发压缩", () => {
    const messages = conversation(10, 200); // 4000 字符 ≈ 2000 token
    expect(shouldCompact(messages, 8192)).toBe(false); // 阈 4915
    expect(shouldCompact(messages, 2048)).toBe(true); // 阈 1229
    // ctx 未知时不乱触发
    expect(shouldCompact(messages, 0)).toBe(false);
  });

  /** 关键：只能在轮次边界下刀，不能把一轮劈开。 */
  it("按轮次切分，保留最近几轮", () => {
    const messages = conversation(6);
    const { toCompact, toKeep } = splitForCompaction(messages, 2);

    expect(toKeep).toHaveLength(4); // 最近 2 轮 = 4 条
    expect(toCompact).toHaveLength(8); // 前 4 轮

    // 两段的角色序列都必须从 user 开始（否则就是切坏了）
    expect(toCompact[0].role).toBe("user");
    expect(toKeep[0].role).toBe("user");
    expect(toKeep[1].role).toBe("assistant");
  });

  it("轮次不够时不做压缩", () => {
    const messages = conversation(2);
    const { toCompact, toKeep } = splitForCompaction(messages, 3);
    expect(toCompact).toHaveLength(0);
    expect(toKeep).toEqual(messages);
  });

  it("压缩后摘要以 system 身份排在保留消息之前", () => {
    const messages = conversation(6);
    const { toKeep } = splitForCompaction(messages, 2);
    const next = applyCompaction(toKeep, "已定译法：API=接口");

    expect(next).toHaveLength(toKeep.length + 1);
    expect(next[0].role).toBe("system");
    expect(next[0].content).toContain("API=接口");
    expect(next[1]).toEqual(toKeep[0]);
  });

  /**
   * 摘要不能污染请求：system 消息和相邻的 user 不该被合并成一条提问。
   */
  it("摘要与用户提问不会被合并", () => {
    const messages = conversation(4);
    const { toKeep } = splitForCompaction(messages, 1);
    const compacted = applyCompaction(toKeep, "要点");

    const normalized = normalizeHistory(compacted);
    expect(normalized[0].role).toBe("system");
    expect(normalized[1].role).toBe("user");
    // system 的内容不会被并进 user
    expect(normalized[1].content).not.toContain("要点");
  });

  /**
   * 最关键的不变量：**压缩完必须真的低于触发阈值**。
   *
   * 否则下一次发送会立刻再压一遍，每轮都白烧一次模型调用。
   * 这个坑是真踩过的：keepTurns 定死 3 轮时，3 轮本身就可能超过阈值。
   */
  it("压缩后必须低于触发阈值，否则等于没压", () => {
    const ctx = 512;
    const budget = ctx * COMPACT_TRIGGER_RATIO;
    const messages = conversation(8, 200); // 8 轮 × 200 字

    const plan = planCompaction(messages, ctx);
    expect(plan).not.toBeNull();

    const after = historyTokens(plan!.toKeep) + 100; // 摘要占位约 100 token
    expect(after).toBeLessThanOrEqual(budget);
  });

  /**
   * 保留轮数取"能装下的最多轮"——但这里有个取舍：只要 1 轮就够，
   * 就返回 1 轮（少保留更安全，也更省），不会为了连贯硬塞更多。
   * 真正要保证的是：保留的轮数足够小，使压缩后确实低于阈值。
   */
  it("保留的轮数足够小，压缩后一定低于阈值", () => {
    const ctx = 8192;
    const budget = ctx * COMPACT_TRIGGER_RATIO;
    const messages = conversation(20, 1000);

    const plan = planCompaction(messages, ctx);
    expect(plan).not.toBeNull();
    expect(historyTokens(plan!.toKeep) + 100).toBeLessThanOrEqual(budget);
    // 至少保留最后一轮，不能把当前对话也压掉
    expect(plan!.toKeep.filter((m) => m.role === "user").length).toBeGreaterThanOrEqual(1);
  });

  it("没超阈值时不压缩", () => {
    const messages = conversation(2, 50);
    expect(planCompaction(messages, 8192)).toBeNull();
  });

  /** 就算超了阈值，如果轮数不够切出可压缩片段，也不该返回空计划去调模型。 */
  it("轮数不够时返回 null，不发起无意义的摘要请求", () => {
    const messages = conversation(1, 5000); // 一轮但极长
    const plan = planCompaction(messages, 4096);
    // 要么 null，要么 toCompact 非空——绝不能返回 toCompact 为空的计划
    if (plan) expect(plan.toCompact.length).toBeGreaterThan(0);
  });
});
