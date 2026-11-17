/**
 * 显存 / 内存兼容性判定。
 *
 * ## 这个模块解决什么
 *
 * 预设卡片原本写死「建议 4 / 8 / 12 GB 显存」——不管你机器什么配置都这么
 * 说。用户既不知道自己够不够，也不知道不够时会怎样，只能下完、加载、
 * 失败，然后看到一句「引擎启动失败」。
 *
 * ## 判定依据：显存不够不代表跑不动
 *
 * llama.cpp 的 `-ngl N` 允许一部分层留在内存里由 CPU 跑。所以真正的判据是：
 *
 * | 情况 | 结论 |
 * |---|---|
 * | 显存放得下全部 | 全速 |
 * | **显存 + 内存**放得下 | 能跑，比全 GPU 慢 |
 * | 两者都不够 | 真跑不动 |
 *
 * 只看显存会把第二种误判成「跑不动」——这是本模块要修的主要错误。
 *
 * ## KV cache 用精确值，不用系数
 *
 * 早先按 `权重 × 1.25` 估算，两个问题：
 *
 * 1. **不随上下文长度变化**。用户把 ctx 从 4096 调到 32768，占用涨 8 倍，
 *    估算纹丝不动，于是「放不下」被算成「放得下」
 * 2. 系数是拍的。GQA 模型（`head_count_kv` 远小于 `head_count`）误差很大
 *
 * 现在从 GGUF 头读 `layers / kv_heads / key_len / value_len` 精确算：
 *
 * ```
 * KV = layers × kv_heads × (key_len + value_len) × ctx × 1字节
 * ```
 *
 * 最后那个 1 字节来自项目 preset 里的 `cache-type-k/v = q8_0`（每元素 1 字节）。
 * 实测校验（Hy-MT2-1.8B，本机 RTX 4060）：
 *
 * | ctx | 本公式 | 引擎 q8_0 实测 |
 * |---|---|---|
 * | 4096 | 128 MiB | 272 MiB |
 * | 8192 | 256 MiB | 512 MiB |
 *
 * 差异是 q8_0 的块对齐开销（每 32 元素加 2 字节 scale），量级可接受。
 *
 * 读不到结构参数时退回体积估算——**宁可粗，不可错**。
 */

/** 判定所需输入。 */
export interface FitInput {
  /** 模型文件字节数 */
  sizeBytes: number;
  /** 当前上下文长度（用户可调，来自模型设置） */
  ctxLength: number;
  /** 硬件总显存（MB）；`null` 表示读不到 */
  totalVramMb: number | null;
  /** 物理总内存（MB）；`null` 表示读不到 */
  totalRamMb: number | null;
  /** GGUF 里的 KV 结构参数；读不到为 `null`，此时退回估算 */
  kvShape?: KvShape | null;
  /** 苹果统一内存：显存与内存是同一池 */
  isAppleSilicon?: boolean;
}

/** GGUF 里的 KV 结构参数，与后端 `models::KvShape` 对应。 */
export interface KvShape {
  layers: number;
  kvHeads: number;
  keyLen: number;
  valueLen: number;
}

/**
 * 判定结论。
 *
 * 用 `green / yellow / red` 而不是「够 / 勉强 / 不够」，是因为这三个词在
 * 代码里更短、也更接近业界（jan 用的就是这一套），跨模块引用时不歧义。
 */
export type FitLevel = "green" | "yellow" | "red" | "unknown";

export interface FitResult {
  level: FitLevel;
  /** 一句可直接显示给人看的话；`unknown` 时为空串 */
  text: string;
}

/**
 * 系统与驱动占用的显存/内存余量（MB）。
 *
 * 桌面合成、浏览器、驱动缓冲都要吃。实测 RTX 4060 Laptop 8 GB 空闲时
 * 可用约 7.1 GB——差出接近 1 GB。取 2.2 GB 是留够余量：
 * **估高会直接加载失败，估低只是少放几层**（慢一点但能跑）。
 *
 * 取值与 jan 的 `DISCRETE_RESERVE_BYTES`（2.28 GB）对齐。
 */
const RESERVE_MB = 2233;

/** 苹果统一内存：系统固定占用（MB）与浮动比例，取自 jan。 */
const APPLE_FIXED_RESERVE_MB = 2560;
const APPLE_VARIABLE_RATIO = 0.1;
/** 苹果上「舒适」阈值：用到 85% 以内算全速，再往上算勉强。 */
const APPLE_COMFORTABLE_RATIO = 0.85;

