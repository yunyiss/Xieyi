import { describe, expect, it } from "vitest";
import { evaluateFit } from "./modelFit";

/**
 * 显存兼容性判定。
 *
 * 这些断言描述的是**产品判断**，不是数学推导：
 *
 * - 卡片原本写死「建议 8 GB 显存」，不管用户机器什么配置
 * - 用户既不知道够不够，也不知道不够时会怎样
 *
 * 所以这里每一条都对应一个用户会问的问题。
 */

const GB = 1024;
/** 常见档位（与实际预设一致） */
const SIZE_1_8B = 1_908_528_192; // 1.8 GB
const SIZE_7B_Q4 = 4_624_648_896; // 4.3 GB
const SIZE_7B_Q8 = 7_981_928_896; // 7.4 GB

describe("evaluateFit", () => {
  /**
   * 读不到显存时**不给结论**。
   *
   * 无独显、虚拟机、macOS 都会走到这里。编一个数字出来比不说话更糟：
   * 用户会照着错的建议做决定。
   *
   * 变异验证：把 `unknown` 分支改成返回 `fits`，本测试失败。
   */
  it("读不到显存时返回 unknown，不给结论", () => {
    for (const v of [null, 0, undefined as unknown as null]) {
      const r = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb: v });
      expect(r.level).toBe("unknown");
      expect(r.text).toBe("");
      expect(r.gpuShare).toBeNull();
    }
  });

  /**
   * 8 GB 卡跑 7B Q4（4.3 GB）应当判「放得下」。
   *
   * 这是最典型的一档：预设里「均衡」就写着建议 8 GB。
   * 扣掉 1.5 GB 系统占用后仍有 6.5 GB，装 4.3×1.25=5.4 GB 有余。
   */
  it("8GB 卡跑 7B Q4 判为 fits", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb: 8188 });
    expect(r.level).toBe("fits");
    expect(r.gpuShare).toBe(1);
    expect(r.text).toMatch(/8 GB/);
    expect(r.text).toMatch(/全速/);
  });

  /**
   * 只有 4 GB 可用时跑 7B Q4 → 部分层落 CPU。
   *
   * 4 GB - 1.5 GB = 2.5 GB 可用，需要 5.4 GB，share≈0.46…
   * 这个值落在 0.5 以下，所以判 `cpu` 而不是 `partial`——
   * 过半和不过半是两件事，说「部分层」会让用户以为还能用。
   */
  it("4GB 卡跑 7B Q4 判为 cpu（不是 partial）", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb: 4 * GB });
    expect(r.level).toBe("cpu");
    expect(r.gpuShare).toBeLessThan(0.5);
    expect(r.text).toMatch(/CPU/);
  });

  /**
   * 6 GB 卡跑 7B Q4 → 确实只差一点，判 partial。
   *
   * 6 - 1.5 = 4.5 GB 可用，需要 5.4 GB，share≈0.83 → 介于 0.5 与 1 之间。
   */
  it("6GB 卡跑 7B Q4 判为 partial", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb: 6 * GB });
    expect(r.level).toBe("partial");
    expect(r.gpuShare).toBeGreaterThan(0.5);
    expect(r.gpuShare).toBeLessThan(1);
    expect(r.text).toMatch(/CPU/);
  });

  /**
   * 同一块卡，档位越大结论越差 —— 顺序必须单调。
   *
   * 这条防的是阈值写反：如果某个分支把 `>=` 写成 `<=`，单看一条断言
   * 可能仍然通过（比如恰好落在边界外），但三种档位的相对关系会乱。
   */
  it("同一块卡，档位越大结论不会更好", () => {
    const totalVramMb = 8 * GB;
    const order = { unknown: 0, fits: 2, partial: 1, cpu: 0 } as const;
    const small = evaluateFit({ sizeBytes: SIZE_1_8B, totalVramMb });
    const mid = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb });
    const big = evaluateFit({ sizeBytes: SIZE_7B_Q8, totalVramMb });

    expect(order[small.level]).toBeGreaterThanOrEqual(order[mid.level]);
    expect(order[mid.level]).toBeGreaterThanOrEqual(order[big.level]);

    // 具体到这个组合：1.8B 与 7B Q4 都放得下，7B Q8（7.4 GB）放不下
    expect(small.level).toBe("fits");
    expect(mid.level).toBe("fits");
    expect(big.level).not.toBe("fits");
  });

  /**
   * 12 GB 卡跑 7B Q8 应当放得下。
   *
   * 12 - 1.5 = 10.5 GB 可用，需要 7.4×1.25≈9.3 GB。
   */
  it("12GB 卡跑 7B Q8 判为 fits", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q8, totalVramMb: 12 * GB });
    expect(r.level).toBe("fits");
  });

  /**
   * 文案里必须带上用户的实际显存，不能只说「够」或「不够」。
   *
   * 「你的显卡 6 GB，部分层会放到 CPU」比「显存不足」有用得多——
   * 后者用户无法判断是差一点还是差很多。
   */
  it("文案里给出用户的实际显存", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q4, totalVramMb: 6 * GB });
    expect(r.text).toContain("6 GB");
  });

  /** 8.5 GB 这种非整数要显示成一位小数，不能四舍五入成 8 或 9。 */
  it("非整数显存显示一位小数", () => {
    const r = evaluateFit({
      sizeBytes: SIZE_1_8B,
      totalVramMb: Math.round(8.5 * GB),
    });
    expect(r.text).toContain("8.5 GB");
  });

  /** 纯 CPU 环境（显存极小）不能崩，也不能报负数比例。 */
  it("显存极小时不崩溃、比例不为负", () => {
    const r = evaluateFit({ sizeBytes: SIZE_7B_Q8, totalVramMb: 128 });
    expect(r.level).toBe("cpu");
    expect(r.gpuShare).toBeGreaterThanOrEqual(0);
  });
});
