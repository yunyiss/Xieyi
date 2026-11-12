import { StrictMode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import * as ipc from "../lib/ipc";
import { Onboarding } from "./Onboarding";

/**
 * 首启引导层测试。
 *
 * 它是新用户唯一必经之路：这里卡住，用户看到的就是一片空白，
 * 会直接放弃。所以重点测「能不能顺利走完」，而不是下载细节。
 *
 * 不测的：真实下载/校验/解压（那是后端职责，已有单测）、
 * 进度条百分比动画（纯展示）。
 */

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    getAppInfo: vi.fn(async () => ({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/models",
      modelsDirConfigured: false,
      runtimeVersion: null,
      portable: false,
    })),
    getRuntimeStatus: vi.fn(async () => ({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    })),
    discoverRuntimes: vi.fn(async () => []),
    listRouterModels: vi.fn(async () => []),
    installRuntime: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    })),
    setManualRuntime: vi.fn(async () => ({
      installed: true,
      version: "manual",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    })),
    setModelsDir: vi.fn(async () => "D:/models"),
    restartService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
    })),
    onRuntimeProgress: vi.fn(async () => () => {}),
  };
});

// 文件选择对话框走系统插件，测试里换成可控的桩，
// 好模拟「用户选了一个目录」这个动作。
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => "D:/NewModelDir"),
}));

function renderOnboarding(onDone = vi.fn(), onChanged = vi.fn()) {
  render(<Onboarding onDone={onDone} onChanged={onChanged} />);
  return { onDone, onChanged };
}

/**
 * 在 **StrictMode** 下渲染。
 *
 * 主程序入口（`main.tsx`）就是包在 StrictMode 里的，而 `render()` 默认
 * 不包。于是有些「只在严格模式下才暴露」的 bug 测不出来——比如
 * 挂载/卸载/再挂载时 ref 没被重置。
 *
 * https://react.dev/reference/react/StrictMode
 */
function renderOnboardingStrict(onDone = vi.fn(), onChanged = vi.fn()) {
  render(
    <StrictMode>
      <Onboarding onDone={onDone} onChanged={onChanged} />
    </StrictMode>,
  );
  return { onDone, onChanged };
}

/**
 * 窗口高度自适应用的类名必须在 CSS 里有定义。
 *
 * 这种“类名拼错”不会报错，只会让自适应静默失效：
 * 小窗口下依然溢出，但没人知道是哪个类没生效。
 * 这里做静态对账：JSX 用到的必须在 CSS 里存在。
 */
/**
 * 窗口高度自适应用的类名必须在 CSS 里有定义。
 *
 * 这种“类名拼错”不会报错，只会让自适应静默失效：
 * 小窗口下依然溢出，但没人知道是哪个类没生效。
 */
describe("Onboarding · 自适应类名一致性", () => {
  it("媒体查询覆盖了 JSX 用到的所有 onboarding-* 类", async () => {
    const { ONBOARDING_CLASSES } = await import("./onboardingClasses");
    // 直接读 CSS 源文件做静态对账：类名拼错不会报错，
    // 只会让自适应静默失效，这里把它变成测试失败。
    const css = readFileSync("src/index.css", "utf-8");

    expect(ONBOARDING_CLASSES.length).toBeGreaterThan(0);

    // 精确匹配选择器：.onboarding-titleX 不该被当成 .onboarding-title 存在
    const missing = ONBOARDING_CLASSES.filter(
      (c) => !new RegExp("\\." + c + "(?![-a-zA-Z0-9])").test(css),
    );
    expect(
      missing,
      `这些类在 CSS 里没定义：${missing.join(", ")}`,
    ).toEqual([]);
  });

  it("CSS 里定义的每个 onboarding-* 类都真的被 JSX 用到了", async () => {
    // 反方向对账。上面那条只保证「清单里的类在 CSS 中存在」，
    // 抓不出「CSS 里写了样式、但 JSX 压根没用这个类名」。
    //
    // 这个漏洞真实存在过：`.onboarding-subtitle` 在媒体查询里定义了
    // 小窗口下的字号与间距，但副标题那个 <p> 从来没挂过这个类名。
    // 于是「小窗口收紧副标题」这个功能**从上线起就没生效过**，
    // 而当时所有测试都是绿的。
    //
    // 现在两边都查：JSX 用到的要有 CSS（上面），CSS 写了的要被用到（这里）。
    const css = readFileSync("src/index.css", "utf-8");
    const jsx = readFileSync("src/components/Onboarding.tsx", "utf-8");

    const defined = new Set<string>();
    for (const m of css.matchAll(/\.(onboarding-[a-z0-9-]+)/g)) {
      defined.add(m[1]);
    }
    expect(defined.size).toBeGreaterThan(0);

    const unused = [...defined].filter(
      (c) => !new RegExp("\\b" + c + "\\b").test(jsx),
    );
    expect(
      unused,
      `CSS 里定义了但 JSX 从未使用，说明自适应样式是死的：${unused.join(", ")}`,
    ).toEqual([]);
  });
});

