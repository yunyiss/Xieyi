/**
 * 引导层用到的自适应类名清单。
 *
 * 单独抽成模块而不是从 JSX 里正则提取：测试直接 import 这份清单，
 * 比运行时读文件更简单，也避免依赖 `@types/node`。
 *
 * **加新类时必须同步这里**，否则「类名打错/漏定义」这类问题
 * 不会被测试发现——那种错误不报错，只会让自适应静默失效。
 */
export const ONBOARDING_CLASSES = [
  "onboarding-pad",
  "onboarding-head",
  "onboarding-logo",
  "onboarding-title",
  "onboarding-subtitle",
  "onboarding-card",
  /** 分步容器：恒单列。宽屏只放宽宽度，不改布局。 */
  "onboarding-steps",
] as const;
