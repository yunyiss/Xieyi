import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { LoadIndicator, toLoadState, type LoadState } from "./LoadIndicator";
import type { ModelStatePayload } from "../lib/ipc";

const GB = 1024 ** 3;

function loading(over: Partial<LoadState> = {}): LoadState {
  return {
    model: "Hy-MT2-1.8B-Q8_0",
    state: "loading",
    startAt: Date.now(),
    estimateMs: 8000,
    stage: "read",
    percent: 34,
    readBytes: 3 * GB,
    totalBytes: 4 * GB,
    speedBps: 80 * 1024 * 1024,
    ...over,
  };
}

describe("LoadIndicator", () => {
  /**
   * 阶段信息存在时，默认只占一行：当前阶段 + 读取信息。
   *
   * 变异验证：把 stage 分支删掉、永远走旧单条进度条，本测试失败。
   */
  it("默认一行显示当前阶段与读取信息", () => {
    render(<LoadIndicator load={loading()} />);

    expect(screen.getByText("读取模型")).toBeInTheDocument();
    expect(
      screen.getByText(/约 3\.0 GB \/ 4\.0 GB · 80 MB\/s · 还需约 13 秒/),
    ).toBeInTheDocument();
    // 没展开时，后面的阶段标签不应出现
    expect(screen.queryByText("分配缓存")).not.toBeInTheDocument();
  });

  /**
   * 点「详情」才展开五阶段；这样不会在每次加载时把输入区顶下去。
   *
   * 变异验证：把展开面板删掉，本测试失败。
   */
  it("点详情后展开完整五阶段", async () => {
    const user = userEvent.setup();
    render(<LoadIndicator load={loading()} />);

    await user.click(screen.getByRole("button", { name: /详情/ }));

    for (const label of ["准备", "读取模型", "分配缓存", "预热", "完成"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
  });

  /**
   * 后端拿不到阶段（老引擎/日志格式变了）时，退回原来的单条进度条，
   * 不能因为 stage 为空就什么都不显示。
   *
   * 变异验证：把 `if (!stage)` 的兜底删掉，本测试失败。
   */
  it("没有阶段信息时退回单条进度条", () => {
    render(<LoadIndicator load={loading({ stage: null })} />);

    expect(screen.getByText(/加载中/)).toBeInTheDocument();
  });

  /**
   * 事件映射：`stage` / 读取数据要原样带进 LoadState。
   *
   * 变异验证：把 toLoadState 里的字段映射删掉，本测试失败。
   */
  it("把后端事件里的阶段与读取数据映射进来", () => {
    const payload: ModelStatePayload = {
      model: "m",
      state: "loading",
      stage: "read",
      elapsedMs: 1000,
      estimateMs: 8000,
      percent: 20,
      readBytes: 1,
      totalBytes: 2,
      speedBps: 3,
    };

    const state = toLoadState(payload);

    expect(state).not.toBeNull();
    expect(state?.stage).toBe("read");
    expect(state?.readBytes).toBe(1);
    expect(state?.totalBytes).toBe(2);
    expect(state?.speedBps).toBe(3);
  });

  it("非加载态返回 null", () => {
    expect(toLoadState({ model: "m", state: "warm" })).toBeNull();
  });
});
