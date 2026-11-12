/**
 * 模型列表的状态合并。
 *
 * ## 解决的问题
 *
 * 三个页面的模型下拉都来自**本地扫描**（`listLocalModels`，不启动引擎），
 * 而"是否已加载"这个状态只存在于引擎里。于是每次重扫时都要决定：
 * 新算出来的列表，状态该给什么？
 *
 * 无脑写 `"unloaded"` 会造成一个具体的 bug：
 *
 * ```text
 * 用户在设置抽屉里改了个参数
 *   → refreshSignal +1
 *   → bootstrap 重跑
 *   → 状态被覆盖成"未加载"
 *   → 但引擎里的模型其实还驻留着
 *   → 界面显示错了，要等下一次状态事件才纠正
 * ```
 *
 * 正确做法是**保留已有状态**：只有本次列表里没出现过的模型才给默认值
 * （引擎刚扫到它，确实还没加载）。
 *
 * 抽成纯函数是为了能直接单测——在组件里驱动"引擎事件 + 刷新"的时序
 * 太脆，测试写起来又长又容易误报。
 */

/** 列表项至少要有的形状：一个标识 + 一个状态字符串。 */
export interface ModelStateLike {
  id: string;
  state: string;
}

/**
 * 把新扫描出的 id 列表与旧列表合并，保留旧状态。
 *
 * @param previous 旧列表（可能为空）
 * @param scanned  本次扫描到的模型 id（按顺序）
 * @param fallback 旧列表里没有的模型该用什么状态，默认 `"unloaded"`
 * @param keep     "状态是否可信"的判断。默认全保留。
 *                 页面在引擎确认未运行时可以传 `() => false` 强制重置。
 */
export function mergeModelStates<T extends ModelStateLike>(
  previous: T[],
  scanned: string[],
  fallback = "unloaded",
  keep: (state: string) => boolean = () => true,
): { id: string; state: string }[] {
  const prev = new Map(previous.map((m) => [m.id, m.state]));
  return scanned.map((id) => {
    const old = prev.get(id);
    return {
      id,
      state: old !== undefined && keep(old) ? old : fallback,
    };
  });
}
