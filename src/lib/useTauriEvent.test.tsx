import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useTauriEvent } from "./useTauriEvent";

/**
 * 这些用例锁住的是「监听器不能泄漏」。
 *
 * 泄漏的表现是：一个事件被处理两次（StrictMode 双重挂载时尤其明显），
 * 导致思维链重复、用量翻倍。根因是 `listen()` 异步 —— 组件卸载时它
 * 往往还没 resolve，cleanup 拿不到注销函数。
 */

/** 造一个可控的订阅函数：手动决定何时 resolve。 */
function makeSubscribe() {
  const unlisten = vi.fn();
  let resolveIt: ((fn: UnlistenFn) => void) | null = null;

  const subscribe = vi.fn(
    (_cb?: (p: string) => void) =>
      new Promise<UnlistenFn>((resolve) => {
        resolveIt = resolve;
      }),
  );

  return {
    subscribe,
    unlisten,
    /** 让订阅完成，返回注销函数 */
    fire: () => resolveIt?.(unlisten),
  };
}

describe("useTauriEvent", () => {
  it("正常卸载时注销监听器", async () => {
    const { subscribe, unlisten, fire } = makeSubscribe();
    const handler = vi.fn();

    const { unmount } = render(<Probe subscribe={subscribe} handler={handler} />);
    fire();
    await Promise.resolve();

    expect(unlisten).not.toHaveBeenCalled();
    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("卸载早于订阅完成时，兑现后立刻注销（不泄漏）", async () => {
    const { subscribe, unlisten, fire } = makeSubscribe();
    const handler = vi.fn();

    const { unmount } = render(<Probe subscribe={subscribe} handler={handler} />);
    // 关键时序：还没 resolve 就卸载
    unmount();
    fire();
    await Promise.resolve();

    // 这里是 bug 的核心：旧写法会让 unlisten 一次都不被调用
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("回调变化不会重建订阅（只订阅一次）", async () => {
    const { subscribe } = makeSubscribe();
    const first = vi.fn();
    const second = vi.fn();

    const { rerender } = render(
      <Probe subscribe={subscribe} handler={first} />,
    );
    rerender(<Probe subscribe={subscribe} handler={second} />);

    expect(subscribe).toHaveBeenCalledTimes(1);
  });

  it("始终调用最新的回调", async () => {
    const { subscribe, fire } = makeSubscribe();
    const first = vi.fn();
    const second = vi.fn();

    // 订阅函数自己记下推事件用的回调
    let push: (p: string) => void = () => {};
    const wrapped = vi.fn((cb: (p: string) => void) => {
      push = cb;
      return subscribe(cb);
    });

    const { rerender } = render(<Probe subscribe={wrapped} handler={first} />);
    fire();
    await Promise.resolve();

    rerender(<Probe subscribe={wrapped} handler={second} />);
    push("hello");

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith("hello");
  });

  it("订阅失败（非 Tauri 环境）不会抛出", async () => {
    const boom = vi.fn(() => Promise.reject(new Error("no tauri")));
    expect(() =>
      render(<Probe subscribe={boom} handler={vi.fn()} />),
    ).not.toThrow();
  });
});

function Probe({
  subscribe,
  handler,
}: {
  subscribe: (cb: (p: string) => void) => Promise<UnlistenFn>;
  handler: (p: string) => void;
}) {
  useTauriEvent(subscribe, handler);
  return null;
}
