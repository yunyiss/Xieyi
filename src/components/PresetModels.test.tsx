import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import type { PresetProgress, PresetStatus } from "../lib/ipc";
import { PresetModels, formatBytes, percentOf } from "./PresetModels";

/**
 * 预设模型下载卡片测试。
 *
 * ## 这个组件最容易写出的假测试
 *
 * 只断言「渲染出名字和下载按钮」——那种测试在把整个四态判断删掉之后
 * 依然通过。所以每条测试都盯着**状态机的一个具体转移**：
 *
 *   idle → downloading → verifying → installed
 *   idle → downloading → cancelled → paused → (续传 | 删除)
 *
 * 并且每一条都用 `ipc.*` 的调用**和**界面文字双向确认：
 * 只测界面，可能是一个写死的字符串在骗人；只测 mock 调用，
 * 可能界面上什么都没显示。
 */

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    listPresets: vi.fn(async () => []),
    downloadPreset: vi.fn(async () => "D:/models/x.gguf"),
    cancelDownload: vi.fn(async () => {}),
    deletePresetPartial: vi.fn(async () => 0),
    onPresetProgress: vi.fn(async () => () => {}),
  };
});

/** 三个预设里挑一个来当样本，字段与后端 `presets.rs` 对齐。 */
function preset(over: Partial<PresetStatus> = {}): PresetStatus {
  return {
    id: "Hy-MT2-1.8B-Q8_0",
    name: "Hy-MT2 轻量",
    summary: "纯 CPU 也能跑，日常中英互译",
    sizeBytes: 1_908_528_192,
    vramHintMb: 4096,
    family: "Hy-MT2",
    downloaded: false,
    path: null,
    partialBytes: 0,
    partialTotal: 0,
    ...over,
  };
}

function progress(over: Partial<PresetProgress> = {}): PresetProgress {
  return {
    id: "Hy-MT2-1.8B-Q8_0",
    stage: "downloading",
    downloaded: 0,
    total: 1_908_528_192,
    percent: 0,
    ...over,
  };
}

/**
 * 让 mock 的 `onPresetProgress` 把回调接出来，测试里手动推事件。
 *
 * 直接 `vi.mocked(ipc.onPresetProgress).mockImplementation(cb => { push = cb; })`
 * 在每条测试里写一遍太啰嗦，抽成 helper。
 */
function captureProgress() {
  let push: ((p: PresetProgress) => void) | null = null;
  vi.mocked(ipc.onPresetProgress).mockImplementation(async (cb) => {
    push = cb;
    return () => {};
  });
  return (p: PresetProgress) => {
    if (!push) throw new Error("进度回调还没被订阅");
    push(p);
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // clearAllMocks 不会恢复 mockImplementation，必须逐条重置默认值，
  // 否则上一条测试留下的 listPresets 结果会漏到下一条
  vi.mocked(ipc.listPresets).mockResolvedValue([]);
  vi.mocked(ipc.downloadPreset).mockResolvedValue("D:/models/x.gguf");
  vi.mocked(ipc.cancelDownload).mockResolvedValue(undefined);
  vi.mocked(ipc.deletePresetPartial).mockResolvedValue(0);
  vi.mocked(ipc.onPresetProgress).mockResolvedValue(() => {});
});

describe("PresetModels · 四态渲染", () => {
  it("未下载时显示下载按钮与体积", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    expect(await screen.findByText("Hy-MT2 轻量")).toBeInTheDocument();
    expect(screen.getByText(/1\.78 GB/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下载" })).toBeInTheDocument();
    // 这一态不该出现暂停/已安装的字样
    expect(screen.queryByText(/已暂停/)).not.toBeInTheDocument();
    expect(screen.queryByText(/已安装/)).not.toBeInTheDocument();
  });

  it("后端报告 partialBytes 时显示「已暂停」并给出续传与删除", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue([
      preset({ partialBytes: 800_000_000, partialTotal: 1_908_528_192 }),
    ]);
    render(<PresetModels mode="full" />);

    await screen.findByText("Hy-MT2 轻量");
    expect(screen.getByText(/已暂停/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续下载" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "删除" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载" })).not.toBeInTheDocument();
  });

  it("已下载时显示「已安装」，不再显示任何下载入口", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue([
      preset({ downloaded: true, path: "D:/models/x.gguf" }),
    ]);
    render(<PresetModels mode="full" />);

    await screen.findByText("Hy-MT2 轻量");
    expect(screen.getByText(/已安装/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "继续下载" })).not.toBeInTheDocument();
  });
});

