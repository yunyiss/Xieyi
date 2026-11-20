import { describe, expect, it } from "vitest";
import { estimateKvMb, evaluateFit, type FitInput } from "./modelFit";

/**
 * 显存 / 内存兼容性判定。
 *
 * 这些断言描述的是**产品判断**，每一条都对应一个用户会问的问题。
 *
 * ## 两个关键事实（改这块前必须知道）
 *
 * 1. **显存不够不代表跑不动**——llama.cpp 的 `-ngl N` 允许一部分层留在
 *    内存里由 CPU 跑。所以判据是「显存 + 内存」，不是只看显存。
 * 2. **KV 占用随 ctx 线性增长**——用户能把 ctx 从 2048 调到 65536，
 *    占用差 32 倍。写死一个系数必然算错。
 */

/** 常见档位（与实际预设一致） */
const SIZE_1_8B = 1_908_528_192; // 1.8 GB
const SIZE_7B_Q4 = 4_624_648_896; // 4.3 GB
const SIZE_7B_Q8 = 7_981_928_896; // 7.4 GB

/** 实测得到的 KV 结构参数（与 presets 里硬编码的一致） */
const KV_1_8B = { layers: 32, kvHeads: 4, keyLen: 128, valueLen: 128 };
const KV_7B = { layers: 32, kvHeads: 8, keyLen: 128, valueLen: 128 };

function input(over: Partial<FitInput> = {}): FitInput {
  return {
    sizeBytes: SIZE_7B_Q4,
    ctxLength: 4096,
    totalVramMb: 8188,
    totalRamMb: 16104,
    kvShape: null,
    isAppleSilicon: false,
    ...over,
  };
}

describe("estimateKvMb", () => {
  /**
   * 精确公式必须与引擎实测对得上。
   *
   * 判据来自 `llama-server -v` 的
   * `llama_kv_cache: Vulkan0 KV buffer size = N MiB`：
   * Hy-MT2-1.8B 在 ctx=8192、q8_0 KV 下实测 272 MiB（未量化时 512 MiB）。
   *
   * 本公式算的是「q8_0 理想值」，块对齐开销让实测略大，量级对齐即可。
   */
  it("有结构参数时按精确公式算", () => {
    const kv4096 = estimateKvMb({
      sizeBytes: SIZE_1_8B,
      ctxLength: 4096,
      kvShape: KV_1_8B,
    });
    const kv8192 = estimateKvMb({
      sizeBytes: SIZE_1_8B,
      ctxLength: 8192,
      kvShape: KV_1_8B,
    });
    // 32层 × 4头 × (128+128) × 4096 = 128 MiB
    expect(kv4096).toBeCloseTo(128, 0);
    expect(kv8192).toBeCloseTo(256, 0);
  });

  /**
   * 混合注意力：只算真正有全上下文 KV 的层，局部层按滑动窗口算。
   *
   * 变异验证：退回 `layers × kvHeads`（相当于按全部 48 层全上下文算），
   * 结果会从 72 MiB 变成 384 MiB，本测试失败。
   */
  it("混合注意力只算真正有 KV 的层", () => {
    const hybrid = {
      layers: 48,
      kvHeads: 4,
      kvHeadLayers: 8 * 4,
      localKvHeadLayers: 40 * 4,
      slidingWindow: 1024,
      keyLen: 128,
      valueLen: 128,
    };
    const kv = estimateKvMb({
      sizeBytes: SIZE_7B_Q4,
      ctxLength: 4096,
      kvShape: hybrid,
    });
    // (8层×4头×4096 + 40层×4头×1024) × 256 字节 = 72 MiB
    expect(kv).toBeCloseTo(72, 0);
  });

  /**
   * **ctx 翻倍，KV 必须翻倍**。
   *
   * 这是旧实现最严重的错误：它用固定系数，ctx 从 4096 调到 32768 时
   * 少算 8 倍，把「放不下」算成「放得下」。
   *
   * 变异验证：把公式里的 `ctx` 换成常数 4096，本测试失败。
   */
  it("ctx 翻倍时 KV 也翻倍（旧实现不会）", () => {
    const a = estimateKvMb({
      sizeBytes: SIZE_7B_Q4,
      ctxLength: 4096,
      kvShape: KV_7B,
    });
    const b = estimateKvMb({
      sizeBytes: SIZE_7B_Q4,
      ctxLength: 8192,
      kvShape: KV_7B,
    });
    expect(b).toBeCloseTo(a * 2, 1);
  });

  /** 没有结构参数时退回体积估算，且同样随 ctx 变。 */
  it("无结构参数时退回估算，仍随 ctx 变化", () => {
    const a = estimateKvMb({ sizeBytes: SIZE_7B_Q4, ctxLength: 4096 });
    const b = estimateKvMb({ sizeBytes: SIZE_7B_Q4, ctxLength: 8192 });
    expect(a).toBeGreaterThan(0);
    expect(b).toBeCloseTo(a * 2, 1);
  });

  /** 7B 的 kv_heads 是 8（1.8B 是 4），KV 应该正好是两倍。 */
  it("kv_heads 越多 KV 越大（GQA 结构差异）", () => {
    const small = estimateKvMb({
      sizeBytes: SIZE_1_8B,
      ctxLength: 4096,
      kvShape: KV_1_8B,
    });
    const big = estimateKvMb({
      sizeBytes: SIZE_7B_Q4,
      ctxLength: 4096,
      kvShape: KV_7B,
    });
    expect(big).toBeCloseTo(small * 2, 0);
  });
});

