import { useState } from "react";
import type { QualityReport } from "../lib/ipc";

/**
 * 译文质量提示条。
 *
 * 设计取向：**提醒，不是报警**。
 *
 * 早先这里是一条左侧竖线的灰字，用户反馈"太生硬、不够精致"——因为它
 * 看起来像错误，但用户手上其实是有译文的，只是质量存疑。
 *
 * 现在的处理：
 * - 嵌在译文卡片内部（标题下、正文上），不覆盖、不阻塞
 * - 浅琥珀底色 + 低对比边框：比灰色醒目，比红色温和
 * - 只展示**实际命中**的原因，两个都中就合并成一句
 * - 可以关掉：关掉只隐藏提示，译文和复制/编辑照旧
 *
 * 文案按原因码分派，不解析后端的中文说明（那是给人读的，
 * 措辞随时可能改）。
 */
interface Props {
  reports: QualityReport[];
  /** 目标语言的中文名，用于「译文可能不是中文」这类措辞 */
  targetLabel: string;
  /** 重新翻译（用当前模型再跑一次） */
  onRetry: () => void;
  /** 把译文带到对话页继续调整 */
  onSendToChat?: () => void;
}

/** 把命中的原因码整理成一句人话。 */
export function describeIssue(codes: string[], targetLabel: string): string {
  const has = (c: string) => codes.includes(c);

  // 两条最值得单独说明的：根本没翻、翻错语言
  if (has("untranslated")) {
    return "检测到译文与原文相同，可能未发生实际翻译。";
  }
  if (has("wrong-script")) {
    return `检测到译文可能不是${targetLabel}，建议重新翻译或进一步调整。`;
  }
  if (has("wrong-variant")) {
    return `检测到译文用了${targetLabel}的另一种字形，可重新翻译或手动调整。`;
  }
  if (has("instruction-leak")) {
    return "检测到译文里混入了说明文字，可能影响了译文完整性。";
  }
  if (has("too-short")) {
    return "检测到译文明显短于原文，可能被截断。";
  }
  if (has("too-long")) {
    return "检测到译文明显长于原文，可能夹杂了重复或解释。";
  }
  if (has("structure")) {
    return "检测到原文的结构标记（列表、代码块等）在译文里丢失了。";
  }
  if (has("glossary")) {
    return "检测到术语表中的译法没有生效。";
  }
  if (has("empty")) {
    return "检测到译文为空。";
  }
  return "本次译文未通过质量检查。";
}

export function QualityHint({
  reports,
  targetLabel,
  onRetry,
  onSendToChat,
}: Props) {
  const [dismissed, setDismissed] = useState(false);

  // 只关心"重试后仍未修好"的那些——修好了就不必打扰用户
  const unfixed = reports.filter((r) => !r.fixed);
  if (dismissed || unfixed.length === 0) return null;

  // 合并所有原因码，一句话说清
  const codes = Array.from(new Set(unfixed.flatMap((r) => r.codes)));
  const text = describeIssue(codes, targetLabel);

  return (
    <div
      className="mb-2.5 rounded-lg border px-3 py-2.5"
      style={{
        background: "color-mix(in srgb, #f59e0b 7%, var(--surface))",
        borderColor: "color-mix(in srgb, #f59e0b 28%, var(--border))",
      }}
    >
      <div className="flex items-start gap-2">
        {/* 提示图标：一个感叹号圆环，比 emoji 更克制 */}
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden
          className="mt-0.5 shrink-0"
          style={{ color: "#b45309" }}
        >
          <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="1.8" />
          <path
            d="M12 7.8v5.2"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
          <circle cx="12" cy="16.4" r="1" fill="currentColor" />
        </svg>

        <div className="min-w-0 flex-1">
          <div
            className="text-[12px] font-medium"
            style={{ color: "color-mix(in srgb, #b45309 85%, var(--text))" }}
          >
            译文可能需要确认
          </div>
          <p
            className="m-0 mt-0.5 text-[11px] leading-relaxed"
            style={{ color: "var(--text-muted)" }}
          >
            {text}
          </p>
          <p
            className="m-0 mt-0.5 text-[11px]"
            style={{ color: "var(--text-muted)" }}
          >
            你仍可复制或编辑当前结果。
          </p>

          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <button
              className="btn h-7 rounded-md border px-2 text-[11px]"
              style={{
                background: "var(--surface)",
                borderColor: "var(--border-strong)",
                color: "var(--text)",
              }}
              onClick={onRetry}
            >
              重新翻译
            </button>
            {onSendToChat && (
              <button
                className="btn h-7 rounded-md px-1.5 text-[11px]"
                style={{ color: "var(--brand-text)" }}
                onClick={onSendToChat}
              >
                转到对话调整
              </button>
            )}
          </div>
        </div>

        <button
          className="btn shrink-0 rounded p-0.5 text-[11px] opacity-60 hover:opacity-100"
          style={{ color: "var(--text-muted)" }}
          onClick={() => setDismissed(true)}
          title="关闭提示（不影响译文）"
          aria-label="关闭提示"
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden>
            <path
              d="m6 6 12 12M18 6 6 18"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
            />
          </svg>
        </button>
      </div>
    </div>
  );
}