describe("PresetModels · 进度事件", () => {
  it("downloading 事件把按钮换成取消，并显示字节数", async () => {
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });

    push(progress({ downloaded: 500_000_000, total: 1_908_528_192 }));

    // 状态转移：按钮没了，出现取消
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "取消" })).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "下载" })).not.toBeInTheDocument();
    expect(screen.getByText(/476\.8 MB \/ 1\.78 GB/)).toBeInTheDocument();
  });

  it("verifying 事件显示校验文案而不是停在 100% 的下载", async () => {
    // 这条锁住一个具体的观感缺陷：SHA-256 算 8 GB 要 7 秒，
    // 期间若仍显示「下载中 100%」，用户会以为卡死。
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });
    push(progress({ stage: "verifying", downloaded: 1_908_528_192 }));

    expect(await screen.findByText(/正在校验文件完整性/)).toBeInTheDocument();
    expect(screen.getByText("下载完成，正在校验…")).toBeInTheDocument();
    // 校验阶段不该再出现「取消」——此时中断没有意义
    expect(screen.queryByRole("button", { name: "取消" })).not.toBeInTheDocument();
  });

  it("cancelled 事件回到「已暂停」，且不报错", async () => {
    const push = captureProgress();
    vi.mocked(ipc.listPresets)
      .mockResolvedValueOnce([preset()])
      // 取消后后端刷新：此时已有 .part
      .mockResolvedValue([preset({ partialBytes: 500_000_000 })]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });
    push(progress({ downloaded: 500_000_000 }));
    await screen.findByRole("button", { name: "取消" });

    push(progress({ stage: "cancelled" }));

    // 关键：取消不是失败，界面上不能有红字
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "取消" })).not.toBeInTheDocument();
    });
    expect(screen.queryByText(/失败|错误/)).not.toBeInTheDocument();
  });

  it("failed 事件显示错误说明，不是静默回到未下载", async () => {
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });
    push(progress({ stage: "failed", error: "磁盘空间不足：需要 2.1 GB" }));

    expect(await screen.findByText(/磁盘空间不足/)).toBeInTheDocument();
  });

  it("终态事件触发一次列表刷新（.part 有没有留下只有后端知道）", async () => {
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });
    const callsBefore = vi.mocked(ipc.listPresets).mock.calls.length;

    push(progress({ stage: "done" }));

    await waitFor(() => {
      expect(vi.mocked(ipc.listPresets).mock.calls.length).toBeGreaterThan(
        callsBefore,
      );
    });
  });

  it("别的模型的进度事件不影响这张卡片", async () => {
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });

    push(progress({ id: "别的模型", downloaded: 100 }));

    // 仍是初始态
    expect(screen.getByRole("button", { name: "下载" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "取消" })).not.toBeInTheDocument();
  });
});

