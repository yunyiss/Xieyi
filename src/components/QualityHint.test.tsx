import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { QualityHint, describeIssue } from "./QualityHint";
import type { QualityReport } from "../lib/ipc";

function report(codes: string[], fixed = false): QualityReport {
  return {
    requestId: "r1",
    segment: "hello",
    found: "可以忽略",
    codes,
    fixed,
  };
}

describe("describeIssue", () => {
  it("原文原样返回时说清楚", () => {
    expect(describeIssue(["untranslated"], "中文")).toContain(
      "译文与原文相同",
    );
  });

  it("语言不对时带上目标语言名", () => {
    expect(describeIssue(["wrong-script"], "日文")).toContain("日文");
  });

  it("多条原因合并成一句，不堆砌", () => {
    const text = describeIssue(["untranslated", "too-short"], "中文");
    // 只取最靠前的那条，不该两句都塞进去
    expect(text).toContain("译文与原文相同");
    expect(text.split("。").filter(Boolean).length).toBe(1);
  });

  it("认不出的原因码也给出兜底说明", () => {
    expect(describeIssue(["something-new"], "中文")).toContain("质量检查");
  });
});

describe("QualityHint", () => {
  it("修好的问题不打扰用户", () => {
    const { container } = render(
      <QualityHint
        reports={[report(["untranslated"], true)]}
        targetLabel="中文"
        onRetry={() => {}}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("没修好的问题展示标题与说明", () => {
    render(
      <QualityHint
        reports={[report(["untranslated"])]}
        targetLabel="中文"
        onRetry={() => {}}
      />,
    );
    expect(screen.getByText("译文可能需要确认")).toBeInTheDocument();
    expect(screen.getByText(/你仍可复制或编辑/)).toBeInTheDocument();
  });

  it("关闭后整个提示消失", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <QualityHint
        reports={[report(["untranslated"])]}
        targetLabel="中文"
        onRetry={() => {}}
      />,
    );
    await user.click(screen.getByLabelText("关闭提示"));
    expect(container).toBeEmptyDOMElement();
  });

  it("有回话入口时才显示「转到对话调整」", () => {
    const { rerender } = render(
      <QualityHint
        reports={[report(["untranslated"])]}
        targetLabel="中文"
        onRetry={() => {}}
      />,
    );
    expect(screen.queryByText("转到对话调整")).not.toBeInTheDocument();

    rerender(
      <QualityHint
        reports={[report(["untranslated"])]}
        targetLabel="中文"
        onRetry={() => {}}
        onSendToChat={() => {}}
      />,
    );
    expect(screen.getByText("转到对话调整")).toBeInTheDocument();
  });

  it("两个按钮各自触发回调", async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    const onSend = vi.fn();
    render(
      <QualityHint
        reports={[report(["wrong-script"])]}
        targetLabel="日文"
        onRetry={onRetry}
        onSendToChat={onSend}
      />,
    );
    await user.click(screen.getByText("重新翻译"));
    expect(onRetry).toHaveBeenCalledOnce();
    await user.click(screen.getByText("转到对话调整"));
    expect(onSend).toHaveBeenCalledOnce();
  });
});
