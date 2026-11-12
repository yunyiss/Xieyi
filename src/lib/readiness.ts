/**
 * 「配置是否完成」的**唯一判定来源**。
 *
 * ## 为什么需要这个模块
 *
 * 这里曾经有三套各自为政的判定：
 *
 * ```text
 * App.tsx        引擎装了 && 模型列表非空        → 决定弹不弹引导层
 * Onboarding     引擎装了 && (选过目录 || 有模型) → 决定按钮写「开始使用」
 * Translate      引擎装了 && 模型列表非空        → 决定顶部报不报「未配置」
 * Batch          引擎装了 && 模型列表非空
 * ```
 *
 * 中间那套是唯一的异类。它起源于一个合理的诉求——扫描模型要读每个
 * GGUF 的元数据，模型多的用户要等几秒，不该让他干等。但它用了
 * **「选过目录」**当替代条件，而那个条件与「能翻译」无关：
 *
 * - 用户点「选择模型目录…」选了一个**空目录** → `dirConfigured = true`
 * - 引导层据此显示「开始使用」，用户以为配好了
 * - 进主界面，翻译页仍然报「还没配置好引擎或模型」
 *
 * 同一个 bug 反复出现过两次，第一次修的是「引擎装了就算完成」，
 * 第二次修的是「选过目录就算完成」——**都是在改判断条件，
 * 而没有把判断收敛到一处**。所以这次改成共享函数，任何页面
 * 想知道「能不能翻」，只能问它。
 *
 * ## ready 的定义
 *
 * `引擎就绪 && 至少有一个模型`。
 *
 * 不做任何「宽松判定」：翻译必须有模型，这是硬事实。让某个页面
 * 比别的页面更乐观，只会制造「这里说好了、那里说没好」的割裂。
 *
 * 对「扫描要等几秒」的诉求，正确的做法是**把等待状态显示出来**
 * （`scanning` → 「正在扫描…」），而不是**放宽完成的条件**。
 * 这两件事被混为一谈，就是上面那个 bug 的来源。
 */

/** 缺什么。用于按原因分派文案，不让调用方各自拼字符串。 */
export type ReadinessMissing = "none" | "engine" | "models" | "both";

export interface Readiness {
  /** 推理引擎已安装且自检通过 */
  engineReady: boolean;
  /** 至少扫到一个可用模型 */
  modelReady: boolean;
  /**
   * **能进入主界面**。只要引擎就绪即可。
   *
   * 模型目录本来就是允许空的——用户可能还没下完、想先把界面摸一遍、
   * 或者准备稍后再放 .gguf。把这些用户挡在门外没有任何好处，
   * 而且会形成死循环：不放模型就进不去，进不去就看不到模型目录在哪。
   *
   * 真正需要模型的地方（翻译/批量）会自己说清楚缺什么。
   */
  canEnter: boolean;
  /**
   * **能翻译**。引擎就绪 **且** 至少有一个模型。
   *
   * 所有页面都必须用它，不要自己再算一遍。翻译必须有模型，这是硬事实，
   * 在这里放宽只会制造「界面说能用、点下去报错」的割裂。
   */
  ready: boolean;
  missing: ReadinessMissing;
  /**
   * 未就绪时的一句话说明（已就绪时为空串）。
   *
   * 写成「缺什么」而不是笼统的「未配置」：用户看到「引擎或模型」
   * 这种含糊说法时，根本不知道该去补哪一项。
   */
  hint: string;
}

export interface ReadinessInput {
  /** `RuntimeStatus.installed` */
  installed: boolean;
  /** 扫描到的模型数量 */
  modelCount: number;
  /**
   * 是否仍在扫描模型。
   *
   * 只影响 `hint` 的措辞（「正在扫描…」而不是「没有模型」），
   * **不影响 `ready`** —— 还没扫完不等于没配好，但也不等于配好了。
   */
  scanning?: boolean;
  /** 用户是否显式设过模型目录，用于把「没模型」说得更具体 */
  dirConfigured?: boolean;
}

export function computeReadiness(input: ReadinessInput): Readiness {
  const engineReady = Boolean(input.installed);
  const modelReady = input.modelCount > 0;
  const canEnter = engineReady;
  const ready = engineReady && modelReady;

  const missing: ReadinessMissing = engineReady
    ? modelReady
      ? "none"
      : "models"
    : modelReady
      ? "engine"
      : "both";

  let hint = "";
  if (!ready) {
    if (!engineReady) {
      hint = "还没配置推理引擎";
    } else if (input.scanning) {
      // 扫描中不能说「没有模型」——那可能是错的，会误导用户去重复操作
      hint = "正在扫描模型…";
    } else if (input.dirConfigured) {
      // 目录设过但还是没有模型：问题在目录内容，不在设置
      hint = "模型目录里还没有 .gguf 文件";
    } else {
      hint = "还没配置模型目录";
    }
  }

  return { engineReady, modelReady, canEnter, ready, missing, hint };
}
