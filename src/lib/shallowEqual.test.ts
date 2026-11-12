import { describe, expect, it } from "vitest";
import { shallowEqual } from "./shallowEqual";

/**
 * 浅比较的单元测试。
 *
 * 它存在的原因很具体：轮询每次拿到新对象，直接 setState 会让整棵树
 * 每 2.5 秒重渲染一次。判等后返回 prev 才能跳过。
 *
 * 所以测试重点是**不能漏判真实变更**——宁可多渲染一次，也不能让界面
 * 不更新。这也是"嵌套对象保守判不等"那条设计的原因。
 */
describe("shallowEqual", () => {
  it("同一个引用相等", () => {
    const o = { a: 1 };
    expect(shallowEqual(o, o)).toBe(true);
  });

  it("字段相同但引用不同的对象相等", () => {
    // 这正是轮询场景：IPC 每次都反序列化出新对象
    expect(shallowEqual({ a: 1, b: "x" }, { a: 1, b: "x" })).toBe(true);
  });

  it("任何字段变了就不等", () => {
    expect(shallowEqual({ a: 1, b: "x" }, { a: 2, b: "x" })).toBe(false);
  });

  it("字段数量不同就不等", () => {
    expect(shallowEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);
  });

  it("值为 null / 原始类型时按值比较", () => {
    expect(shallowEqual({ a: null }, { a: null })).toBe(true);
    expect(shallowEqual({ a: null }, { a: 0 })).toBe(false);
    expect(shallowEqual(1, 1)).toBe(true);
    expect(shallowEqual(1, 2)).toBe(false);
    expect(shallowEqual("a", "a")).toBe(true);
  });

  it("字符串数组按内容比较（pinned 列表常见）", () => {
    expect(shallowEqual({ pinned: ["a", "b"] }, { pinned: ["a", "b"] })).toBe(true);
    expect(shallowEqual({ pinned: ["a"] }, { pinned: ["a", "b"] })).toBe(false);
    expect(shallowEqual({ pinned: ["a", "b"] }, { pinned: ["b", "a"] })).toBe(false);
  });

  it("对象的键顺序不影响判等", () => {
    expect(shallowEqual({ a: 1, b: 2 }, { b: 2, a: 1 })).toBe(true);
  });

  it("一侧缺 key 时不等（不能把缺失当成 undefined 相等）", () => {
    expect(shallowEqual({ a: 1 }, { a: undefined } as never)).toBe(false);
  });

  it("嵌套对象保守判不等，避免漏掉深层变更", () => {
    // 本项目状态里没有嵌套对象；真出现了就多渲染一次，
    // 总比"判等相等 → 界面不更新"强。
    expect(shallowEqual({ a: { x: 1 } }, { a: { x: 1 } })).toBe(false);
  });

  it("null / 非对象参与时不会抛异常", () => {
    expect(shallowEqual(null, null)).toBe(true);
    expect(shallowEqual(null, {})).toBe(false);
    expect(shallowEqual({}, null)).toBe(false);
    expect(shallowEqual(undefined, undefined)).toBe(true);
  });

  it("数组与对象互不相等", () => {
    expect(shallowEqual([1, 2], { 0: 1, 1: 2 })).toBe(false);
  });
});