describe("evaluateFit", () => {
  /**
   * 读不到任何硬件信息时**不给结论**。
   *
   * 无独显、虚拟机、权限受限都可能走到这里。编一个数字比不说话更糟。
   *
   * 变异验证：把 `unknown` 分支改成返回 `green`，本测试失败。
   */
  it("硬件都读不到时返回 unknown", () => {
    for (const over of [
      { totalVramMb: null, totalRamMb: null },
      { totalVramMb: 0, totalRamMb: 0 },
    ]) {
      const r = evaluateFit(input(over));
      expect(r.level).toBe("unknown");
      expect(r.text).toBe("");
    }
  });

  /** 显存充足 → green。8GB 卡跑 7B-Q4（4.4GB 权重）绰绰有余。 */
  it("显存放得下时判 green", () => {
    const r = evaluateFit(input({ kvShape: KV_7B }));
    expect(r.level).toBe("green");
    expect(r.text).toMatch(/全速/);
  });

  /**
   * **核心用例**：4GB 显存 + 16GB 内存跑 7B-Q4。
   *
   * 旧实现只看显存 → 判「跑不动」。
   * 新实现看到显存+内存够 → 判 yellow「部分层在内存里跑」。
   *
   * 这才是本模块存在的意义：不把能跑的情况说成跑不动。
   *
   * 变异验证：把 `requiredMb > usableVram + usableRam` 改成只看
   * `usableVram`，本测试失败（会变成 red）。
   */
  it("显存不够但内存够时判 yellow，不是 red", () => {
    const r = evaluateFit(
      input({ totalVramMb: 4096, totalRamMb: 16104, kvShape: KV_7B }),
    );
    expect(r.level).toBe("yellow");
    expect(r.text).toMatch(/内存/);
  });

  /**
   * 措辞不能写「放到 CPU」。
   *
   * llama.cpp 里每一层都要 CPU 参与调度，说「放到 CPU」会让用户误以为
   * 切成了纯 CPU 模式。说「在内存里跑」才准确。
   */
  it("yellow 文案不说「放到 CPU」", () => {
    const r = evaluateFit(
      input({ totalVramMb: 4096, totalRamMb: 16104, kvShape: KV_7B }),
    );
    expect(r.text).toContain("内存");
    expect(r.text).not.toContain("放到 CPU");
  });

  /** 显存和内存都不够 → red。 */
  it("显存和内存都不够时判 red", () => {
    const r = evaluateFit(
      input({
        sizeBytes: SIZE_7B_Q8,
        totalVramMb: 2048,
        totalRamMb: 4096,
        kvShape: KV_7B,
      }),
    );
    expect(r.level).toBe("red");
    expect(r.text).toMatch(/不够/);
  });

  /**
   * **ctx 变大时结论必须变差**。
   *
   * 这是旧实现完全做不到的：它算出的需求与 ctx 无关。
   *
   * 用 8GB 卡跑 7B-Q8（7.4GB 权重）：
   * - ctx=2048 → KV 小，显存放得下 → green
   * - ctx=32768 → KV 大 16 倍，放不下 → 变差
   */
  it("同一个模型，ctx 越大结论越差", () => {
    const order = { unknown: 0, red: 1, yellow: 2, green: 3 } as const;
    // 用 12B（48 层、key/value 各 256）配 8GB 内存：ctx 从 2K 到 32K
    // 需求从 7.0GB 涨到 12.8GB，正好跨过「显存+内存」的上限。
    const big = { layers: 48, kvHeads: 8, keyLen: 256, valueLen: 256 };
    const small = evaluateFit(
      input({
        sizeBytes: 6_935_348_544,
        ctxLength: 2048,
        totalRamMb: 8192,
        kvShape: big,
      }),
    );
    const large = evaluateFit(
      input({
        sizeBytes: 6_935_348_544,
        ctxLength: 32768,
        totalRamMb: 8192,
        kvShape: big,
      }),
    );
    expect(small.level).toBe("yellow");
    expect(large.level).toBe("red");
    expect(order[small.level]).toBeGreaterThan(order[large.level]);
  });

  /**
   * 无独显（totalVramMb 为 0）但有内存 → 用内存判定。
   *
   * 旧实现直接返回 unknown，等于放弃所有核显/纯 CPU 用户。
   */
  it("无独显时用内存判定，不再返回 unknown", () => {
    const enough = evaluateFit(
      input({ totalVramMb: 0, totalRamMb: 16104, kvShape: KV_7B }),
    );
    expect(enough.level).toBe("green");

    const notEnough = evaluateFit(
      input({
        sizeBytes: SIZE_7B_Q8,
        totalVramMb: 0,
        totalRamMb: 4096,
        kvShape: KV_7B,
      }),
    );
    expect(notEnough.level).toBe("red");
  });

  /**
   * 苹果统一内存：显存读不到（通常是 0），靠内存判。
   *
   * 分支与常规不同：系统固定占 2.5GB + 总量 10%，用到 85% 以内算舒适。
   */
  it("苹果统一内存走单独分支", () => {
    const r = evaluateFit(
      input({
        isAppleSilicon: true,
        totalVramMb: null,
        totalRamMb: 16384,
        kvShape: KV_7B,
      }),
    );
    // 16GB 统一内存：可用 = 16384 - 2560 - 1638 = 12186 MB，7B-Q4 约 4.8GB
    expect(r.level).toBe("green");
  });

  /** 苹果上内存不够时判 red。 */
  it("苹果统一内存不够时判 red", () => {
    const r = evaluateFit(
      input({
        sizeBytes: SIZE_7B_Q8,
        isAppleSilicon: true,
        totalVramMb: null,
        totalRamMb: 8192,
        kvShape: KV_7B,
      }),
    );
    expect(r.level).toBe("red");
  });

  /**
   * 文案里要给出需求体积，让用户能自己判断。
   *
   * 只说「够」或「不够」用户没法核实。
   */
  it("文案给出需求体积", () => {
    const r = evaluateFit(input({ kvShape: KV_7B }));
    expect(r.text).toMatch(/\d+(\.\d)? GB/);
  });

  /** 纯 CPU 环境（显存 0、内存很小）不能崩。 */
  it("极端输入不崩溃", () => {
    const r = evaluateFit({
      sizeBytes: SIZE_7B_Q8,
      ctxLength: 0,
      totalVramMb: 0,
      totalRamMb: 128,
      kvShape: null,
    });
    expect(r.level).toBe("red");
  });

  /** sizeBytes 非法时返回 unknown，不抛异常。 */
  it("非法 sizeBytes 返回 unknown", () => {
    expect(evaluateFit(input({ sizeBytes: 0 })).level).toBe("unknown");
    expect(evaluateFit(input({ sizeBytes: -1 })).level).toBe("unknown");
    expect(evaluateFit(input({ sizeBytes: NaN })).level).toBe("unknown");
  });
});
