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
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
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
    compactHistory: vi.fn(async () => "压缩后的要点"),
  };
});

const chatMock = vi.mocked(ipc.chat);
const compactMock = vi.mocked(ipc.compactHistory);
const saveMock = vi.mocked(ipc.saveSession);
const abortMock = vi.mocked(ipc.abort);
const listLocalMock = vi.mocked(ipc.listLocalModels);
const listRouterMock = vi.mocked(ipc.listRouterModels);

/** 默认的"一个通用对话模型"环境；个别用例会覆盖它。 */
function useDefaultModels() {
  listRouterMock.mockResolvedValue([{ id: "demo.gguf", state: "loaded" }]);
  listLocalMock.mockResolvedValue([
    {
      id: "demo.gguf",
      path: "C:/models/demo.gguf",
      sizeMb: 1024,
      ctxSize: 4096,
      trainedCtx: 8192,
      supportsThinking: false,
      thinking: "off" as const,
      chatCapable: true,
      family: "通用",
      unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
    },
  ]);
}

beforeEach(() => {
  vi.clearAllMocks();
  chatMock.mockReset();
  compactMock.mockReset();
  compactMock.mockResolvedValue("压缩后的要点");
  abortMock.mockReset();
  abortMock.mockResolvedValue(undefined);
  // 模型列表是模块级 mock，被某个用例改过之后会影响后续用例，
  // 所以每条用例开跑前都恢复默认。
  useDefaultModels();
  streams.token = null;
  streams.reasoning = null;
});