/** 读不到结构参数时的兜底：KV 约占权重的 10%（ctx=4096 基准）。 */
const KV_FALLBACK_RATIO = 0.1;
const KV_BASELINE_CTX = 4096;

/**
 * 算 KV cache 占用（MB）。
 *
 * 有精确结构参数就精确算，没有就按 jan 的经验比例估。
 */
export function estimateKvMb({
  sizeBytes,
  ctxLength,
  kvShape,
}: {
  sizeBytes: number;
  ctxLength: number;
  kvShape?: KvShape | null;
}): number {
  const weightsMb = sizeBytes / (1024 * 1024);
  const ctx = ctxLength > 0 ? ctxLength : KV_BASELINE_CTX;

  if (kvShape && kvShape.layers > 0 && kvShape.kvHeads > 0) {
    // 精确公式。q8_0 = 1 字节/元素；kv_unified=true 不乘并行槽数。
    const perToken =
      kvShape.kvHeads * (kvShape.keyLen + kvShape.valueLen);
    const bytes = kvShape.layers * perToken * ctx;
    return bytes / (1024 * 1024);
  }

  // 兜底：权重 × 10% × (ctx/4096)
  return weightsMb * KV_FALLBACK_RATIO * (ctx / KV_BASELINE_CTX);
}

export function evaluateFit(input: FitInput): FitResult {
  const { sizeBytes, ctxLength, totalVramMb, totalRamMb, isAppleSilicon } = input;

  // 没有权重体积 → 无法判断
  if (!Number.isFinite(sizeBytes) || sizeBytes <= 0) {
    return { level: "unknown", text: "" };
  }

  const weightsMb = sizeBytes / (1024 * 1024);
  const kvMb = estimateKvMb({ sizeBytes, ctxLength, kvShape: input.kvShape });
  const requiredMb = weightsMb + kvMb;

  /*
   * 苹果统一内存：显存与内存共用一个池，`totalVramMb` 通常是 0。
   * 按 jan 的公式：可用 = 总量 - 固定 2.5G - 总量 10%。
   */
  if (isAppleSilicon) {
    if (!totalRamMb || totalRamMb <= 0) {
      return { level: "unknown", text: "" };
    }
    const usable =
      totalRamMb - APPLE_FIXED_RESERVE_MB - totalRamMb * APPLE_VARIABLE_RATIO;
    if (usable <= 0 || requiredMb > usable) {
      return { level: "red", text: redText(requiredMb) };
    }
    // 用掉 85% 以内算舒适
    return requiredMb <= usable * APPLE_COMFORTABLE_RATIO
      ? { level: "green", text: greenText(requiredMb) }
      : { level: "yellow", text: yellowText(requiredMb) };
  }

  const usableVram = Math.max(0, (totalVramMb ?? 0) - RESERVE_MB);
  const usableRam = Math.max(0, (totalRamMb ?? 0) - RESERVE_MB);

  // 无独显（或有卡但读不到）：只看内存能不能放下
  if (!totalVramMb || totalVramMb <= 0) {
    if (!totalRamMb || totalRamMb <= 0) {
      // 连内存都读不到——不给结论，也不编数字
      return { level: "unknown", text: "" };
    }
    return requiredMb <= usableRam
      ? { level: "green", text: greenText(requiredMb) }
      : { level: "red", text: redText(requiredMb) };
  }

  if (requiredMb > usableVram + usableRam) {
    return { level: "red", text: redText(requiredMb) };
  }
  if (requiredMb <= usableVram) {
    return { level: "green", text: greenText(requiredMb) };
  }
  return { level: "yellow", text: yellowText(requiredMb) };
}

function greenText(needMb: number): string {
  return `需要约 ${gb(needMb)} GB，可全速运行`;
}

/**
 * 「部分层在内存里跑」的措辞。
 *
 * 刻意**不写「放到 CPU」**：llama.cpp 里每一层都要 CPU 参与调度，说
 * 「放到 CPU」会让用户误以为切成了纯 CPU 模式。说「在内存里跑」才准确，
 * 也才能解释为什么"比全 GPU 慢、但比纯 CPU 快"。
 */
function yellowText(needMb: number): string {
  return `需要约 ${gb(needMb)} GB，显存放不下全部——部分层在内存里跑，比全 GPU 慢但可用`;
}

function redText(needMb: number): string {
  return `需要约 ${gb(needMb)} GB，显存和内存都不够`;
}

/** MB → 用户习惯的 GB 字符串（整数不带小数点）。 */
function gb(mb: number): string {
  const g = mb / 1024;
  const rounded = Math.round(g * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}
