import { useEffect, useRef } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * 订阅一个 Tauri 事件，并在卸载时可靠地注销。
 *
 * **为什么需要这个 hook**：`listen()` 返回的是 Promise。直接写成
 * ```tsx
 * useEffect(() => {
 *   let un: UnlistenFn | null = null;
 *   onX(cb).then((fn) => (un = fn));
 *   return () => { un?.(); };   // 时序不对
 * }, []);
 * ```
 * 会在「组件已经卸载，但 Promise 还没 resolve」时留下一个泄漏的监听器：
 * cleanup 执行时 `un` 仍是 `null`，于是什么都没注销；等 Promise 兑现时，
 * 监听器已经被挂上去了，却再也没有人负责摘掉它。
 *
 * 平时看不出来，但 `<React.StrictMode>` 在开发模式下会故意「挂载→卸载→再挂载」
 * 一遍来暴露副作用问题，正好撞上这个窗口期：两次挂载各留一个监听器，
 * 于是每个事件被处理两次——思维链显示重复、用量统计翻倍都源于此。
 * 生产构建没有双重挂载，所以这个 bug 只在开发时可见，但它是真的。
 *
 * 处理办法：用 `dead` 记住「effect 是否已经卸载」。如果 Promise 兑现时组件
 * 已经走了，就当场把刚拿到的注销函数执行掉，不留残余。
 *
 * 订阅只建立一次（不随 `handler` 变化重建）：回调存在 ref 里，所以写内联箭头
 * 函数也不会导致反复订阅/退订。
 */
export function useTauriEvent<T>(
  subscribe: (cb: (payload: T) => void) => Promise<UnlistenFn>,
  handler: (payload: T) => void,
) {
  // 回调放进 ref：既保证订阅只建一次，又总是指向最新的闭包
  const latest = useRef(handler);
  latest.current = handler;

  useEffect(() => {
    let un: UnlistenFn | null = null;
    let dead = false;

    subscribe((payload) => latest.current(payload))
      .then((fn) => {
        // 卸载早于 Promise 兑现：立刻注销，别留泄漏的监听器
        if (dead) fn();
        else un = fn;
      })
      .catch(() => {
        // 非 Tauri 环境（纯浏览器预览）里订阅会失败，忽略即可
      });

    return () => {
      dead = true;
      un?.();
    };
  }, [subscribe]);
}