/** 渲染对话页并等到模型列表加载完成（发送按钮可用）。 */
async function renderReady() {
  render(<Chat />);
  await waitFor(() => expect(screen.getByLabelText("发送")).toBeEnabled());
  // `refresh()` 里 listRouterModels / listSessions / listLocalModels 是三个
  // 独立 await，发送按钮只依赖第一个。localModels 决定 ctxSize（进而决定
  // 是否压缩），所以要把剩余两个也等完——用一个微任务轮次即可。
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** 输入一条消息并用 Enter 发送。 */
async function sendText(text: string) {
  const user = userEvent.setup();
  await user.type(screen.getByPlaceholderText("输入消息…"), text);
  await user.keyboard("{Enter}");
}

describe("Chat", () => {
  /**
   * 翻译专用模型不能出现在对话页的模型列表里。
   *
   * 实测这类模型（translategemma、LMT-60）会把用户的问题本身翻译掉，
   * 或者编造身份，放在对话页只会让用户以为工具坏了。
   */
  it("翻译专用模型不出现在对话页的模型列表", async () => {
    listRouterMock.mockResolvedValue([
      { id: "qwen.gguf", state: "unloaded" },
      { id: "hy-mt2.gguf", state: "unloaded" },
    ]);
    listLocalMock.mockResolvedValue([
      {
        id: "qwen.gguf",
        path: "C:/models/qwen.gguf",
        sizeMb: 4000,
        ctxSize: 8192,
        trainedCtx: 32768,
        supportsThinking: true,
        thinking: "off" as const,
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
      {
        id: "hy-mt2.gguf",
        path: "C:/models/hy-mt2.gguf",
        sizeMb: 1100,
        ctxSize: 4096,
        trainedCtx: 32768,
        supportsThinking: false,
        thinking: "off" as const,
        chatCapable: false,
        family: "Hy-MT2",
        unsupportedReason: null,
        chatAuto: false,
        chatOverridden: false,
      },
    ]);

    await renderReady();

    // 打开模型选择器：只应看到通用模型，翻译模型被过滤掉。
    // 注意两点：界面会把 ".gguf" 去掉再显示；选中项在触发器和列表里各出现一次，
    // 所以按"至少出现一次"断言。
    const user = userEvent.setup();
    await user.click(screen.getByLabelText("模型与思考强度"));
    expect(screen.queryByText("hy-mt2")).not.toBeInTheDocument();
    expect(screen.getAllByText("qwen").length).toBeGreaterThan(0);
  });

  /** 目录里只有翻译模型时，要明确告诉用户为什么对话页用不了。 */
  it("只有翻译模型时给出明确说明", async () => {
    listRouterMock.mockResolvedValue([{ id: "hy-mt2.gguf", state: "unloaded" }]);
    listLocalMock.mockResolvedValue([
      {
        id: "hy-mt2.gguf",
        path: "C:/models/hy-mt2.gguf",
        sizeMb: 1100,
        ctxSize: 4096,
        trainedCtx: 32768,
        supportsThinking: false,
        thinking: "off" as const,
        chatCapable: false,
        family: "Hy-MT2",
        unsupportedReason: null,
        chatAuto: false,
        chatOverridden: false,
      },
    ]);

    render(<Chat />);
    expect(await screen.findByText(/只有翻译专用模型/)).toBeInTheDocument();
  });

  /**
   * 手动把某个模型改成"可对话"后，它应当出现在对话页。
   *
   * 这是"自动识别猜错了"的兜底：用户不需要去改模型文件名，
   * 在设置里打个勾就行。
   */
  it("手动放开能力限制后，翻译模型也能进对话页", async () => {
    listRouterMock.mockResolvedValue([
      { id: "qwen.gguf", state: "unloaded" },
      { id: "tg.gguf", state: "unloaded" },
    ]);
    listLocalMock.mockResolvedValue([
      {
        id: "qwen.gguf",
        path: "C:/models/qwen.gguf",
        sizeMb: 4000,
        ctxSize: 8192,
        trainedCtx: 32768,
        supportsThinking: true,
        thinking: "off" as const,
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
      {
        // 自动判为不可对话，但用户手动开了
        id: "tg.gguf",
        path: "C:/models/tg.gguf",
        sizeMb: 3800,
        ctxSize: 4096,
        trainedCtx: 131072,
        supportsThinking: false,
        thinking: "off" as const,
        chatCapable: true,
        family: "TranslateGemma",
        unsupportedReason: null,
        chatAuto: false,
        chatOverridden: true,
      },
    ]);

    await renderReady();

    const user = userEvent.setup();
    await user.click(screen.getByLabelText("模型与思考强度"));
    // 选择器是两级菜单：先点开，再进「模型」这一层才是模型列表
    await user.click(screen.getByText("模型"));
    // 手动放开的那个也出现了（列表里会把 .gguf 后缀去掉）
    expect(screen.getAllByText("tg").length).toBeGreaterThan(0);
  }, 20000);

  /**
   * 快捷指令曾经完全无效：它被当成独立的 system 消息插在对话中间，
   * 而多数本地模型的对话模板只认开头那条 system，中途的会被忽略，
   * 于是"点按钮"和"不点"输出一模一样。
   *
   * 现在改成把指令并进用户消息。这条用例锁住"发给模型的文本里确实带上了指令"。
   */
  it("快捷指令会并进用户消息发给模型", async () => {
    chatMock.mockResolvedValue("润色后的结果");

    await renderReady();
    const user = userEvent.setup();
    // 先把文字打进输入框，再点指令，最后发送
    await user.type(screen.getByPlaceholderText("输入消息…"), "这段文字需要处理");
    await user.click(screen.getByText("润色"));
    expect(screen.getByText("下一句生效")).toBeInTheDocument();

    await user.click(screen.getByLabelText("发送"));
    await screen.findByText("润色后的结果");

    const calls = chatMock.mock.calls;
    const sent = calls[calls.length - 1]?.[0] as
      | { messages: { role: string; content: string }[] }
      | undefined;
    const last = sent?.messages[sent.messages.length - 1];
    // 指令与原文在同一条用户消息里
    expect(last?.role).toBe("user");
    expect(last?.content).toContain("润色");
    expect(last?.content).toContain("这段文字需要处理");
    // 不该再出现"中途插入的 system"
    expect(sent?.messages.some((m) => m.role === "system")).toBe(false);
  });

  /** 不用快捷指令时，消息原样发出，不该被拼上任何提示词。 */
  it("不选快捷指令就不拼接任何指令", async () => {
    chatMock.mockResolvedValue("普通回答");

    await renderReady();
    await sendText("普通提问");
    await screen.findByText("普通回答");

    const calls = chatMock.mock.calls;
    const sent = calls[calls.length - 1]?.[0] as
      | { messages: { role: string; content: string }[] }
      | undefined;
    const last = sent?.messages[sent.messages.length - 1];
    expect(last?.content).toBe("普通提问");
  });

  /** 「对照原文」已移除：对话里没有原文，这个功能只会让模型凭空编造。 */
  it("不再提供「对照原文」", async () => {
    await renderReady();
    expect(screen.queryByText("对照原文")).not.toBeInTheDocument();
  });

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

  /**
   * 历史超过 ctx 的 60% 时，发送前应先压缩；且压缩结果要写回会话，
   * 否则每发一句都会把同样几轮再压一遍，白烧模型调用。
   *
   * 用 512 的小 ctx 模型，200 字左右就能触发，避免在测试里敲长文本。
   */
  it("历史过长时先压缩，且压过之后不重复压缩", async () => {
    const smallCtx = vi.mocked(ipc.listLocalModels);
    smallCtx.mockResolvedValue([
      {
        id: "demo.gguf",
        path: "C:/models/demo.gguf",
        sizeMb: 512,
        ctxSize: 512, // 阈值 0.6×512 ≈ 307 token ≈ 614 字符
        trainedCtx: 4096,
        supportsThinking: false,
        thinking: "off" as const,
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
    ]);
    chatMock.mockResolvedValue("好的");

    await renderReady();

    // 需要超过 keepTurns(3) 轮才切得出可压缩片段，且要越过触发阈值。
    // 每轮用"发送按钮重新可用"作为完成信号，别用文本查找——
    // 多轮回复文本相同时会命中多个元素。
    const chunk = "字".repeat(250);
    for (let i = 0; i < 5; i++) {
      await sendText(`${i}${chunk}`);
      await waitFor(() =>
        expect(screen.getByLabelText("发送")).toBeEnabled(),
      );
    }

    const firstRound = compactMock.mock.calls.length;
    expect(firstRound).toBeGreaterThan(0);

    // 再发一句：上一轮已经压缩并写回，历史变短，不该再压
    await sendText("再来");
    await waitFor(() => expect(screen.getByLabelText("发送")).toBeEnabled());
    expect(compactMock.mock.calls.length).toBe(firstRound);
  }, 30000);

  /** 压缩失败不能挡住对话——退回让后端按轮次裁。 */
  it("压缩失败时对话照常继续", async () => {
    const smallCtx = vi.mocked(ipc.listLocalModels);
    smallCtx.mockResolvedValue([
      {
        id: "demo.gguf",
        path: "C:/models/demo.gguf",
        sizeMb: 512,
        ctxSize: 512,
        trainedCtx: 4096,
        supportsThinking: false,
        thinking: "off" as const,
        chatCapable: true,
        family: "通用",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
    ]);
    compactMock.mockRejectedValue(new Error("摘要请求失败"));
    chatMock.mockResolvedValue("照样能答");

    await renderReady();
    const chunk = "字".repeat(250);
    for (let i = 0; i < 3; i++) {
      await sendText(`${i}${chunk}`);
      await screen.findAllByText("照样能答");
    }

    expect(compactMock).toHaveBeenCalled();
    // 摘要挂了，但对话仍然正常完成
    expect(screen.getAllByText("照样能答").length).toBeGreaterThan(0);
  }, 30000);
});
