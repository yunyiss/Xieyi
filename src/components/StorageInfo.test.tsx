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
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
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
    // AppInfo 也要复位：某条用例会把它改成 linux 平台，
    // 不清的话后面的用例会继承成 Linux 写法。
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data/com.xieyi.translate",
      modelsDir: "D:/models",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    } as never);
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

  it("标准模式显示「标准模式」标签", async () => {
    render(<StorageInfo />);
    await waitFor(() => {
      expect(screen.getByText("标准模式")).toBeInTheDocument();
    });
  });

  it("便携模式显示「便携模式」标签", async () => {
    // 用户把 exe 拖走后会从便携变回标准模式，
    // 标签是唯一能告诉他「现在在哪种模式」的地方
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "D:/Xieyi-0.1.0-win64/data",
      modelsDir: "D:/Xieyi-0.1.0-win64/models",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: true,
    });
    render(<StorageInfo />);
    await waitFor(() => {
      expect(screen.getByText("便携模式")).toBeInTheDocument();
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

  /**
   * 平台差异：webview 缓存目录名与路径分隔符都不能写死 Windows 的。
   *
   * 实际踩到过：Linux 截图里「界面缓存」显示成 `EBWebView` 加反斜杠，
   * 而 Linux 上真实目录是 `WebKitCache`——用户照着名字去文件夹里找会
   * 找不到。
   *
   * 变异验证：把 `webviewCache` 写死成 "EBWebView"，本测试失败。
   */
  it("Linux 上显示 WebKitCache 与正斜杠", async () => {
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "linux",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "/home/u/.local/share/com.xieyi.translate",
      modelsDir: "/home/u/models",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    } as never);

    render(<StorageInfo />);

    // 等这一块渲染出来，再用全文断言——子项文本被截断样式包着，
    // 精确 getByText 匹配很容易被空白/截断搞崩。
    await waitFor(() => screen.getByText(/界面缓存/));
    const text = document.body.textContent ?? "";
    expect(text).toContain("WebKitCache");
    expect(text).not.toContain("EBWebView");
    expect(text).toContain("sessions/");
    expect(text).toContain("logs/");
    expect(text).toContain("cache/");
  });

  /** Windows 上仍显示 EBWebView 与反斜杠。 */
  it("Windows 上显示 EBWebView 与反斜杠", async () => {
    render(<StorageInfo />);
    // 用正则避开源码里反斜杠的转义噪音
    expect(await screen.findByText(/^EBWebView.$/)).toBeInTheDocument();
    expect(screen.getByText(/^sessions.$/)).toBeInTheDocument();
  });

  /**
   * 「打开」失败时必须说清原因。
   *
   * 早先这些按钮是 `onClick={() => openDataSubdir("root")}`——没有 await、
   * 没有 catch。后端返回错误时前端**静默吞掉**，按钮点了毫无反应，
   * 用户既不知道失败也看不到原因。
   *
   * 变异验证：把 onClick 改回直接调用、去掉 openDir 包装，本测试失败。
   */
  it("打开目录失败时显示后端给的原因", async () => {
    vi.mocked(ipc.openDataSubdir).mockRejectedValueOnce(
      new Error("无法打开目录 C:/data：拒绝访问"),
    );
    const user = userEvent.setup();
    render(<StorageInfo />);
    await waitFor(() => screen.getByText("12"));

    // 「数据根目录」那一行的打开按钮
    const btns = screen.getAllByRole("button", { name: "打开" });
    await user.click(btns[0]);

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(/拒绝访问/);
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
