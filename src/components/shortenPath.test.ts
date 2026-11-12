import { describe, expect, it } from "vitest";
import { shortenPath } from "./Onboarding";

/**
 * 路径省略的单元测试。
 *
 * 这个函数存在的理由是**具体的**：早先用 CSS `truncate` 截尾，把
 * `C:\Users\wbai\Desktop\scratch-models` 显示成
 * `C:\Users\wbai\Desktop\sc` —— 用户最需要辨认的目录名反而被切掉。
 */
describe("shortenPath", () => {
  it("够短的路径原样返回", () => {
    expect(shortenPath("D:\\Model\\translate")).toBe("D:\\Model\\translate");
  });

  it("空字符串返回空（由调用方决定显示占位符）", () => {
    expect(shortenPath("")).toBe("");
  });

  it("过长的路径保留开头和结尾，中间用省略号", () => {
    const long = "C:\\Users\\wbai\\Desktop\\scratch-models-and-more-stuff";
    const out = shortenPath(long, 30);
    expect(out).toContain("…");
    // 开头必须保得住（盘符 + 用户目录）
    expect(out.startsWith("C:\\Users")).toBe(true);
    // 结尾必须保得住（目录名，这是用户辨认的关键）
    expect(out.endsWith("stuff")).toBe(true);
  });

  it("结尾保留得比开头多——目录名比盘符更有辨识度", () => {
    const long = "C:\\Users\\wbai\\Desktop\\aaaaaaaabbbbbbbb";
    const out = shortenPath(long, 24);
    const [head, tail] = out.split("…");
    expect(tail.length).toBeGreaterThan(head.length * 0.8);
  });

  it("结果宽度不超过上限（中日韩字符按 2 计）", () => {
    const width = (s: string) =>
      [...s].reduce((w, c) => w + (c.charCodeAt(0) > 0x2e80 ? 2 : 1), 0);
    // 混中文的路径：按字符数算会低估宽度，必须按显示宽度控制
    const mixed = "C:\\用户\\wbai\\文档\\我的翻译模型目录\\很长的子目录名字";
    const out = shortenPath(mixed, 30);
    expect(width(out)).toBeLessThanOrEqual(31); // 上限 + 容一位省略号误差
  });

  it("不会把代理对（emoji/罕见字）切一半", () => {
    // 用展开运算符切分码点，切出来的字符必须仍然合法
    const withAstral = "C:\\Models\\" + "𝄞".repeat(30);
    const out = shortenPath(withAstral, 20);
    expect([...out].every((c) => c.length >= 1)).toBe(true);
    // 不应出现孤立的代理项（decode 后长度 1 的 char 若为高位代理即为坏）
    for (const ch of out) {
      const code = ch.codePointAt(0)!;
      expect(code >= 0xd800 && code <= 0xdfff).toBe(false);
    }
  });
});
