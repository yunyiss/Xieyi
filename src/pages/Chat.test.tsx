import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import React from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import type { TokenPayload } from "../lib/ipc";
import { Chat } from "./Chat";

/** 流式事件回调：测试里手动触发，模拟后端逐字返回。 */
const streams = vi.hoisted(() => ({
  token: null as ((p: { requestId: string; token: string }) => void) | null,
  reasoning: null as ((p: { requestId: string; token: string }) => void) | null,
  /** 当前仍然挂着的思维链订阅者。泄漏时这里会有多个 */
  reasoningListeners: [] as ((p: { requestId: string; token: string }) => void)[],
}));

/**
 * 组件测试不连 Tauri：把整个 `lib/ipc` 换成可控的假实现，
 * 命令直接返回、事件由测试手动推送。
 */
vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();

  return {
    ...actual,
    listRouterModels: vi.fn(async () => [{ id: "demo.gguf", state: "loaded" }]),
    listSessions: vi.fn(async () => []),
    listLocalModels: vi.fn(async () => [
      {
        id: "demo.gguf",
        path: "C:/models/demo.gguf",
        sizeMb: 1024,
        ctxSize: 4096,
        trainedCtx: 8192,
        supportsThinking: false,
        thinking: "off" as const,
      },
    ]),
    newRequestId: vi.fn(() => "req-1"),
    thinkingTiers: vi.fn(async () => []),
    saveSession: vi.fn(async () => {}),
    setModelParams: vi.fn(async () => ({ ctx: 4096, thinking: "off" as const })),
    deleteSession: vi.fn(async () => {}),
    deleteAllSessions: vi.fn(async () => 0),
    chat: vi.fn(),
    abort: vi.fn(async () => {}),
    onToken: vi.fn(async (cb: (p: TokenPayload) => void) => {
      streams.token = cb;
      return () => {};
    }),
    onReasoning: vi.fn(async (cb: (p: TokenPayload) => void) => {
      // 像真实的 listen() 一样：登记订阅者，注销时移除。
      // 只有真正注销干净，界面才不会重复累加——这正是被验证的行为。
      streams.reasoning = cb;
      streams.reasoningListeners.push(cb);
      return () => {
        streams.reasoningListeners = streams.reasoningListeners.filter(
          (fn) => fn !== cb,
        );
      };
    }),
    onUsage: vi.fn(async () => () => {}),
    onModelState: vi.fn(async () => () => {}),
  };
});

const chatMock = vi.mocked(ipc.chat);
const saveMock = vi.mocked(ipc.saveSession);
const abortMock = vi.mocked(ipc.abort);

beforeEach(() => {
  vi.clearAllMocks();
  chatMock.mockReset();
  abortMock.mockReset();
  abortMock.mockResolvedValue(undefined);
  streams.token = null;
  streams.reasoning = null;
});

/** 渲染对话页并等到模型列表加载完成（发送按钮可用）。 */
async function renderReady() {
  render(<Chat />);
  await waitFor(() => expect(screen.getByLabelText("发送")).toBeEnabled());
}

/** 输入一条消息并用 Enter 发送。 */
async function sendText(text: string) {
  const user = userEvent.setup();
  await user.type(screen.getByPlaceholderText("输入消息…"), text);
  await user.keyboard("{Enter}");
}

