/**
 * 浅比较两个值是否等价，用于 `setState(prev => equal(prev, next) ? prev : next)`。
 *
 * ## 为什么需要
 *
 * 轮询类代码每次都会拿到**新对象**（IPC 反序列化的必然结果），而 React
 * 的状态更新用引用比较。于是哪怕数据一个字都没变：
 *
 * ```tsx
 * setRuntime(await getRuntimeStatus());   // 新对象 → 必然重渲染
 * ```
 *
 * 每 2.5 秒触发一次整棵树重渲染。本项目的三个页面是**常驻挂载**的
 * （显隐切换而非条件渲染），所以空闲时也在重画，代价比一般页面高。
 *
 * 判等后返回 `prev`，引用不变，React 就会跳过这次更新。
 *
 * ## 为什么只做浅比较
 *
 * 这里比较的都是 IPC 返回的小型扁平对象（`RuntimeStatus` /
 * `ServiceStatus` / `Settings`），字段全是原始值或长度很短的字符串数组。
 * 为它们引入深比较库或写递归比较都不划算。
 *
 * 遇到嵌套对象时保守返回 `false`（视为不等）——宁可多渲染一次，
 * 也不要因为判等漏掉真实变更导致界面不更新。
 */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (a === null || b === null) return false;
  if (typeof a !== "object" || typeof b !== "object") return false;

  // 数组：长度 + 逐项引用（数组元素是字符串/数字时等价于值比较）
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) return false;
    if (a.length !== b.length) return false;
    return a.every((v, i) => Object.is(v, b[i]));
  }

  const ao = a as Record<string, unknown>;
  const bo = b as Record<string, unknown>;
  const ak = Object.keys(ao);
  const bk = Object.keys(bo);
  if (ak.length !== bk.length) return false;

  for (const k of ak) {
    if (!Object.prototype.hasOwnProperty.call(bo, k)) return false;
    const av = ao[k];
    const bv = bo[k];
    // 嵌套对象/数组不展开：保守判为不同，交给 React 多渲染一次
    if (av !== null && bv !== null && typeof av === "object" && typeof bv === "object") {
      if (!sameFlatContainer(av, bv)) return false;
      continue;
    }
    if (!Object.is(av, bv)) return false;
  }
  return true;
}

/**
 * 只比较「一层扁平容器」：数组按项，对象按值。
 *
 * 再往深处遇到对象就返回 false —— 见 `shallowEqual` 末尾的说明。
 */
function sameFlatContainer(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false;
    return a.every((v, i) => {
      if (v !== null && typeof v === "object") return false; // 嵌套太深，保守不等
      return Object.is(v, b[i]);
    });
  }
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  // 一侧是普通对象：本项目的状态里不该出现，保守判不等
  return false;
}
