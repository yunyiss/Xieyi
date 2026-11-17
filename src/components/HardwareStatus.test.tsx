import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { HardwareStatus } from "./HardwareStatus";

/**
 * 「通用」页的硬件状态。
 *
 * 这一块是**纯展示**：数据由 `ModelDrawer` 探测并传入。
 * 这样切走再切回页签时不会重新挂载探测、也不会闪「检测中…」——
 * 这里不测探测逻辑，只测“拿到什么数就显示什么”。
 *
 * 关键约束：`undefined`（还没读过）与 `null`（读过但读不到）语义不同，
 * 前者显示「检测中…」，后者显示「未检测到独立显卡 / 读取不到」。
 * **不能猜数字**。
 */

describe("HardwareStatus", () => {
  it("显存与内存都显示总量、可用量与已用百分比", () => {
    render(
      <HardwareStatus
        freeVramMb={5136}
        totalVramMb={8192}
        ram={{ totalMb: 16384, availableMb: 4096 }}
        onRedetect={() => {}}
      />,
    );

    expect(screen.getByText("8.0 GB · 可用 5.0 GB")).toBeInTheDocument();
    // 显存已用 = 8192 - 5136 = 3056 → 37%
    expect(screen.getByText("已用 37%")).toBeInTheDocument();

    expect(screen.getByText("16.0 GB · 可用 4.0 GB")).toBeInTheDocument();
    // 内存已用 = 16384 - 4096 = 12288 → 75%
    expect(screen.getByText("已用 75%")).toBeInTheDocument();
  });

  it("尚未探测时显示检测中，按钮禁用", () => {
    render(
      <HardwareStatus
        freeVramMb={null}
        totalVramMb={undefined}
        ram={undefined}
        onRedetect={() => {}}
      />,
    );

    expect(screen.getAllByText("检测中…").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByRole("button", { name: "检测中…" })).toBeDisabled();
  });

  it("读过但读不到总显存时明说未检测到独显，不猜数字", () => {
    render(
      <HardwareStatus
        freeVramMb={null}
        totalVramMb={null}
        ram={{ totalMb: 16384, availableMb: 4096 }}
        onRedetect={() => {}}
      />,
    );

    expect(screen.getByText("未检测到独立显卡")).toBeInTheDocument();
    expect(
      screen.getByText(/没有可用显存信息，将由 CPU 运行/),
    ).toBeInTheDocument();
  });

  it("总显存已知但可用读不到时，显示「可用未知」且不画显存条", () => {
    render(
      <HardwareStatus
        freeVramMb={null}
        totalVramMb={8192}
        ram={{ totalMb: 16384, availableMb: 4096 }}
        onRedetect={() => {}}
      />,
    );

    expect(screen.getByText("8.0 GB · 可用未知")).toBeInTheDocument();
    // 显存没有百分比，内存还有一个
    expect(screen.getAllByText(/^已用 /)).toHaveLength(1);
  });

  it("「重新检测」交给父组件处理", async () => {
    const onRedetect = vi.fn();
    const user = userEvent.setup();

    render(
      <HardwareStatus
        freeVramMb={5136}
        totalVramMb={8192}
        ram={{ totalMb: 16384, availableMb: 4096 }}
        onRedetect={onRedetect}
      />,
    );

    await user.click(screen.getByRole("button", { name: "重新检测" }));
    expect(onRedetect).toHaveBeenCalledTimes(1);
  });
});
