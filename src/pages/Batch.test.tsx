import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { Batch } from "./Batch";

/**
 * 批量页组件测试。
 *
 * 这是本项目首次给 `Batch.tsx` 补测试 —— 它 542 行、且最近刚加了
 * 断点续传，此前完全靠手测（技术债 #8）。
 *
 * 重点覆盖「断点续传在界面上到底看不看得见」：后端会通过
 * `batch://progress` 上报 `resumedSegments`，界面据此显示「续」标记。
 * 这条如果坏了，用户会以为程序抽风（一上来就 250/300）。
 */

// Tauri 的拖放 API：jsdom 里没有，必须打桩。
// 返回一个永远不触发事件的假监听器即可。
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async () => () => {},
  }),
}));

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    getRuntimeStatus: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    })),
    startService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
    })),
    // 页面启动改用本地扫描（不拉引擎），不再用 listRouterModels
    listLocalModels: vi.fn(async () => [
      {
        id: "demo.gguf",
        path: "D:/models/demo.gguf",
        sizeMb: 1024,
        ctxSize: 4096,
        trainedCtx: 4096,
        supportsThinking: false,
        thinking: "off",
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
    ]),
    filterBatchPaths: actual.filterBatchPaths,
    baseName: actual.baseName,
    batchTranslate: vi.fn(async () => ({
      total: 1,
      succeeded: 1,
      failed: 0,
      skipped: 0,
      failedSegments: 0,
      qualitySegments: 0,
      qualityFixed: 0,
      elapsedMs: 1000,
      cancelled: false,
    })),
    batchCancel: vi.fn(async () => {}),
    pickBatchFiles: vi.fn(async () => []),
    // 进度监听交给测试自己控制
    // 必须返回 Promise<UnlistenFn>：useTauriEvent 会 .then()
    onBatchProgress: vi.fn(async () => () => {}),
    onModelState: vi.fn(async () => () => {}),
  };
});

/** 抓取 `onBatchProgress` 注册进去的回调，用于手动推送进度事件。 */
function takeProgressHandler(): (p: unknown) => void {
  // onBatchProgress 的签名是 (cb) => Promise<UnlistenFn>，
  // 所以回调永远是第 0 个实参
  const calls = vi.mocked(ipc.onBatchProgress).mock.calls;
  const last = calls[calls.length - 1];
  return last[0] as unknown as (p: unknown) => void;
}

/** 往队列里塞一个文件（绕过真实的文件选择对话框）。 */
async function queueOneFile(user: ReturnType<typeof userEvent.setup>) {
  vi.mocked(ipc.pickBatchFiles).mockResolvedValueOnce(["C:/docs/report.md"]);
  const add = await screen.findByRole("button", { name: /添加文件/ });
  await user.click(add);
  await screen.findByText("report.md");
}

function progress(over: Record<string, unknown> = {}) {
  return {
    path: "C:/docs/report.md",
    index: 0,
    total: 1,
    stage: "translating",
    done: 0,
    segments: 10,
    failedSegments: 0,
    ...over,
  };
}

describe("Batch · 断点续传的界面反馈", () => {
  beforeEach(() => {
    vi.mocked(ipc.batchTranslate).mockClear();
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    });
    vi.mocked(ipc.listLocalModels).mockResolvedValue([
      {
        id: "demo.gguf",
        path: "D:/models/demo.gguf",
        sizeMb: 1024,
        ctxSize: 4096,
        trainedCtx: 4096,
        supportsThinking: false,
        thinking: "off",
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
    ]);
  });

  it("续传时显示「续」标记与已恢复的段数", async () => {
    const user = userEvent.setup();
    render(<Batch />);
    await queueOneFile(user);

    const push = takeProgressHandler();
    // 后端告知：这个文件有 4 段是从上次进度恢复的
    push(progress({ resumedSegments: 4, done: 4, segments: 10 }));

    await waitFor(() => {
      expect(screen.getByText("续")).toBeInTheDocument();
    });
    // 进度从 4 开始，不是 0
    expect(screen.getByText(/4\/10/)).toBeInTheDocument();
  });

  it("全新任务不显示「续」标记", async () => {
    const user = userEvent.setup();
    render(<Batch />);
    await queueOneFile(user);

    const push = takeProgressHandler();
    push(progress({ resumedSegments: 0, done: 3, segments: 10 }));

    await waitFor(() => {
      expect(screen.getByText(/3\/10/)).toBeInTheDocument();
    });
    // 没有续传就不该出现这个标记
    expect(screen.queryByText("续")).toBeNull();
  });

  it("翻译完成后「续」标记消失", async () => {
    const user = userEvent.setup();
    render(<Batch />);
    await queueOneFile(user);

    const push = takeProgressHandler();
    push(progress({ resumedSegments: 2, done: 2, segments: 10 }));
    await waitFor(() => expect(screen.getByText("续")).toBeInTheDocument());

    // 阶段变成 done：不再处于 translating，标记应撤掉
    push(
      progress({
        stage: "done",
        resumedSegments: 2,
        done: 10,
        segments: 10,
        output: "C:/docs/report.zh.md",
      }),
    );
    await waitFor(() => {
      expect(screen.queryByText("续")).toBeNull();
    });
  });

  it("失败片段会在完成后汇总里体现", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.batchTranslate).mockResolvedValueOnce({
      total: 1,
      succeeded: 1,
      failed: 0,
      skipped: 0,
      failedSegments: 2,
      qualitySegments: 0,
      qualityFixed: 0,
      elapsedMs: 2000,
      cancelled: false,
    });

    render(<Batch />);
    await queueOneFile(user);

    const run = await screen.findByRole("button", { name: /开始翻译/ });
    await user.click(run);

    // 「2 段保留原文」这句是用户判断"要不要重翻"的依据
    await waitFor(() => {
      expect(screen.getByText(/2\s*段保留原文/)).toBeInTheDocument();
    });
  });

  it("不支持的文件类型会被拒绝，不进入队列", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.pickBatchFiles).mockResolvedValueOnce(["C:/docs/bad.docx"]);

    render(<Batch />);
    const add = await screen.findByRole("button", { name: /添加文件/ });
    await user.click(add);

    await waitFor(() => {
      expect(screen.getByText(/只支持/)).toBeInTheDocument();
    });
    expect(screen.queryByText("bad.docx")).toBeNull();
  });
});
