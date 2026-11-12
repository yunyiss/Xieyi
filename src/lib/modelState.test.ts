import { describe, expect, it } from "vitest";
import { mergeModelStates } from "./modelState";

/**
 * 状态合并的单元测试。
 *
 * 它存在的理由很具体：三个页面重扫模型列表时，若无脑把状态写成
 * "unloaded"，正在驻留的模型会被显示成"未加载"。用户在设置里改个
 * 参数就会触发重扫，于是"模型忽然没了"——其实引擎里还在。
 */
describe("mergeModelStates", () => {
  it("保留已有模型的状态", () => {
    const prev = [
      { id: "a", state: "loaded" },
      { id: "b", state: "unloaded" },
    ];
    expect(mergeModelStates(prev, ["a", "b"])).toEqual([
      { id: "a", state: "loaded" },
      { id: "b", state: "unloaded" },
    ]);
  });

  it("新出现的模型给默认状态", () => {
    const prev = [{ id: "a", state: "loaded" }];
    expect(mergeModelStates(prev, ["a", "b"])).toEqual([
      { id: "a", state: "loaded" },
      { id: "b", state: "unloaded" },
    ]);
  });

  it("旧列表为空时全部给默认状态", () => {
    expect(mergeModelStates([], ["a"])).toEqual([{ id: "a", state: "unloaded" }]);
  });

  it("本次没扫到的模型会被丢掉（文件被删）", () => {
    const prev = [
      { id: "a", state: "loaded" },
      { id: "gone", state: "loaded" },
    ];
    expect(mergeModelStates(prev, ["a"])).toEqual([{ id: "a", state: "loaded" }]);
  });

  it("可以自定义默认状态", () => {
    expect(mergeModelStates([], ["a"], "missing")).toEqual([
      { id: "a", state: "missing" },
    ]);
  });

  it("keep 返回 false 时强制重置（引擎确认未运行时用）", () => {
    // 场景：用户点了「卸载全部」或引擎已停。此时旧状态不可信，
    // 必须整体重置，否则界面会一直显示"已加载"。
    const prev = [{ id: "a", state: "loaded" }];
    expect(mergeModelStates(prev, ["a"], "unloaded", () => false)).toEqual([
      { id: "a", state: "unloaded" },
    ]);
  });

  it("keep 只对逐个状态生效", () => {
    // 只信任"已加载"，把"加载中"这类中间态重置——
    // 因为重扫时中间态大概率已经过期了。
    const prev = [
      { id: "a", state: "loaded" },
      { id: "b", state: "loading" },
    ];
    const out = mergeModelStates(prev, ["a", "b"], "unloaded", (s) => s === "loaded");
    expect(out).toEqual([
      { id: "a", state: "loaded" },
      { id: "b", state: "unloaded" },
    ]);
  });

  it("扫描顺序决定输出顺序", () => {
    const prev = [
      { id: "a", state: "loaded" },
      { id: "b", state: "loaded" },
    ];
    expect(mergeModelStates(prev, ["b", "a"]).map((m) => m.id)).toEqual(["b", "a"]);
  });

  it("状态为 undefined 的旧条目不会被误当成有效状态", () => {
    // 旧数据可能有残缺（历史版本存下来的），不能让它污染新列表
    const prev = [{ id: "a", state: undefined as unknown as string }];
    const out = mergeModelStates(prev, ["a"]);
    expect(typeof out[0].state).toBe("string");
  });
});
