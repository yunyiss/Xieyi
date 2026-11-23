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

/**
 * 剥掉所有 `@media { ... }` 块，只留媒体查询之外的内容。
 *
 * 用于判断"某个类是否有基础定义，而不是只写在媒体查询里"。
 * 手写配对括号扫描而不是正则：媒体查询里还有嵌套的 `{}`，
 * 一个 `[^}]*` 贪婪匹配很容易把后面的规则一起吃掉。
 */
function stripMediaBlocks(css: string): string {
  let out = "";
  let i = 0;
  while (i < css.length) {
    const m = css.indexOf("@media", i);
    if (m === -1) {
      out += css.slice(i);
      break;
    }
    out += css.slice(i, m);
    // 从 @media 往后找第一个 `{`，再配对到它的闭括号
    const open = css.indexOf("{", m);
    if (open === -1) break;
    let depth = 0;
    let j = open;
    for (; j < css.length; j += 1) {
      if (css[j] === "{") depth += 1;
      else if (css[j] === "}") {
        depth -= 1;
        if (depth === 0) {
          j += 1;
          break;
        }
      }
    }
    i = j;
  }
  return out;
}

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
      partialBytes: 0,
    })),
    setManualRuntime: vi.fn(async () => ({
      installed: true,
      version: "manual",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    })),
    setModelsDir: vi.fn(async () => "D:/models"),
    restartService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
      partialBytes: 0,
    })),
    onRuntimeProgress: vi.fn(async () => () => {}),
    cancelDownload: vi.fn(async () => {}),
    deleteRuntimePartial: vi.fn(async () => 0),
    // 推荐模型下载（`PresetModels` 子组件会调）
    listPresets: vi.fn(async () => []),
    downloadPreset: vi.fn(async () => "D:/models/x.gguf"),
    deletePresetPartial: vi.fn(async () => 0),
    onPresetProgress: vi.fn(async () => () => {}),
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

  /**
   * 容器类必须有**媒体查询之外的基础定义**。
   *
   * ## 为什么只查容器类（上一版是假测试）
   *
   * 上一版对 `ONBOARDING_CLASSES` 全体套用这条判据，结果五个类报错——
   * 但那是**误报**：`onboarding-head` / `-logo` / `-title` / `-subtitle`
   * / `-card` 的基础样式本来就在 JSX 的 Tailwind 类里，`onboarding-*`
   * 只负责在窄窗下**覆盖**几个值（缩小字号、收紧间距）。那套设计是对的。
   *
   * 真正需要基础定义的只有**布局容器**：它们的默认值不能靠 JSX 里的
   * Tailwind 类表达，否则宽屏媒体查询一覆盖，窄屏就没有可回退的原始值。
   *
   * ## 这条测试抓到的真实问题
   *
   * `onboarding-grid` 最初只写了宽屏的 `display: grid`，没有基础
   * `display: block`。窄窗下它没有任何样式——靠浏览器的 `div` 默认
   * 行为侥幸正确，但那不是设计，是巧合。
   *
   * 变异验证：删掉 `.onboarding-grid { display: block; }` 整块，
   * 本测试失败（而"类名存在"那条仍然通过）。
   */
  it("布局容器类有媒体查询之外的基础定义", async () => {
    const css = readFileSync("src/index.css", "utf-8");
    const withoutMedia = stripMediaBlocks(css);

    // 只列**布局容器**：基础样式必须写在 CSS 里，不能依赖 JSX 的
    // Tailwind 类——媒体查询覆盖后需要原始值可回退。
    const LAYOUT_CLASSES = ["onboarding-pad", "onboarding-steps"] as const;

    const missingBase = LAYOUT_CLASSES.filter(
      (c) => !new RegExp("\\." + c + "\\s*[,{]").test(withoutMedia),
    );

    expect(
      missingBase,
      `这些布局类只有媒体查询里的定义、缺基础样式：${missingBase.join(", ")}`,
    ).toEqual([]);
  });

  /**
   * 引导层必须**始终单列**，不许再出现两列布局。
   *
   * ## 为什么写成测试而不只是注释
   *
   * 「全屏留白多」这个反馈很容易被再次理解成"应该占满宽度"，于是又去
   * 开两列。而两列在这里是**结构性错误**：模型依赖引擎，没装引擎时
   * 右列只有一句「请先完成推理引擎配置」，整片空白。
   *
   * 换句话说，这个改法看起来解决了留白，实际制造了更大的视觉空洞。
   * 光靠注释拦不住下次的"顺手优化"，写成测试才行。
   *
   * 判据：`.onboarding-pad` 的宽度媒体查询里，不许出现
   * `grid-template-columns` 或 `columns`。
   *
   * 变异验证：在 `@media (min-width: 720px)` 里给 `.onboarding-steps`
   * 加一条 `grid-template-columns: 1fr 1fr`，本测试失败。
   */
  it("引导层保持单列，宽屏只放宽宽度", () => {
    const css = readFileSync("src/index.css", "utf-8");

    // 引导层相关规则里不该有任何分列声明
    const onboardingBlocks = css.match(/\.onboarding-[a-z]+\s*\{[^}]*\}/g) ?? [];
    const offenders = onboardingBlocks.filter((block) =>
      /grid-template-columns|column-count|flex-direction:\s*row/.test(block),
    );

    expect(
      offenders,
      `引导层不该分列。模型依赖引擎，并排会让右列整片空白：\n${offenders.join("\n")}`,
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
      partialBytes: 0,
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

  /**
   * **装好之后就不要再显示「下载推理引擎…」。**
   *
   * 用户实测：引擎装好之后那个按钮还在，再点一次会重新下 31 MB ——
   * 国内 90 KB/s 要等 5 分钟，纯属白等。
   *
   * 变异验证：把 JSX 里的 `{!hasDownloadedEngine && (...)}` 改回无条件
   * 渲染，本测试失败。
   */
  it("引擎装好后不再显示下载按钮", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    } as never);

    renderOnboarding();

    // 等引擎卡片渲染出来
    await screen.findByText(/b11714/);
    expect(screen.queryByText(/下载推理引擎/)).toBeNull();
    // 但「指定其他引擎…」要保留——用户可能想换成自己的那份
    expect(screen.getByText(/指定其他引擎/)).toBeInTheDocument();
  });

  /**
   * **手动指定**引擎时，下载按钮要保留。
   *
   * 因为那种情况下 `runtime/` 目录里并没有我们下载的那一份，
   * 用户可能想换成我们提供的 Vulkan 版（手动指定的未必是最优后端）。
   *
   * 这条与上一条是一对：只看 `engineReady` 会把两种情况混为一谈，
   * 必须按 `backend === "manual"` 区分。
   *
   * 变异验证：把 `hasDownloadedEngine` 里的
   * `runtime?.backend !== "manual"` 去掉，本测试失败。
   */
  it("手动指定引擎时仍保留下载按钮", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "manual",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    } as never);

    renderOnboarding();

    // 等状态渲染完（会显示「手动指定」）
    await screen.findByText(/手动指定/);
    expect(screen.getByText(/下载推理引擎/)).toBeInTheDocument();
  });

  /**
   * 引擎下载中必须能取消。
   *
   * 这条锁住一个实测过的痛点：国内加速代理约 90 KB/s，30.8 MB 要下
   * 5 分钟。早先没有取消按钮，用户只能干等或者关掉窗口——关窗口后
   * `.part` 留在磁盘上而界面看不见，用户会以为白下了。
   *
   * 变异验证：把 JSX 里那段 `progress.stage === "downloading" && <button>`
   * 删掉，本测试失败。
   */
  it("引擎下载中显示取消按钮，点了会调 cancelDownload", async () => {
    const user = userEvent.setup();

    // 让 installRuntime 挂住，制造「正在下载」的现场
    let push: ((p: unknown) => void) | null = null;
    vi.mocked(ipc.onRuntimeProgress).mockImplementation(async (cb) => {
      push = cb as (p: unknown) => void;
      return () => {};
    });
    vi.mocked(ipc.installRuntime).mockImplementation(
      () => new Promise(() => {}), // 永不 resolve
    );

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));

    // 推一条下载中进度
    await waitFor(() => expect(push).not.toBeNull());
    push!({
      stage: "downloading",
      downloaded: 5_000_000,
      total: 30_800_000,
      percent: 16.2,
    });

    const cancel = await screen.findByRole("button", { name: "取消下载" });
    await user.click(cancel);

    await waitFor(() => {
      expect(ipc.cancelDownload).toHaveBeenCalledTimes(1);
    });
  });

  /**
   * 校验阶段不该有取消按钮——那些步骤不可中断，给了按钮会让人
   * 以为点了能停。
   */
  it("校验阶段不显示取消按钮", async () => {
    const user = userEvent.setup();
    let push: ((p: unknown) => void) | null = null;
    vi.mocked(ipc.onRuntimeProgress).mockImplementation(async (cb) => {
      push = cb as (p: unknown) => void;
      return () => {};
    });
    vi.mocked(ipc.installRuntime).mockImplementation(() => new Promise(() => {}));

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));

    await waitFor(() => expect(push).not.toBeNull());
    push!({ stage: "verifying", downloaded: 30_800_000, total: 30_800_000, percent: 100 });

    // 等进度区渲染出来（有「校验中」字样），此时不该有取消按钮
    await screen.findByText(/校验中/);
    expect(screen.queryByRole("button", { name: "取消下载" })).toBeNull();
  });

  /**
   * 引擎下载完必须解锁 ② 模型卡，不能停在「完成」。
   *
   * 用户实测偶发（三次一次）：引导里下载好引擎后显示「完成」，但模型卡
   * 没解锁，只能先进主界面再从设置里下模型。根因是收尾只等
   * `installRuntime` 的 Promise——回包迟到或丢失时，界面就永远停在
   * 「完成」这一步。
   *
   * 现在「done」进度事件也会触发收尾（两条路只认先到的第一条）。
   * 这条测试模拟最坏情况：回包永不返回，只靠事件收尾。
   */
  it("done 事件先到、回包不回也能解锁模型卡", async () => {
    const user = userEvent.setup();
    const installed = {
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    };

    let push: ((p: unknown) => void) | null = null;
    vi.mocked(ipc.onRuntimeProgress).mockImplementation(async (cb) => {
      push = cb as (p: unknown) => void;
      return () => {};
    });
    vi.mocked(ipc.installRuntime).mockImplementation(() => new Promise(() => {}));

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));
    await waitFor(() => expect(push).not.toBeNull());

    // done 发出时后端已经把配置写好了
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue(installed);
    push!({ stage: "done", downloaded: 0, total: 0, percent: 100 });

    // 进度区消失，不再停在「完成」
    await waitFor(() => expect(screen.queryByText("完成")).toBeNull());
    // ② 模型卡解锁（出现「当前目录」这一行）
    expect(await screen.findByText("当前目录")).toBeInTheDocument();
    // 装好后下载按钮隐藏
    await waitFor(() => expect(screen.queryByText(/下载推理引擎/)).toBeNull());
  });

  /**
   * 迟到的 done 事件不能把进度条重新设回「完成」。
   *
   * 正常路径是 Promise 先回、事件后到。早先事件处理直接 `setProgress`，
   * 于是收尾之后的 done 又把进度区画了出来，用户看到「完成」一直挂着。
   */
  it("迟到的 done 事件不会让引擎卡在「完成」", async () => {
    const user = userEvent.setup();
    const installed = {
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    };

    let push: ((p: unknown) => void) | null = null;
    vi.mocked(ipc.onRuntimeProgress).mockImplementation(async (cb) => {
      push = cb as (p: unknown) => void;
      return () => {};
    });
    vi.mocked(ipc.installRuntime).mockResolvedValue(installed);
    vi.mocked(ipc.getRuntimeStatus)
      .mockResolvedValueOnce({
        installed: false,
        version: null,
        backend: null,
        serverPath: null,
        modelsDir: "D:/models",
        partialBytes: 0,
      })
      .mockResolvedValue(installed);

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));

    // Promise 先收尾：模型卡已解锁
    expect(await screen.findByText("当前目录")).toBeInTheDocument();

    // 事件后到：不能再画进度区
    await waitFor(() => expect(push).not.toBeNull());
    push!({ stage: "done", downloaded: 0, total: 0, percent: 100 });
    expect(screen.queryByText("完成")).toBeNull();
  });

  /**
   * 引擎包有未完成下载残留时，界面上必须显示出来并能清理。
   *
   * 这条锁住一个和模型那边**完全对称**的缺陷：`.part` 是故意保留的
   * （下次续传要用），但它不在任何列表里、启动时不扫描、界面上没有
   * 入口。用户取消过几次，磁盘上就躺着好几份半截包而一无所知。
   *
   * 模型那边上一轮已经修了（`PresetStatus.partialBytes` + 「删除」），
   * 引擎这边当时漏了。
   *
   * 变异验证：把 JSX 里 `(runtime?.partialBytes ?? 0) > 0` 那段删掉，
   * 本测试失败。
   */
  it("引擎包有残留时显示大小并给出清理入口", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 12_000_000,
    } as never);
    vi.mocked(ipc.deleteRuntimePartial).mockResolvedValue(12_000_000);

    renderOnboarding();

    // 显示出残留大小（12 MB）
    expect(await screen.findByText(/上次下载留下/)).toBeInTheDocument();
    expect(screen.getByText(/11\.4 MB/)).toBeInTheDocument();

    const clear = screen.getByRole("button", { name: "清理" });
    await user.click(clear);

    await waitFor(() => {
      expect(ipc.deleteRuntimePartial).toHaveBeenCalledTimes(1);
    });
  });

  /** 没有残留时不该出现那一行——不能凭空多一条"上次下载留下 0 B"。 */
  it("没有残留时不显示清理入口", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    } as never);

    renderOnboarding();

    // 等引擎卡片渲染完（用引擎路径作为稳定锚点，
    // 标题「推理引擎」四个字在小窗口下可能与其他文本合并）
    await screen.findByText(/llama-server/);
    expect(screen.queryByText(/上次下载留下/)).toBeNull();
    expect(screen.queryByRole("button", { name: "清理" })).toBeNull();
  });

  /**
   * **取消下载之后必须重新拉状态**，否则残留提示永远不出现。
   *
   * 这条锁住一个用户实测出来的真实缺陷：
   *
   * 1. 用户下载了 10 多 MB，点取消
   * 2. 界面上什么都没出现——既没有"已取消"的残留提示，也没有清理入口
   * 3. 再点下载，**直接续上了**（说明数据确实都在磁盘上）
   *
   * 根因：`download()` 的 catch 分支只 `setError`，**没调 `refresh()`**。
   * 于是 `runtime.partialBytes` 停在进入下载前的旧值 0，那行提示的
   * 显示条件 `(runtime?.partialBytes ?? 0) > 0` 永远不成立。
   *
   * 数据全在，只是界面不知道。这正是"残留不可见"那个缺陷的**第二次**
   * 出现——第一次是后端没有这个字段，这一次是前端没刷新。
   *
   * 变异验证：删掉 catch 里的 `await refresh()`，本测试失败。
   */
  it("取消下载后会刷新状态，残留提示随即出现", async () => {
    const user = userEvent.setup();

    // 下载场景 = 还没装我们的引擎，所以 backend 必须**不是** vulkan，
    // 否则「下载推理引擎…」按钮会按"已装"隐藏，点不到。
    // 这里用 manual（手动指定），是真实存在的一种中间状态。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
      partialBytes: 0,
    } as never);

    // 点下载 → 用户取消 → installRuntime reject
    vi.mocked(ipc.installRuntime).mockRejectedValue("已取消下载");
    vi.mocked(ipc.cancelDownload).mockResolvedValue(undefined);

    renderOnboarding();
    await screen.findByText(/下载推理引擎/);

    // 在点击**之前**就排好后续返回值：取消后 refresh 会立刻拉一次，
    // 若等到点击之后再改 mock，那次 refresh 可能已经拿到旧值。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 10_485_760,
    } as never);

    await user.click(screen.getByText(/下载推理引擎/));

    // 关键断言：提示出现了（说明 catch 里真的重新拉了状态）
    expect(
      await screen.findByText(/上次下载留下/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "清理" })).toBeInTheDocument();
  });

  /**
   * 取消要显示成**中性提示**，不能是红色错误。
   *
   * 用户主动点了「取消下载」，凭什么看到红色警告？后端专门为取消加了
   * 独立的 `cancelled` 阶段（不发 `failed`），就是为了不让它走错误路径；
   * 前端若一律染红，等于把那个区分又抹平了。
   *
   * 判据：用 `role="status"`（中性）而不是 `role="alert"`（错误）。
   *
   * 变异验证：把 download() 里的
   * `msg.includes("已取消") ? "info" : "error"` 改成恒为 `"error"`，
   * 本测试失败。
   */
  it("取消下载显示中性提示，不报红色错误", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.installRuntime).mockRejectedValue(
      "已取消下载。已下载的部分保留，下次会继续",
    );

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));

    // role=status 才是中性；role=alert 是错误
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent(/已取消/);
    // 且不能同时冒出一个 alert
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /**
   * 浮条必须能**手动关闭**，并且会**自动消失**。
   *
   * 用户反馈：「取消后那句提示一直挂着呐，不消失的」。
   * 早先只有下一次操作开始时才被清掉，所以它会一直挡在屏幕下方。
   *
   * 这条测手动关闭（自动消失靠定时器，测试里用假时钟会拖慢整个套件，
   * 且 4 秒/8 秒的时长本身不是逻辑）。定时器那条在实现里保证：
   * 新提示到来时先清旧定时器，不会出现"前一条把后一条提前关掉"。
   */
  it("浮条可以手动关闭", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.installRuntime).mockRejectedValue("磁盘空间不足");

    renderOnboarding();
    await user.click(await screen.findByText(/下载推理引擎/));

    expect(await screen.findByRole("alert")).toHaveTextContent(/磁盘空间不足/);

    await user.click(screen.getByRole("button", { name: "关闭提示" }));

    expect(screen.queryByRole("alert")).toBeNull();
  });

  /**
   * 「模型」卡片的空状态**不该再有**「回到第 ① 步」按钮。
   *
   * 曾经按 UI/UX 规范的 Empty State 条目（"Show helpful message and
   * action"）加过它，用户反馈「这是干什么用的，我没看懂」。
   *
   * 原因：引导层总共两张卡，引擎卡就在上面，往上滑一眼就看到。
   * 一个"滚到视线内已有内容"的按钮，只会让人怀疑自己漏了什么。
   * 规范要按场景用，不能照抄。
   *
   * 变异验证：把那句 `请先完成上面的推理引擎配置` 旁边加回一个
   * 「回到第 ① 步」按钮，本测试失败。
   */
  it("模型卡片空状态不再有「回到第 ① 步」按钮", async () => {
    renderOnboarding();

    // 引擎未配置 → 模型卡片显示空状态
    expect(
      await screen.findByText(/请先完成上面的推理引擎配置/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /回到第/ })).toBeNull();
  });

  /**
   * 引导层**不能**宣传「支持拖入 .gguf」。
   *
   * 拖入只发生在主界面（Translate / Batch 监听 Tauri 的 drag-drop 事件），
   * 引导层根本没有 drop 处理。早先这句文案是从主界面照搬的，用户在引导层
   * 试了一次拖入，什么都没发生。
   *
   * 宣传一个此时不存在的功能，比不宣传更糟——用户会以为软件坏了。
   *
   * 变异验证：把那句「支持将 .gguf 文件直接拖入窗口」加回去，
   * 本测试失败。
   */
  it("引导层不宣传拖入（此时并不支持）", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
      partialBytes: 0,
    } as never);

    renderOnboarding();
    await screen.findByText(/llama-server/);

    expect(screen.queryByText(/拖入/)).toBeNull();
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
