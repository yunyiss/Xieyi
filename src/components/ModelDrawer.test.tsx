import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { ModelDrawer } from "./ModelDrawer";

/**
 * 设置抽屉组件测试。
 *
 * 这是本项目首次给 `ModelDrawer.tsx` 补测试 —— 它 912 行、
 * 负责读写用户配置，此前零测试（技术债 #8）。
 *
 * 为什么这里风险高：它改的是落盘的 `config.json`。
 * 逻辑写错不会崩溃，而是**静默写坏用户数据** ——
 * 比如 TTL 填 0 变成「永不卸载」、上下文设成越界值。
 */

/**
 * 文件对话框走系统插件，测试里换成可控的桩。
 */
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
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
      partialBytes: 0,
    })),
    getSettings: vi.fn(async () => ({
      ttlSecs: 120,
      gpuLayers: 1001,
      pinned: [],
      modelsDir: "D:/models",
      partialBytes: 0,
      running: true,
      port: 8080,
    })),
    listLocalModels: vi.fn(async () => [
      {
        id: "Hy-MT2-1.8B-Q8_0",
        path: "D:/models/Hy-MT2-1.8B-Q8_0.gguf",
        size: 1_908_528_192,
        contextSize: 4096,
        thinking: "off",
        family: "Hy-MT2",
        style: "chinese-official",
        chatCapable: true,
        chatOverride: null,
      },
    ]),
    listRouterModels: vi.fn(async () => []),
    getGlossary: vi.fn(async () => []),
    setGlossary: vi.fn(async () => {}),
    setTtl: vi.fn(async () => {}),
    setGpuLayers: vi.fn(async () => {}),
    setModelsDir: vi.fn(async () => "D:/models"),
    setPinned: vi.fn(async () => []),
    setModelParams: vi.fn(async () => ({
      ctx: 4096,
      thinking: "off" as const,
    })),
    setModelChatCapable: vi.fn(async () => {}),
    setManualRuntime: vi.fn(async () => ({})),
    discoverRuntimes: vi.fn(async () => []),
    probeVram: vi.fn(async () => null),
    openModelsDir: vi.fn(async () => "D:/models"),
    restartService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
      partialBytes: 0,
    })),
    abort: vi.fn(async () => {}),
    getCacheStatus: vi.fn(async () => ({
      entries: 5,
      bytes: 1024,
      persistent: true,
    })),
    clearTranslateCache: vi.fn(async () => 5),
    openDataSubdir: vi.fn(async () => ""),
    openEngineDir: vi.fn(async () => ""),
    // 设置里的「下载引擎」与「下载源」
    installRuntime: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    })),
    onRuntimeProgress: vi.fn(async () => () => {}),
    getDownloadSourceConfig: vi.fn(async (kind: "engine" | "model") => ({
      kind,
      preferred: null,
      customMirror: null,
    })),
    setDownloadSourceConfig: vi.fn(async () => {}),
    getAppInfo: vi.fn(async () => ({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/models",
      partialBytes: 0,
      runtimeVersion: "b11714",
    })),
    syncWindowTheme: vi.fn(async () => {}),
    // 推荐模型下载（`PresetModels` 子组件会调）
    listPresets: vi.fn(async () => []),
    downloadPreset: vi.fn(async () => "D:/models/x.gguf"),
    cancelDownload: vi.fn(async () => {}),
    deletePresetPartial: vi.fn(async () => 0),
    onPresetProgress: vi.fn(async () => () => {}),
  };
});

function renderDrawer(props: Partial<Parameters<typeof ModelDrawer>[0]> = {}) {
  return render(
    <ModelDrawer
      open
      onClose={() => {}}
      onChanged={() => {}}
      {...props}
    />,
  );
}