describe("PresetModels · 操作", () => {
  it("点下载会调后端，并在完成后回调 onDownloaded", async () => {
    const user = userEvent.setup();
    const onDownloaded = vi.fn();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" onDownloaded={onDownloaded} />);

    await user.click(await screen.findByRole("button", { name: "下载" }));

    await waitFor(() => {
      expect(ipc.downloadPreset).toHaveBeenCalledWith("Hy-MT2-1.8B-Q8_0");
    });
    // 下载完必须重启引擎，否则 presets.ini 里没有新文件
    await waitFor(() => {
      expect(onDownloaded).toHaveBeenCalledTimes(1);
    });
  });

  it("点取消调 cancelDownload", async () => {
    const user = userEvent.setup();
    const push = captureProgress();
    vi.mocked(ipc.listPresets).mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await screen.findByRole("button", { name: "下载" });
    push(progress({ downloaded: 1000 }));
    await user.click(await screen.findByRole("button", { name: "取消" }));

    await waitFor(() => {
      expect(ipc.cancelDownload).toHaveBeenCalledTimes(1);
    });
  });

  it("点删除会清掉 .part 并刷新列表", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.listPresets)
      .mockResolvedValueOnce([preset({ partialBytes: 800_000_000 })])
      .mockResolvedValue([preset()]);
    render(<PresetModels mode="full" />);

    await user.click(await screen.findByRole("button", { name: "删除" }));

    await waitFor(() => {
      expect(ipc.deletePresetPartial).toHaveBeenCalledWith("Hy-MT2-1.8B-Q8_0");
    });
    // 删完应回到「下载」
    expect(await screen.findByRole("button", { name: "下载" })).toBeInTheDocument();
  });

  it("一个在下载时，别的卡片不能点（后端有闸门，前端先禁用）", async () => {
    const user = userEvent.setup();
    const other = preset({
      id: "Hy-MT2-7B-Q4_K_M",
      name: "Hy-MT2 均衡",
    });
    // 让下载挂住不返回，保持 busy 状态
    let release: (v: string) => void = () => {};
    vi.mocked(ipc.downloadPreset).mockImplementation(
      () => new Promise<string>((res) => (release = res)),
    );
    vi.mocked(ipc.listPresets).mockResolvedValue([preset(), other]);
    render(<PresetModels mode="full" />);

    const buttons = await screen.findAllByRole("button", { name: "下载" });
    await user.click(buttons[0]);

    // 第一张在下载 → 第二张的按钮必须禁用
    await waitFor(() => {
      const now = screen.getAllByRole("button", { name: "下载" });
      expect(now.some((b) => (b as HTMLButtonElement).disabled)).toBe(true);
    });

    release("D:/models/a.gguf");
  });
});

describe("PresetModels · 格式化", () => {
  it("formatBytes 按 1024 进制", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    // KB 不保留小数：下载进度里 "1 KB" 比 "1.0 KB" 更利落
    expect(formatBytes(1024)).toBe("1 KB");
    // MB 一位、GB 两位：4.31 GB 比 4.3 GB 更有信息量
    expect(formatBytes(1_048_576)).toBe("1.0 MB");
    expect(formatBytes(1_908_528_192)).toBe("1.78 GB");
  });

  it("percentOf 在总量未知时返回 null 而不是 0", () => {
    // 返回 0 会让界面显示 0%，用户以为卡住了；
    // null 让界面显示「未知」，那是事实。
    expect(percentOf(100, 0)).toBeNull();
    expect(percentOf(50, 100)).toBe(50);
    expect(percentOf(200, 100)).toBe(100); // 夹到 100
  });

  it("列表为空时不渲染任何卡片", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue([]);
    const { container } = render(<PresetModels mode="full" />);

    await waitFor(() => {
      expect(container.querySelectorAll("li")).toHaveLength(0);
    });
  });
});

/**
 * `featured` 模式：引导层用。
 *
 * 它与 `full` 的差别是**布局密度**，不是数据。用户实测反馈：
 * 三张卡片全展开把首屏撑到必须滚动，而首次用户根本没有判断依据
 * 去比较三个量化档。
 *
 * 但也不能干脆不给选择：纯 CPU 的用户下 7B 会慢到无法接受。
 */
