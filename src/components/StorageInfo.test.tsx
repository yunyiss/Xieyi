import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { StorageInfo } from "./StorageInfo";

/**
 * 「存储位置 / 翻译缓存」区块。
 *
 * 重点测三件容易出错的事：
 * 1. 落盘不可用时必须**明说**——否则用户以为翻过的下次还在
 * 2. 没有缓存时清空按钮要禁用——避免点了没反应让人困惑
 * 3. 清空后要刷新成 0，并告诉用户清掉了几条
 */
vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    getAppInfo: vi.fn(async () => ({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data/com.xieyi.translate",
      modelsDir: "D:/models",
      runtimeVersion: "b11714",
    })),
    getRuntimeStatus: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    })),
    getCacheStatus: vi.fn(async () => ({
      entries: 12,
      bytes: 2048,
      persistent: true,
    })),
    clearTranslateCache: vi.fn(async () => 12),
    openDataSubdir: vi.fn(async () => ""),
    openEngineDir: vi.fn(async () => ""),
  };
});

describe("StorageInfo · 翻译缓存", () => {
  beforeEach(() => {
    // 只清调用记录，**不清实现**：`vi.clearAllMocks()` 会把 mockResolvedValue
    // 一起复位，导致后面的用例拿到 undefined。用 reset 也不行（同样复位实现）。
    // 这里的默认实现定义在 vi.mock 工厂里，需要保留。
    vi.mocked(ipc.getCacheStatus).mockResolvedValue({
      entries: 12,
      bytes: 2048,
      persistent: true,
    });
    vi.mocked(ipc.clearTranslateCache).mockResolvedValue(12);
  });

  it("显示条数与占用", async () => {
    render(<StorageInfo />);
    await waitFor(() => {
      expect(screen.getByText("12")).toBeTruthy();
    });
    expect(screen.getByText("2.0 KB")).toBeTruthy();
  });

  it("落盘可用时不显示警告", async () => {
    render(<StorageInfo />);
    await waitFor(() => screen.getByText("12"));
    expect(screen.queryByText(/落盘不可用/)).toBeNull();
  });

  it("落盘不可用时要明说，不能让人以为重启还在", async () => {
    vi.mocked(ipc.getCacheStatus).mockResolvedValueOnce({
      entries: 3,
      bytes: 0,
      persistent: false,
    });
    render(<StorageInfo />);
    await waitFor(() => {
      expect(screen.getByText(/落盘不可用/)).toBeTruthy();
    });
  });

  it("没有缓存时清空按钮禁用", async () => {
    vi.mocked(ipc.getCacheStatus).mockResolvedValueOnce({
      entries: 0,
      bytes: 0,
      persistent: true,
    });
    render(<StorageInfo />);
    const btn = await screen.findByRole("button", { name: "清空" });
    await waitFor(() => expect(btn.hasAttribute("disabled")).toBe(true));
  });

  it("清空后刷新成 0 并报告清掉几条", async () => {
    const user = userEvent.setup();
    render(<StorageInfo />);
    await waitFor(() => screen.getByText("12"));

    // 清空之后后端返回 0 条
    vi.mocked(ipc.getCacheStatus).mockResolvedValue({
      entries: 0,
      bytes: 0,
      persistent: true,
    });

    await user.click(screen.getByRole("button", { name: "清空" }));

    expect(ipc.clearTranslateCache).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(screen.getByText(/已清空 12 条/)).toBeTruthy();
    });
  });

  it("清空失败时显示错误，不静默吞掉", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.clearTranslateCache).mockRejectedValueOnce(
      new Error("磁盘只读"),
    );
    render(<StorageInfo />);
    await waitFor(() => screen.getByText("12"));

    await user.click(screen.getByRole("button", { name: "清空" }));

    await waitFor(() => {
      expect(screen.getByText(/磁盘只读/)).toBeTruthy();
    });
  });

  it("「缓存」措辞不混淆：译文缓存与界面缓存分开标注", async () => {
    render(<StorageInfo />);
    await waitFor(() => screen.getByText("12"));
    // 早先两处都叫「缓存」，用户分不清哪个是可清的
    expect(screen.getByText("译文缓存")).toBeTruthy();
    expect(screen.getByText("界面缓存")).toBeTruthy();
  });
});
