import { describe, expect, it } from "vitest";
import { computeReadiness } from "./readiness";

/**
 * 「配置完成」判定的单元测试。
 *
 * 这个模块的存在本身就是一次修 bug 的产物：同一个 bug（引导层说配好了、
 * 页面说没配好）修过两次都没修干净，因为判定散在四个地方各写一套。
 * 收敛成一处的意义只有在**这一处被测试锁住**时才成立——否则下次
 * 有人改一个分支，四个消费者照样一起歪。
 */
describe("computeReadiness", () => {
  it("引擎 + 模型都就绪才算 ready", () => {
    const r = computeReadiness({ installed: true, modelCount: 2 });
    expect(r.ready).toBe(true);
    expect(r.missing).toBe("none");
    expect(r.hint).toBe("");
  });

  it("没有引擎时不算就绪", () => {
    const r = computeReadiness({ installed: false, modelCount: 3 });
    expect(r.ready).toBe(false);
    expect(r.missing).toBe("engine");
    expect(r.hint).toContain("引擎");
  });

  it("引擎就绪但没有模型时不算就绪", () => {
    // 这是那条 bug 的核心：以前这个场景被 `dirConfigured` 放行过。
    const r = computeReadiness({ installed: true, modelCount: 0 });
    expect(r.ready).toBe(false);
    expect(r.missing).toBe("models");
  });

  it("两样都缺时 missing 是 both", () => {
    const r = computeReadiness({ installed: false, modelCount: 0 });
    expect(r.ready).toBe(false);
    expect(r.missing).toBe("both");
  });

  it("dirConfigured 不能把「没模型」变成能翻译", () => {
    // 直接把曾经的错误条件写进断言：这是回归锁。
    // 谁要是哪天想「优化」成选过目录就能翻，这条会先炸。
    const r = computeReadiness({
      installed: true,
      modelCount: 0,
      dirConfigured: true,
    });
    expect(r.ready).toBe(false);
    expect(r.missing).toBe("models");
  });

  it("引擎就绪但没模型时，仍然允许进门", () => {
    // canEnter 与 ready 是两层不同的判据，别混：
    //   canEnter：引擎装好就能进主界面，模型可以稍后再配
    //   ready   ：真要翻译，必须有模型
    //
    // 模型目录本来就允许为空，把「想先进界面看看」的用户挡在门外
    // 会形成死循环：不放模型就进不去，进不去就看不到模型目录在哪。
    const r = computeReadiness({ installed: true, modelCount: 0 });
    expect(r.canEnter).toBe(true);
    expect(r.ready).toBe(false);
  });

  it("没有引擎时也不能进门", () => {
    // 反方向约束：没有引擎连下载和自检都做不了，放进去只会一脸茫然。
    const r = computeReadiness({ installed: false, modelCount: 0 });
    expect(r.canEnter).toBe(false);
    expect(r.ready).toBe(false);
  });

  it("canEnter 从不比 ready 宽松之外的场景放行", () => {
    // 断言两者的关系恒成立：ready 一定是 canEnter 的子集。
    // 否则会出现「说能翻译、但连门都进不去」这种自相矛盾的状态。
    const cases = [
      { installed: false, modelCount: 0 },
      { installed: false, modelCount: 3 },
      { installed: true, modelCount: 0 },
      { installed: true, modelCount: 1 },
    ];
    for (const c of cases) {
      const r = computeReadiness(c);
      if (r.ready) expect(r.canEnter).toBe(true);
    }
  });

  it("dirConfigured 只影响提示文案，不影响判定", () => {
    const withDir = computeReadiness({
      installed: true,
      modelCount: 0,
      dirConfigured: true,
    });
    const withoutDir = computeReadiness({
      installed: true,
      modelCount: 0,
      dirConfigured: false,
    });
    expect(withDir.ready).toBe(withoutDir.ready);
    // 但说法要不同：一个指向「目录里的文件」，一个指向「去设目录」
    expect(withDir.hint).toContain(".gguf");
    expect(withoutDir.hint).toContain("目录");
    expect(withDir.hint).not.toBe(withoutDir.hint);
  });

  it("扫描中如实说正在扫描，不谎称没有模型", () => {
    const r = computeReadiness({
      installed: true,
      modelCount: 0,
      scanning: true,
    });
    expect(r.ready).toBe(false);
    expect(r.hint).toContain("扫描");
    // 不能在这个阶段说「没有 .gguf」——扫描还没结束，那可能是错的
    expect(r.hint).not.toContain("还没发现");
  });

  it("扫描中也不影响 ready 的判定", () => {
    // 扫描完成与否是等待状态，不是配置状态。
    // 已经有模型时哪怕还在扫，也应该算就绪。
    const r = computeReadiness({ installed: true, modelCount: 1, scanning: true });
    expect(r.ready).toBe(true);
    expect(r.hint).toBe("");
  });
});