describe("ModelDrawer · 设置读写", () => {
  beforeEach(async () => {
    // **`clearAllMocks` 不会重置 `mockImplementation`。**
    //
    // 这条踩过一次：某条用例把 `getRuntimeStatus` 改成"永不 resolve"
    // 来模拟加载中，之后所有用例都拿不到引擎状态，连带三条测试失败。
    // 所以这里显式恢复本文件依赖的默认实现。
    vi.clearAllMocks();
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    });
  });

  it("打开时读取设置并显示 TTL", async () => {
    renderDrawer();
    await waitFor(() => {
      expect(ipc.getSettings).toHaveBeenCalled();
    });
    // 120 秒的 TTL 应该以某种形式出现在界面上
    await waitFor(() => {
      expect(screen.getByText(/120/)).toBeInTheDocument();
    });
  });

  it("设置里提供「下载引擎」入口，并能触发下载", async () => {
    // 这条锁住一个真实缺口：下载引擎**只在首启引导层有**，
    // 设置里一直是「选择文件」+「扫描已安装」两个按钮。
    // 用户删了引擎、或引导时选了「稍后配置」，就再也找不到下载入口，
    // 只能自己去 GitHub 翻文件——而 GitHub 在国内时通时不通。
    //
    // 默认 mock 里引擎是已装的，所以这里要走两步（先变确认态）。
    //
    // 变异验证：把 downloadEngine 那个按钮删掉 → 本条失败。
    renderDrawer();

    await userEvent.click(
      await screen.findByRole("button", { name: /重新下载/ }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: /确认重装/ }),
    );

    await waitFor(() => {
      expect(ipc.installRuntime).toHaveBeenCalled();
    });
    // **必须带 force=true**，否则后端那道「已装过就别再下」的闸门会
    // 直接拒绝，这个按钮就变成永远失败的摆设。
    expect(ipc.installRuntime).toHaveBeenCalledWith("vulkan", true);
  });

  /**
   * 引擎状态还没读到时，**不能先画出结论**。
   *
   * 用户实测：首次打开设置，引擎区先是一块深色主按钮（「下载引擎」），
   * 几十毫秒后才变成浅色的「重新下载」。虽然快到肉眼看不清，但深色块
   * 闪一下是能感觉到的。
   *
   * 根子不是配色，是**在知道答案之前就画了答案**：`runtime` 还是 null，
   * `installed` 被当成 false。只把深色改成浅色是治标——深浅仍会变。
   *
   * 这条测试卡在第一帧：`getRuntimeStatus` 永不 resolve，此时界面上
   * 既不该出现「下载引擎」（错的结论），也不该出现「重新下载」（对的
   * 结论但还不知道），只该是一句「正在读取…」。
   *
   * 变异验证：把 `runtimeLoaded ? … : 占位` 的守卫去掉，直接按
   * `runtime?.installed` 渲染，本测试失败。
   */
  it("引擎状态未就绪时不渲染下载或重下按钮", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockImplementation(
      () => new Promise(() => {}), // 永不返回，模拟"数据还在路上"
    );

    renderDrawer();

    // 停在「通用」页即可，引擎区块默认就显示在这里
    expect(await screen.findByText(/正在读取/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /下载引擎/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /重新下载/ })).toBeNull();
  });

  /**
   * 已装过引擎时，「重新下载」需要**两步确认**——但不是系统弹窗。
   *
   * 用户实测：装好之后点它会一声不响重下 31 MB，连点几次白下上百 MB。
   * 第一版用系统确认框（`plugin-dialog` 的 `ask`），被否掉了：
   * 系统弹窗由 OS 绘制，与整套自绘 UI 不搭，而且是模态的、会打断操作。
   *
   * 现在改成按钮自身两步：第一次点变成「确认重装？31 MB」，再点才真下。
   *
   * 变异验证：把 `if (runtime?.installed && !confirmReinstall)` 那段
   * 整块删掉，本测试失败（第一下就直接调后端了）。
   */
  it("已装引擎时第一下只变文字，不发起下载", async () => {
    renderDrawer(); // 默认 mock 里 installed = true

    await userEvent.click(
      await screen.findByRole("button", { name: /重新下载/ }),
    );

    // 第一下：按钮变成待确认态，且**没有**调后端
    expect(
      await screen.findByRole("button", { name: /确认重装/ }),
    ).toBeInTheDocument();
    expect(ipc.installRuntime).not.toHaveBeenCalled();
  });

  /** 第二下才真正开始下载。 */
  it("再点一次「确认重装」才真正下载", async () => {
    renderDrawer();

    await userEvent.click(
      await screen.findByRole("button", { name: /重新下载/ }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: /确认重装/ }),
    );

    await waitFor(() => {
      expect(ipc.installRuntime).toHaveBeenCalledWith("vulkan", true);
    });
  });

  /**
   * 未装引擎时**不用**确认——首下意图明确，多一步只是碍事。
   *
   * 变异验证：把确认条件里的 `runtime?.installed` 去掉（无条件确认），
   * 本测试失败。
   */
  it("未装引擎时点一下就直接下载", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
      partialBytes: 0,
    });

    renderDrawer();

    await userEvent.click(
      await screen.findByRole("button", { name: /下载引擎/ }),
    );

    await waitFor(() => {
      expect(ipc.installRuntime).toHaveBeenCalledTimes(1);
    });
  });

  /**
   * 待确认态**不能让按钮变宽**。
   *
   * 用户反馈的视觉问题："确认重装？31 MB" 比 "重新下载" 长，按钮一变宽
   * 就把旁边那个「扫描本机」挤到第二行，整块跳了一下。
   *
   * 修法不是调宽度，而是让两段文案**字数相同**（都是 4 个汉字），
   * 体积信息挪到下面那行说明里——位置和高度都不动。
   *
   * 这条测试直接量字宽：两段文案的字符数必须相等。
   *
   * 变异验证：把文案改回含 "31 MB" 的长版本，本测试失败。
   */
  it("待确认的按钮文案与「重新下载」等长，不会撑宽布局", async () => {
    renderDrawer();

    const before = await screen.findByRole("button", { name: /重新下载/ });
    const widthBefore = before.textContent?.length ?? 0;

    await userEvent.click(before);
    const after = await screen.findByRole("button", { name: /确认重装/ });

    expect(after.textContent?.length).toBe(widthBefore);
  });

  /**
   * 报错必须做成**浮条**，不能插在滚动内容里。
   *
   * 用户实测：解压失败时，错误原本渲染在内容末尾，面板滚到别处就看不见了
   * （「解压失败: i/o error」就是这么被错过的）。而且它出现/消失时还会把
   * 下面的元素推来推去。
   *
   * 判据：报错节点要 `position: absolute`——脱离文档流，不受滚动影响。
   *
   * 变异验证：改回在内容流里渲染 `<p>`，本测试失败。
   */
  it("报错以浮条呈现（绝对定位，不随滚动）", async () => {
    vi.mocked(ipc.installRuntime).mockRejectedValue("解压失败: i/o error");

    renderDrawer();
    await userEvent.click(
      await screen.findByRole("button", { name: /重新下载/ }),
    );
    await userEvent.click(screen.getByRole("button", { name: /确认重装/ }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/解压失败/);

    // 承载它的外层必须是绝对定位
    const floating = alert.parentElement as HTMLElement;
    expect(floating.className).toMatch(/\babsolute\b/);
  });

  /** 浮条能手动关掉。 */
  it("报错浮条可以关闭", async () => {
    vi.mocked(ipc.installRuntime).mockRejectedValue("磁盘空间不足");

    renderDrawer();
    await userEvent.click(
      await screen.findByRole("button", { name: /重新下载/ }),
    );
    await userEvent.click(screen.getByRole("button", { name: /确认重装/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/磁盘空间不足/);

    await userEvent.click(screen.getByRole("button", { name: "关闭提示" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("下载源是独立页签，引擎与模型各一份可分别配置", async () => {
    // 下载源是**横切关注点**：同时管引擎（GitHub）与模型（魔搭/HF），
    // 所以单独一页，不塞进「通用」或「模型与术语」。
    //
    // 早先两页各放一份，用户反馈说"放一起更贴切"——现在集中在一页里，
    // 两类各自带偏好选择，互不干扰（后端配置键也分开存）。
    renderDrawer();

    // 「通用」页不该出现下载源
    expect(screen.queryByText(/从 GitHub 下载/)).toBeNull();

    // 切到「下载源」页签
    await userEvent.click(screen.getByText("下载源"));

    // 两类都在，且标题与说明能让人分清哪块管什么。
    //
    // 早先两块都叫「下载源」，用户根本分不清。现在标题按用途命名，
    // 并各带一行"引擎下载源 / 模型下载源"的副标题。
    await waitFor(() => {
      expect(screen.getByText(/从 GitHub 下载/)).toBeInTheDocument();
    });
    expect(screen.getByText(/优先从魔搭下载/)).toBeInTheDocument();
    // 副标题区分用途
    expect(screen.getByText("引擎下载源")).toBeInTheDocument();
    expect(screen.getByText("模型下载源")).toBeInTheDocument();
  });

  it("模型页也能手选源——海外用户可能访问不了魔搭", async () => {
    // 默认首选魔搭（国内快），但海外用户可能连不上。
    // 留一个手选 HuggingFace 的出口，比让人卡住强。
    renderDrawer();
    await userEvent.click(screen.getByText("下载源"));

    await waitFor(() => {
      expect(ipc.getDownloadSourceConfig).toHaveBeenCalledWith("model");
    });
    // 引擎那份也要读
    expect(ipc.getDownloadSourceConfig).toHaveBeenCalledWith("engine");
  });

  it("关闭状态不渲染内容", async () => {
    renderDrawer({ open: false });
    // 抽屉 unmount 后不应有可交互的设置项
    await waitFor(() => {
      expect(screen.queryByText(/通用/)).toBeNull();
    });
  });

  /**
   * 设置抽屉里「推荐模型」**默认收起**。
   *
   * 用户反馈：「设置里的推荐模型下载可以完全折叠收起来不，因为下载完
   * 之后就不需要再下载了，堆一起占空位」。
   *
   * 与引导层的区别：引导层是第一次面对我没有模型，必须直接摊开；
   * 设置抽屉是回过头的用户，多半只想改个参数。
   *
   * 变异验证：把  改成 ，本测试失败。
   */
  it("推荐模型默认收起，点标题才展开", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.listPresets).mockResolvedValue([
      {
        id: "Hy-MT2-1.8B-Q8_0",
        name: "Hy-MT2 1.8B · 轻量",
        summary: "轻量：4G 显卡或纯 CPU 可用",
        sizeBytes: 1_908_528_192,
        vramHintMb: 4096,
        family: "Hy-MT2",
        downloaded: false,
        path: null,
        partialBytes: 0,
        partialTotal: 0,
      },
    ]);

    renderDrawer();
    await user.click(await screen.findByRole("tab", { name: /模型与术语/ }));

    // 收起状态下：标题在，卡片**不可见**
    //
    // 不能断言 queryByText 为 null——组件是始终挂载、靠 display:none
    // 隐藏的（否则它没法上报"还有几个没下"）。jsdom 里 display:none
    // 不影响文本查询，只能查可见性。
    expect(await screen.findByText(/推荐模型/)).toBeInTheDocument();
    expect(screen.getByText(/Hy-MT2 1.8B · 轻量/)).not.toBeVisible();
    // 且透出还有几个没下
    expect(await screen.findByText(/1 个未下载/)).toBeInTheDocument();

    // 点标题展开
    await user.click(screen.getByRole("button", { name: /推荐模型/ }));
    await waitFor(() => {
      expect(screen.getByText(/Hy-MT2 1.8B · 轻量/)).toBeVisible();
    });
  });

  it("模型列表在「模型与术语」页显示模型 id", async () => {
    const user = userEvent.setup();
    renderDrawer();

    // 模型列表不在默认的「通用」页，要先切过去
    await user.click(await screen.findByRole("tab", { name: /模型与术语/ }));

    await waitFor(() => {
      expect(screen.getByText(/Hy-MT2-1.8B-Q8_0/)).toBeInTheDocument();
    });
  });

  it("保存术语表会把解析后的条目交给后端", async () => {
    const user = userEvent.setup();
    renderDrawer();

    // 切到「模型与术语」页
    const tab = await screen.findByRole("tab", { name: /模型与术语/ });
    await user.click(tab);

    const box = await screen.findByPlaceholderText(/API=接口/);
    await user.clear(box);
    await user.type(box, "API=接口\nLLM=大语言模型");

    const save = screen.getByRole("button", { name: /保存术语表/ });
    await user.click(save);

    await waitFor(() => {
      expect(ipc.setGlossary).toHaveBeenCalledWith([
        { from: "API", to: "接口" },
        { from: "LLM", to: "大语言模型" },
      ]);
    });
  });

  it("术语表空行被忽略，不会产生空条目", async () => {
    const user = userEvent.setup();
    renderDrawer();

    const tab = await screen.findByRole("tab", { name: /模型与术语/ });
    await user.click(tab);
    const box = await screen.findByPlaceholderText(/API=接口/);
    await user.clear(box);
    // 故意混入：空行、只有左边、只有右边、没有等号的。
    // 这几种都必须被拒绝 —— 写进配置会让术语表出现无从匹配的条目。
    await user.type(box, "A=B\n\n\n=只有右边\n只有左边=\n没有等号\nC=D");

    await user.click(screen.getByRole("button", { name: /保存术语表/ }));

    await waitFor(() => {
      expect(ipc.setGlossary).toHaveBeenCalledWith([
        { from: "A", to: "B" },
        { from: "C", to: "D" },
      ]);
    });
  });
});