describe("Onboarding", () => {
  beforeEach(() => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    });
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([]);
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);
  });

  it("渲染引导层且不阻塞（能拿到「稍后配置」）", async () => {
    renderOnboarding();
    // 新用户最需要的逃生口：不想现在配也能进主界面
    await waitFor(() => {
      expect(screen.getByText(/稍后配置/)).toBeInTheDocument();
    });
  });

  it("点「稍后配置」会关闭引导层", async () => {
    const user = userEvent.setup();
    const { onDone } = renderOnboarding();

    const skip = await screen.findByText(/稍后配置/);
    await user.click(skip);

    expect(onDone).toHaveBeenCalled();
  });

  it("发现本机已有引擎时列出来供选择", async () => {
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([
      {
        path: "D:/llama-b11714/llama-server.exe",
        label: "llama-b11714-bin-win-cuda-13.4-x64",
        backend: "cuda",
        version: "b11714",
      },
    ]);
    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByText(/llama-b11714/)).toBeInTheDocument();
    });
  });

  it("选中已有引擎后调用 setManualRuntime 而不是下载", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([
      {
        path: "D:/llama-b11714/llama-server.exe",
        label: "llama-b11714-bin-win-cuda-13.4-x64",
        backend: "cuda",
        version: "b11714",
      },
    ]);
    const { onChanged } = renderOnboarding();

    const item = await screen.findByText(/llama-b11714/);
    await user.click(item);

    await waitFor(() => {
      expect(ipc.setManualRuntime).toHaveBeenCalledWith(
        "D:/llama-b11714/llama-server.exe",
      );
    });
    // 关键：有现成引擎就不该去下载 30 MB
    expect(ipc.installRuntime).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalled();
  });

  it("没有候选引擎时提供下载入口", async () => {
    renderOnboarding();
    await waitFor(() => {
      expect(screen.getByText(/下载推理引擎/)).toBeInTheDocument();
    });
  });

  it("点下载会调用 installRuntime", async () => {
    const user = userEvent.setup();
    renderOnboarding();

    const dl = await screen.findByText(/下载推理引擎/);
    await user.click(dl);

    await waitFor(() => {
      expect(ipc.installRuntime).toHaveBeenCalled();
    });
  });

  it("引擎就绪但目录里没有模型时，仍然可以进主界面（按钮写「开始使用」）", async () => {
    // 这条锁住一个反复走过弯路的判定：
    //
    // 第一代：`Boolean(runtime?.modelsDir)` 判「选过目录」——后端总返回
    //         默认路径，恒为 true，于是引擎一装好就「完成」。
    // 第二代（矫枉过正）：只看「有模型」——把「想先进界面看看」的用户
    //         挡在门外，形成「不放模型就进不去、进不去就看不到模型目录
    //         在哪」的死循环。
    //
    // 现在分清两件事：
    //   canEnter = 引擎就绪         → 能不能进门（本条断言）
    //   ready    = 引擎 && 有模型    → 能不能翻译（页面里自己提示）
    //
    // 模型目录本来就是允许空的，用户完全可能稍后再配。
    //
    // 变异验证：把 allReady 改成 readiness.ready（要求有模型）→ 本条失败。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    // 用户确实设过目录（但目录是空的）
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/Model/translate",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    });
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);

    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /开始使用/ })).toBeInTheDocument();
    });
    // 但要如实说出缺什么，不能让用户进去以后才发现。
    // 提示在标题右侧（与「已发现 N 个」同一个位置），不再另起一行。
    expect(screen.getByText("没有 .gguf 文件")).toBeInTheDocument();
  });

  it("引擎没装时不给进门（按钮写「稍后配置」）", async () => {
    // 另一侧的反向约束：没有引擎，翻译和下载都无从谈起，
    // 这时候放进门只会让人一脸茫然地看到一个不能用的界面。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    });
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);

    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /稍后配置/ })).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: /开始使用/ })).toBeNull();
  });

  it("引擎就绪且扫到模型时，按钮写「开始使用」", async () => {
    // 补上正面场景：上面那条只管「不该放行」，这条保证「该放行时放行」。
    // 两条一起才能约束住判定，只测一边的话把 ready 恒为 false 也能过。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/Model/translate",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    });
    vi.mocked(ipc.listRouterModels).mockResolvedValue([
      { id: "Hy-MT2-1.8B-Q8_0", state: "unloaded" },
    ]);

    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /开始使用/ })).toBeInTheDocument();
    });
  });

  it("扫描期间不显示「正在扫描」，扫完直接出结果", async () => {
    // 「正在扫描…」曾经存在，现已删除。
    //
    // 原因：它覆盖的是 `listRouterModels`，而那只是一次**本地 HTTP
    // 查询**（llama-server 的 /models，几十毫秒）。用户根本来不及看见，
    // 闪一下反而像界面在抖。
    //
    // 真正需要提示的是「重启引擎」——实测十几秒（见日志
    // 01:30:27 → 01:30:40），那条提示单独测（见下一条）。
    //
    // 这条测试用来锁住「别再把它加回来」：扫描挂起期间不应该出现
    // 任何扫描态的文案，扫完直接显示结果。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    let release: (v: never[]) => void = () => {};
    vi.mocked(ipc.listRouterModels).mockImplementation(
      () => new Promise((res) => { release = res as never; }),
    );

    renderOnboarding();

    // 等引擎状态落地（此时扫描已挂起）
    await screen.findByText(/b11714/);
    expect(screen.queryByText(/正在扫描/)).toBeNull();

    // 扫完仍不该出现扫描态文案，只有结果
    release([{ id: "m1", state: "unloaded" } as never]);
    await waitFor(() => {
      expect(screen.getByText(/已发现 1 个/)).toBeInTheDocument();
    });
    expect(screen.queryByText(/正在扫描/)).toBeNull();
  });

  it("重启引擎期间按钮禁用并说明在等什么", async () => {
    // 这是整个引导里唯一会让用户干等十几秒的操作。
    // 没有反馈时表现是「点了按钮什么都没发生」，用户会反复再点，
    // 叠出多个重启请求。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    });
    // 让重启挂住，好在"进行中"这一刻做断言
    let finish: () => void = () => {};
    vi.mocked(ipc.restartService).mockImplementation(
      () =>
        new Promise((res) => {
          finish = () =>
            res({ running: true, port: 1, loaded: [], modelsDir: "D:/NewModelDir" });
        }),
    );

    renderOnboarding();
    const btn = await screen.findByRole("button", { name: /更换目录|选择模型目录/ });
    await userEvent.click(btn);

    // 进行中：按钮禁用 + 文案变成进度说明
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /正在重启引擎/ }),
      ).toBeDisabled();
    });
    expect(screen.getByText(/正在让引擎切到新目录/)).toBeInTheDocument();

    // 放行后恢复可点
    finish();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /更换目录/ })).toBeEnabled();
    });
  });

  it("引擎少于等于 7 个时不限高（不出现滚动条）", async () => {
    // 多数用户只有 1-3 个引擎，滚动条纯属噪音
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue(
      Array.from({ length: 3 }, (_, i) => ({
        path: `D:/llama-${i}/llama-server.exe`,
        label: `engine-${i}`,
        backend: "cuda",
        version: "b11714",
      })),
    );
    renderOnboarding();

    const list = await screen.findByRole("list");
    expect(list.style.maxHeight).toBe("");
  });

  it("引擎超过 7 个时限高滚动", async () => {
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue(
      Array.from({ length: 9 }, (_, i) => ({
        path: `D:/llama-${i}/llama-server.exe`,
        label: `engine-${i}`,
        backend: "cuda",
        version: "b11714",
      })),
    );
    renderOnboarding();

    const list = await screen.findByRole("list");
    await waitFor(() => {
      expect(list.style.maxHeight).not.toBe("");
    });
    expect(list.style.overflowY).toBe("auto");
  });

  it("模型状态提示只占标题栏一处，不再在卡片底部另起一行", async () => {
    // 回归锁：早先「没找到 .gguf」是卡片底部独立的一行 <p>，
    // 会把卡片硬顶高一行；而「已发现 N 个」在标题旁已经有位置了。
    // 同一件事占两处、其中一处还撑变形布局，是小窗口下最显眼的丑。
    //
    // 现在三种状态（扫描中 / 已发现 / 没有文件）共用标题右侧一个位置。
    //
    // 变异验证：把底部那个 <p> 加回来 → 「只有一个匹配」的断言失败。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/empty",
    });
    vi.mocked(ipc.getAppInfo).mockResolvedValue({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/Model/empty",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    });
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);

    renderOnboarding();

    // 提示必须存在，且**全页只出现一次**
    const hits = await screen.findAllByText("没有 .gguf 文件");
    expect(hits).toHaveLength(1);

    // 它应该待在标题所在的那一行里，而不是卡片底部独立的段落。
    // 标题那一行是 flex 容器，靠 `ml-auto` 把提示推到右侧。
    const line = hits[0].parentElement;
    expect(line?.className).toContain("ml-auto");
  });

  it("有模型时，标题旁显示「已发现 N 个」", async () => {
    // 与上一条配对：同一位置在「有模型」时显示数量。
    // 两条一起才锁得住「同一处按状态互斥」这个设计。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    vi.mocked(ipc.listRouterModels).mockResolvedValue([
      { id: "Hy-MT2-1.8B-Q8_0", state: "unloaded" },
      { id: "Hy-MT2-7B-Q4_K_M", state: "unloaded" },
      { id: "Index-Translate-9B", state: "unloaded" },
      { id: "translategemma-4b-it", state: "unloaded" },
      { id: "translategemma-12b-it", state: "unloaded" },
    ]);

    renderOnboarding();

    expect(await screen.findByText("已发现 5 个")).toBeInTheDocument();
    // 有模型时不该再出现「没有文件」
    expect(screen.queryByText("没有 .gguf 文件")).toBeNull();
  });

  it("已装引擎时不再重复安装", async () => {
    // 二次打开引导层（或用户手动重开）时，不该再触发一次下载
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    });
    renderOnboarding();

    await waitFor(() => {
      expect(ipc.getRuntimeStatus).toHaveBeenCalled();
    });
    expect(ipc.installRuntime).not.toHaveBeenCalled();
  });

  it("选完模型目录后必须重启引擎，否则列表永远扫旧目录", async () => {
    // ── 这条是真实用户踩到的 bug ──────────────────────────────
    //
    // 症状：在引导层选了模型目录，路径不生效，界面一直停在「稍后配置」。
    //
    // 根因：`--models-dir` 是 llama-server 的**启动参数**，进程活着的
    // 期间始终指向启动那一刻的目录。pickDir 只调 setModelsDir 写配置、
    // 没调 restartService，于是：
    //   1. config.json 确实改了，设置里也能看到新路径
    //   2. 但引擎还在扫旧目录 → listRouterModels 返回旧结果（空）
    //   3. modelReady 恒为 false → 一直「稍后配置」
    //
    // 设置抽屉里的同一个操作（ModelDrawer.changeDir）一直是带 restart
    // 的，只有引导层漏了。两处实现不一致，bug 就藏在这种地方。
    //
    // 变异验证：把 pickDir 里的 restartService() 去掉 → 本条失败。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    });
    renderOnboarding();

    // 等引导层拿到引擎状态、把「选择模型目录…」渲染出来
    const btn = await screen.findByRole("button", { name: /更换目录|选择模型目录/ });
    await userEvent.click(btn);

    await waitFor(() => {
      expect(ipc.setModelsDir).toHaveBeenCalledWith("D:/NewModelDir");
    });
    // 关键断言：改完目录必须重启引擎
    await waitFor(() => {
      expect(ipc.restartService).toHaveBeenCalled();
    });
  });

  it("选完模型目录后，路径显示立刻更新（不用等重启引擎）", async () => {
    // ── 真实用户报的第二个症状 ────────────────────────────────
    //
    // 症状：选完目录，路径框还是显示旧的默认路径，看着像没选中。
    //
    // 两个原因叠加：
    //   1. 路径框读的是 `runtime.modelsDir`，要等 getRuntimeStatus
    //      往返 + 引擎重启（1-3 秒）才更新，中间一直显示旧值
    //   2. （更严重）当时的 aliveRef 在 StrictMode 下恒为 false，
    //      refresh() 每个分支都提前 return，状态**永远**不更新
    //
    // 现在路径单独存一份，选完就乐观更新；后端值回来后再校正。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    });
    renderOnboarding();

    // 初始显示后端给的默认路径
    await screen.findByText("D:/models");

    const btn = await screen.findByRole("button", { name: /更换目录|选择模型目录/ });

    // 关键：让桩跟着真实后端的行为走。
    // 真实情况下 setModelsDir 之后，getRuntimeStatus 必然返回新路径；
    // 如果桩一直返回旧值，refresh() 轮询回来就会把显示「校正」回旧路径，
    // 测到的就不是我们要验的东西了。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/NewModelDir",
    });

    await userEvent.click(btn);

    // 选完立刻显示新路径（对话框桩返回 D:/NewModelDir）
    await waitFor(() => {
      expect(screen.getByText("D:/NewModelDir")).toBeInTheDocument();
    });
  });

  it("StrictMode 下挂载两次后，状态仍然能正常刷新", async () => {
    // ── 这条专抓 StrictMode 的坑 ──────────────────────────────
    //
    // 主入口 main.tsx 用 <StrictMode> 包着，它会故意「挂载 → 卸载 →
    // 再挂载」一遍。如果组件用 `const aliveRef = useRef(true)` 加一个
    // 只写 false 的清理函数：
    //
    //   首次挂载 → ref = true
    //   模拟卸载 → 清理函数置 false
    //   再次挂载 → ref 仍是 false（useRef 初值只在首次生效）
    //
    // 结果 refresh() 里所有 `if (!aliveRef.current) return` 全提前返回，
    // 界面永远停在初始状态——正是「选了目录路径不变」的根因。
    //
    // 变异验证：删掉 useEffect 里的 `aliveRef.current = true` → 本条失败。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/StrictModels",
    });
    renderOnboardingStrict();

    // 只要状态能落地，就说明 refresh 没被 aliveRef 短路
    await waitFor(() => {
      expect(screen.getByText("D:/StrictModels")).toBeInTheDocument();
    });
    // 引擎信息也要能显示出来（engineReady 来自同一份 runtime）
    expect(screen.getByText(/b11714/)).toBeInTheDocument();
  });
});