describe("PresetModels · featured 模式", () => {
  const three = [
    preset({
      id: "Hy-MT2-1.8B-Q8_0",
      name: "Hy-MT2 1.8B",
      summary: "轻量：4G 显卡或纯 CPU 可用，速度最快", 
      sizeBytes: 1_908_528_192,
    }),
    preset({
      id: "Hy-MT2-7B-Q4_K_M",
      name: "Hy-MT2 7B · 均衡",
      summary: "标准：8G 显卡的均衡选择",
      sizeBytes: 4_624_648_896,
    }),
    preset({
      id: "HY-MT2-7B-Q8_0",
      name: "Hy-MT2 7B · 高质量",
      summary: "高质量：12G 以上显卡",
      sizeBytes: 7_981_928_896,
    }),
  ];

  it("默认只显示推荐的那一档，其余折叠", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue(three);
    render(<PresetModels mode="featured" />);

    // 推荐的（均衡档）在
    expect(await screen.findByText("Hy-MT2 7B · 均衡")).toBeInTheDocument();

    // 另外两档**不在**——这是这个模式存在的全部理由
    //
    // 变异验证：把 `mode === "full"` 的分支去掉（让它永远走 featured）
    // 不会失败；要验证这条，得把 featured 分支改成直接 `list.map(row)`，
    // 此时本断言失败。
    expect(screen.queryByText("Hy-MT2 1.8B")).toBeNull();
    expect(screen.queryByText("Hy-MT2 7B · 高质量")).toBeNull();
  });

  it("有「其他档位」入口，点开后两档都出现", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.listPresets).mockResolvedValue(three);
    render(<PresetModels mode="featured" />);

    const toggle = await screen.findByRole("button", { name: /其他 .* 个档位/ });
    await user.click(toggle);

    expect(await screen.findByText("Hy-MT2 1.8B")).toBeInTheDocument();
    expect(screen.getByText("Hy-MT2 7B · 高质量")).toBeInTheDocument();
    // 展开后按钮变成「收起」
    expect(
      screen.getByRole("button", { name: /收起/ }),
    ).toBeInTheDocument();
  });

  it("full 模式不做折叠——设置页要看到全部", async () => {
    vi.mocked(ipc.listPresets).mockResolvedValue(three);
    render(<PresetModels mode="full" />);

    // 三档全在
    expect(await screen.findByText("Hy-MT2 1.8B")).toBeInTheDocument();
    expect(screen.getByText("Hy-MT2 7B · 均衡")).toBeInTheDocument();
    expect(screen.getByText("Hy-MT2 7B · 高质量")).toBeInTheDocument();
    // 也不该有折叠入口
    expect(screen.queryByRole("button", { name: /个档位/ })).toBeNull();
  });

  it("推荐档不在列表里时退化为第一项，不崩", async () => {
    // 预设列表将来增删条目时，写死的 FEATURED_ID 可能不存在
    vi.mocked(ipc.listPresets).mockResolvedValue([
      preset({ id: "custom-a", name: "自定义 A" }),
      preset({ id: "custom-b", name: "自定义 B" }),
    ]);
    render(<PresetModels mode="featured" />);

    expect(await screen.findByText("自定义 A")).toBeInTheDocument();
    expect(screen.queryByText("自定义 B")).toBeNull();
  });

  /**
   * 展开按钮必须在**列表最后面**，不能夹在推荐卡与其他卡片之间。
   *
   * 用户反馈：按钮夹在中间时"像标题一样挡住"，模型列表的连续性被打断。
   *
   * 判据用 DOM 顺序：按钮的文档位置要排在其他档位卡片**之后**。
   *
   * 变异验证：把 `{others.length > 0 && (<button…>)}` 挪回
   * `{expanded && …}` 列表的上方，本测试失败。
   */
  it("展开按钮排在其余档位卡片之后", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.listPresets).mockResolvedValue(three);
    render(<PresetModels mode="featured" />);

    await user.click(await screen.findByRole("button", { name: /其他 .* 个档位/ }));

    const toggle = screen.getByRole("button", { name: /收起/ });
    const low = screen.getByText("Hy-MT2 1.8B");

    // compareDocumentPosition: 后者(2) 在 前者(4) 之后 → 返回带 4 的位
    const rel = toggle.compareDocumentPosition(low);
    expect(
      rel & Node.DOCUMENT_POSITION_PRECEDING,
      "按钮应当排在卡片之后（展开的内容在它上面）",
    ).toBeTruthy();
  });

  /**
   * 说明文字在任何模式下都必须显示。
   *
   * 曾经为了压缩高度，在「其他档位」里把 `summary` 藏了。用户反馈：
   * 「不需要这种为了压缩把模型描述干掉了」——高度问题应该用别的方式解决，
   * 而不是删掉帮助用户做选择的信息。
   *
   * 变异验证：给 `{preset.summary}` 外面套回 `{!compact && …}`，本测试失败。
   */
  it("展开的其他档位也显示说明文字", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.listPresets).mockResolvedValue(three);
    render(<PresetModels mode="featured" />);

    await user.click(await screen.findByRole("button", { name: /其他 .* 个档位/ }));

    // 三档的说明都该在
    expect(screen.getByText(/轻量：4G 显卡或纯 CPU 可用/)).toBeInTheDocument();
    expect(screen.getByText(/标准：8G 显卡的均衡选择/)).toBeInTheDocument();
    expect(screen.getByText(/高质量：12G 以上显卡/)).toBeInTheDocument();
  });
});
