import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  ModelLoadInfoChip,
  toLoadActual,
  type LoadActual,
} from "./ModelLoadInfoChip";
import type { ModelStatePayload } from "../lib/ipc";

const GB = 1024 ** 3;

function actual(over: Partial<LoadActual> = {}): LoadActual {
  return {
    model: "Hy-MT2-7B-Q4_K_M",
    gpuLayers: 32,
    totalLayers: 32,
    ctx: 4096,
    kvBytes: 128 * 1024 * 1024,
    gpuModelBytes: 4 * GB,
    cpuModelBytes: 256 * 1024 * 1024,
    vramProjectedBytes: 5 * GB,
    vramFreeBytes: 7 * GB,
    ...over,
  };
}

describe("ModelLoadInfoChip", () => {
  it("全 GPU 时只显示层数与上下文，不出现告警", async () => {
    const user = userEvent.setup();
    render(<ModelLoadInfoChip actual={actual()} model="Hy-MT2-7B-Q4_K_M" />);

    expect(screen.getByText("32/32 层")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "32/32 层" }));

    expect(screen.getByText("本次加载的实际分配")).toBeInTheDocument();
    expect(screen.queryByText(/驻留在系统内存/)).not.toBeInTheDocument();
    expect(screen.queryByText(/未启用 GPU 加速/)).not.toBeInTheDocument();
  });

  /**
   * 部分层在内存里时给出书面告警。
   *
   * 变异验证：把 partial 分支删掉，本测试失败。
   */
  it("部分层驻留内存时显示告警", async () => {
    const user = userEvent.setup();
    render(
      <ModelLoadInfoChip
        actual={actual({ gpuLayers: 24, totalLayers: 32 })}
        model="Hy-MT2-7B-Q4_K_M"
      />,
    );

    await user.click(screen.getByRole("button", { name: /24\/32 层/ }));

    expect(
      screen.getByText(/当前有 8 层驻留在系统内存/),
    ).toBeInTheDocument();
  });

  /**
   * 一层都没上显卡时，文案要说明“未启用 GPU 加速”。
   *
   * 变异验证：把 noGpu 分支删掉，本测试失败。
   */
  it("0 层上卡时提示未启用 GPU", async () => {
    const user = userEvent.setup();
    render(
      <ModelLoadInfoChip
        actual={actual({ gpuLayers: 0, totalLayers: 32 })}
        model="Hy-MT2-7B-Q4_K_M"
      />,
    );

    await user.click(screen.getByRole("button", { name: /0\/32 层/ }));

    expect(screen.getByText(/未启用 GPU 加速/)).toBeInTheDocument();
  });

  it("模型不匹配或没有实际数据时不渲染", () => {
    const a = render(
      <ModelLoadInfoChip actual={actual()} model="other-model" />,
    );
    expect(a.container.textContent).toBe("");
    a.unmount();

    const b = render(<ModelLoadInfoChip actual={null} model="m" />);
    expect(b.container.textContent).toBe("");
  });

  /**
   * 事件映射：只有 warm/loaded 且带层数时才算“实际分配”。
   *
   * 变异验证：把 state 判断删掉，loading 事件也会生成 chip，本测试失败。
   */
  it("只从 warm/loaded 且带层数的事件里提取实际分配", () => {
    const base: ModelStatePayload = {
      model: "m",
      state: "warm",
      gpuLayers: 24,
      totalLayers: 32,
      ctx: 4096,
    };
    expect(toLoadActual(base)?.gpuLayers).toBe(24);
    expect(toLoadActual({ ...base, state: "loading" })).toBeNull();
    expect(toLoadActual({ ...base, gpuLayers: undefined })).toBeNull();
  });
});
