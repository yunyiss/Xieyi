/**
 * 显存兼容性判定。
 *
 * 三档预设卡片上原本写死「4 GB / 8 GB / 12 GB 显存」——**不管你机器是
 * 什么配置，它都这么说**。用户既不知道自己的卡够不够，也不知道不够时
 * 会发生什么，只能下完、加载、失败，然后看到一句「引擎启动失败」。
 *
 * 这里把「够不够」算出来，给三种结论：
 *
 * | 结论 | 含义 |
 * |---|---|
 * | `fits` | 显存放得下，全速运行 |
 * | `partial` | 放不下全部，部分层会落到 CPU，明显变慢 |
 * | `cpu` | 显存远不够，基本只能纯 CPU |
 *
 * ## 为什么不直接信 `vramHintMb`
 *
 * 那是**建议值**（含 KV cache 与桌面余量），不是硬门槛。判断"跑不跑得动"
 * 要用权重体积本身。而且给用户看的应该是"你这台机器会怎样"，不是
 * "官方建议多少"。
 */

/** 判定所需的最小输入。 */
export interface FitInput {
  /** 模型文件字节数 */
  sizeBytes: number;
  /** 硬件总显存（MB）；`null` 表示读不到 */
  totalVramMb: number | null;
}

export type FitLevel = "fits" | "partial" | "cpu" | "unknown";

export interface FitResult {
  level: FitLevel;
  /** 一句可直接显示给人看的话 */
  text: string;
  /**
   * 预计有多少比例的层能上 GPU（0–1）。
   *
   * `unknown` 时为 `null`——不知道就不编数字。
   */
  gpuShare: number | null;
}

/**
 * 系统与驱动占用的显存余量（MB）。
 *
 * 桌面合成、浏览器、驱动自己的缓冲都要吃显存。实测 RTX 4060 Laptop 8 GB
 * 上，空闲时可用约 7.1 GB —— 差出接近 1 GB。给 1.5 GB 是留够余量：
 * 估高会直接加载失败，估低只是少放几层（慢一点但能跑）。
 */
const RESERVED_MB = 1536;

/** 判定「放得下」时要求的余量比例（相对权重体积）。 */
const KV_OVERHEAD = 1.25;

export function evaluateFit({ sizeBytes, totalVramMb }: FitInput): FitResult {
  // 读不到显存（无独显、虚拟机、macOS）→ 不给结论，也不编数字
  if (!totalVramMb || totalVramMb <= 0) {
    return { level: "unknown", text: "", gpuShare: null };
  }

  const weightsMb = sizeBytes / (1024 * 1024);
  const usableMb = Math.max(0, totalVramMb - RESERVED_MB);
  // 权重 + KV cache + 计算缓冲
  const needMb = weightsMb * KV_OVERHEAD;
  const share = Math.min(1, usableMb / needMb);

  if (share >= 1) {
    return {
      level: "fits",
      text: `你的显卡 ${gb(totalVramMb)} GB，可全速运行`,
      gpuShare: 1,
    };
  }
  // 过半还能勉强接受；再低就基本是 CPU 在扛，说"部分"会误导
  if (share >= 0.5) {
    return {
      level: "partial",
      text: `你的显卡 ${gb(totalVramMb)} GB，部分层会放到 CPU，速度较慢`,
      gpuShare: share,
    };
  }
  return {
    level: "cpu",
    text: `你的显卡 ${gb(totalVramMb)} GB，放不下，基本靠 CPU（明显慢）`,
    gpuShare: share,
  };
}

/** MB → 用户习惯的 GB 字符串。 */
function gb(mb: number): string {
  const g = mb / 1024;
  /*
   * **接近整数就说整数。**
   *
   * 显卡的标称容量和系统读数差一点：8 GB 的卡注册表给 8188 MB，
   * 除以 1024 是 7.996 → 直接一位小数会显示「8.0 GB」。
   * 用户知道自己是 8 GB 卡，看到 8.0 会觉得这软件在瞎猜。
   *
   * 所以先四舍五入到一位小数，再去掉无意义的 `.0`。
   * 真正的非整数（如 12.5 GB 的卡）仍保留小数。
   */
  const rounded = Math.round(g * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}