describe("Chat", () => {
  it("流式回复按消息 id 追加，并持久化带 id/status 的会话", async () => {
    chatMock.mockImplementation(async () => {
      await act(async () => {
        streams.token?.({ requestId: "req-1", token: "你好" });
        streams.token?.({ requestId: "req-1", token: "，世界" });
      });
      return "你好，世界";
    });

    await renderReady();
    await sendText("在吗");

    expect(await screen.findByText("你好，世界")).toBeInTheDocument();
    expect(screen.getByText("在吗")).toBeInTheDocument();

    await waitFor(() => expect(saveMock).toHaveBeenCalled());
    const calls = saveMock.mock.calls;
    const saved = calls[calls.length - 1]?.[0];
    expect(saved?.messages.map((m) => [m.role, m.status])).toEqual([
      ["user", "ready"],
      ["assistant", "ready"],
    ]);

    // 每条消息都有独立 id，动作靠它定位
    const ids = saved?.messages.map((m) => m.id) ?? [];
    expect(ids.every(Boolean)).toBe(true);
    expect(new Set(ids).size).toBe(ids.length);

    expect(chatMock).toHaveBeenCalledWith(
      expect.objectContaining({ requestId: "req-1", model: "demo.gguf" }),
    );
  });

  it("失败且一个字都没流出时收掉空回复气泡，保留重试入口", async () => {
    chatMock.mockRejectedValueOnce(new Error("模型未加载"));

    await renderReady();
    await sendText("在吗");

    expect(await screen.findByRole("alert")).toHaveTextContent("模型未加载");
    // 只剩提问，最后一条提问上出现「重试」
    expect(screen.getByText("在吗")).toBeInTheDocument();
    expect(screen.getByText("重试")).toBeInTheDocument();
    expect(
      screen.queryByText("生成中断，回复可能不完整"),
    ).not.toBeInTheDocument();
  });

  it("主动停止不弹错误，半截回复标记为 stopped", async () => {
    let rejectChat: (e: Error) => void = () => {};
    chatMock.mockImplementation(
      () =>
        new Promise<string>((_, reject) => {
          rejectChat = reject;
        }),
    );
    abortMock.mockImplementation(async () => {
      // 后端中断会让这次 chat 以「已取消」失败
      rejectChat(new Error("已取消"));
    });

    await renderReady();
    await sendText("在吗");

    await waitFor(() => expect(chatMock).toHaveBeenCalled());
    await waitFor(() => expect(streams.token).not.toBeNull());
    act(() => streams.token?.({ requestId: "req-1", token: "半截" }));

    const user = userEvent.setup();
    await user.click(screen.getByLabelText("停止"));

    expect(
      await screen.findByText("已停止，回复可能不完整"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("编辑并重发会丢弃这条之后的消息", async () => {
    chatMock.mockResolvedValue("第一次回答");

    await renderReady();
    await sendText("原始问题");
    expect(await screen.findByText("第一次回答")).toBeInTheDocument();

    // 悬停才出现的动作条：直接点「编辑」
    const user = userEvent.setup();
    await user.click(screen.getByText("编辑"));
    const editor = screen.getByDisplayValue("原始问题");
    await user.clear(editor);
    await user.type(editor, "改过的问题");

    chatMock.mockResolvedValue("第二次回答");
    await user.click(screen.getByText("保存并重发"));

    expect(await screen.findByText("第二次回答")).toBeInTheDocument();
    expect(screen.queryByText("第一次回答")).not.toBeInTheDocument();
    expect(screen.queryByText("原始问题")).not.toBeInTheDocument();
    expect(screen.getByText("改过的问题")).toBeInTheDocument();
  });

  /**
   * 重新生成不能把上一版丢掉——用户要对比"更正式"和"更简洁"两种译法，
   * 覆盖式重新生成会让对比变成不可能。
   */
  it("重新生成保留旧版，可以切换回去对比", async () => {
    chatMock.mockResolvedValue("第一版译文");

    await renderReady();
    await sendText("翻译这段");
    expect(await screen.findByText("第一版译文")).toBeInTheDocument();

    // 此时只有一版，不该出现版本计数器
    expect(screen.queryByText("1/1")).not.toBeInTheDocument();

    chatMock.mockResolvedValue("第二版译文");
    const user = userEvent.setup();
    await user.click(screen.getByText("重新生成"));

    expect(await screen.findByText("第二版译文")).toBeInTheDocument();
    // 出现了 2/2 计数器
    expect(await screen.findByText("2/2")).toBeInTheDocument();

    // 切回上一版：旧译文回来了，且没有重新请求模型
    const asked = chatMock.mock.calls.length;
    await user.click(screen.getByLabelText("上一版"));

    expect(await screen.findByText("第一版译文")).toBeInTheDocument();
    expect(screen.queryByText("第二版译文")).not.toBeInTheDocument();
    expect(await screen.findByText("1/2")).toBeInTheDocument();
    expect(chatMock.mock.calls.length).toBe(asked);

    // 再切回新版
    await user.click(screen.getByLabelText("下一版"));
    expect(await screen.findByText("第二版译文")).toBeInTheDocument();
    expect(await screen.findByText("2/2")).toBeInTheDocument();
  });

  /**
   * 重新生成失败时，旧版必须原样留着。
   *
   * 这里踩过一个坑：重新生成沿用了原消息 id，于是失败清理路径
   * （"一个字都没流出就把这条气泡收掉"）会把带着旧版本的同一条消息一起删掉，
   * 用户点一次重新生成、赶上请求失败，之前辛苦调出来的译文就没了。
   */
  it("重新生成失败时不会丢掉旧版译文", async () => {
    chatMock.mockResolvedValue("珍贵的第一版");

    await renderReady();
    await sendText("翻译这段");
    expect(await screen.findByText("珍贵的第一版")).toBeInTheDocument();

    // 第二次请求直接失败，且没有任何内容流出
    chatMock.mockRejectedValueOnce(new Error("模型未加载"));
    const user = userEvent.setup();
    await user.click(screen.getByText("重新生成"));

    expect(await screen.findByRole("alert")).toHaveTextContent("模型未加载");
    // 旧译文还在
    expect(screen.getByText("珍贵的第一版")).toBeInTheDocument();
  });

  /**
   * 思维链曾被重复显示：StrictMode 会「挂载→卸载→再挂载」，
   * 而 listen() 是异步的，cleanup 拿不到注销函数，于是留下两个监听器，
   * 每个 token 被累加两次（`Thinking` 变成 `ThinkingThinking`）。
   *
   * 这条用例把「同一个 token 只追加一次」钉死：用 StrictMode 渲染，
   * 推一个 token，界面上的思维链必须与推入内容完全一致。
   */
  it("StrictMode 下思维链不会重复累加", async () => {
    chatMock.mockImplementation(async () => {
      await act(async () => {
        // 推给所有仍然挂着的监听器：如果卸载没注销干净，
        // 同一个 token 就会在界面里被追加两次
        for (const fn of streams.reasoningListeners) {
          fn({ requestId: "req-1", token: "分析" });
        }
      });
      return "好的";
    });

    render(
      <React.StrictMode>
        <Chat />
      </React.StrictMode>,
    );
    await waitFor(() => expect(screen.getByLabelText("发送")).toBeEnabled());
    await sendText("在吗");

    // 面板里的字数应当就是 2（"分析"），而不是 4
    expect(await screen.findByText("2 字")).toBeInTheDocument();
    expect(screen.getAllByText("分析")).toHaveLength(1);
  });
});
