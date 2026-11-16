import { useCallback, useEffect, useState } from "react";
import {
  clearTranslateCache,
  getAppInfo,
  getCacheStatus,
  getRuntimeStatus,
  openDataSubdir,
  openEngineDir,
  type AppInfo,
  type CacheStatus,
  type RuntimeStatus,
} from "../lib/ipc";

/**
 * 「存储位置」区块：告诉用户各类文件到底在哪。
 *
 * 设计要点（前一版踩过的坑）：
 *
 * 1. **不要逐行重复根目录**。四条路径的公共前缀完全一样，各自 truncate 后
 *    长得一模一样，等于什么都没说。改成：根目录单独一行，下面是相对子项。
 * 2. **「引擎」指的是在跑的那个 llama-server**，不是 `<数据目录>/runtime`。
 *    这两者可以完全无关——用户手动指定 D 盘的引擎时，runtime 目录是空的。
 *    真正下载到本地时两者才重合，所以两个都显示，并注明区别。
 * 3. **模型目录不在数据目录里**（通常在别的盘），必须单独列，否则用户会
 *    以为模型也在这个根下面。
 */
const btnClass =
  "btn h-7 rounded-md border px-2 text-[11px] transition-colors disabled:opacity-50";

const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const mono = { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" };

/**
 * 首帧判平台的兜底：`AppInfo` 还没到的时候用。
 *
 * 只看是不是 Windows——Linux 与 macOS 在这个组件里显示方式相同
 * （都用正斜杠、都叫 `WebKitCache`），不需要再细分。
 */
function isWindowsUA(): boolean {
  return typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
}

export function StorageInfo({ refreshToken = 0 }: { refreshToken?: number }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await getAppInfo());
    } catch {
      // 拿不到就显示占位符，不影响其它设置项
    }
    try {
      setRuntime(await getRuntimeStatus());
    } catch {
      // 引擎未安装是正常状态
    }
  }, []);

  /**
   * `refreshToken` 变化时重取。
   *
   * 早先只在挂载时拉一次，于是在设置里换了引擎之后，这一块仍显示旧路径——
   * 恰恰是用户最需要它更新的时候。父组件每次切换引擎会 +1。
   */
  useEffect(() => {
    void refresh();
  }, [refresh, refreshToken]);

  /** 子项：只显示相对名字，完整路径放 title。 */
  const Sub = ({ label, name }: { label: string; name: string }) => (
    <div className="mt-1 flex items-baseline gap-2">
      <span
        className="w-[44px] shrink-0"
        style={{ color: "var(--text-muted)" }}
      >
        {label}
      </span>
      <span
        className="min-w-0 flex-1 truncate"
        style={{ ...mono, color: "var(--text)" }}
        title={info ? `${info.dataDir}${sep}${name}` : ""}
      >
        {name}
      </span>
    </div>
  );

  const enginePath = runtime?.installed ? runtime.serverPath : null;

  /*
   * 路径分隔符与 webview 缓存目录名都按平台取。
   *
   * 早先这两处都写死成 Windows 的：分隔符用反斜杠，缓存目录叫
   * `EBWebView`。Linux 上截图显示成 `EBWebView\` —— 而实际目录是
   * `WebKitCache`，用户照着这个名字去文件夹里找会找不到。
   *
   * `info` 还没到时用 `navigator.userAgent` 兜底：`info?.os` 为
   * undefined 会被当成"非 Windows"，于是 Windows 用户会先看到一帧
   * Linux 的写法再跳回来。首帧就判对，不需要猜。
   */
  const platform = info?.os ?? (isWindowsUA() ? "windows" : "other");
  const isWindows = platform === "windows";
  const sep = isWindows ? "\\" : "/";
  /** WebView2 在 Windows，WebKitGTK 在 Linux / macOS */
  const webviewCache = isWindows ? "EBWebView" : "WebKitCache";

  return (
    <div className="mb-3">
      <div className="section-title mb-1">存储位置</div>
      <div
        className="rounded-[10px] border p-2.5 text-[11px]"
        style={{
          background: "var(--surface-muted)",
          borderColor: "var(--border)",
        }}
      >
        {/* 数据根目录：单独一行，下面用相对名，避免四条一样的前缀 */}
        <div className="flex items-center gap-2">
          <span
            className="w-[44px] shrink-0"
            style={{ color: "var(--text-muted)" }}
          >
            数据
          </span>
          {/*
            模式标签。用户把 exe 单独拖到桌面后，会发现设置全没了，
            却不知道原因是「已经不在便携目录里了」。标出来就能自己判断。
          */}
          {info && (
            <span
              className="shrink-0 rounded px-1 text-[10px]"
              style={{
                background: info.portable
                  ? "var(--brand-soft)"
                  : "var(--surface)",
                color: info.portable ? "var(--brand-text)" : "var(--text-muted)",
                border: info.portable ? "none" : "1px solid var(--border)",
              }}
              title={
                info.portable
                  ? "数据存在程序目录下的 data/ 里，整个文件夹可以直接搬走"
                  : "数据存在系统标准位置（与安装版相同）"
              }
            >
              {info.portable ? "便携模式" : "标准模式"}
            </span>
          )}
          <span
            className="min-w-0 flex-1 truncate"
            style={{ ...mono, color: "var(--text)" }}
            title={info?.dataDir ?? ""}
          >
            {info?.dataDir ?? "—"}
          </span>
          <button
            className={btnClass}
            style={btnStyle}
            onClick={() => openDataSubdir("root")}
            title="删掉这个目录等于还原出厂设置"
          >
            打开
          </button>
        </div>

        <div
          className="mt-2 border-t pt-2"
          style={{ borderColor: "var(--border)" }}
        >
          <Sub label="配置" name="config.json" />
          <Sub label="会话" name={`sessions${sep}`} />
          <Sub label="日志" name={`logs${sep}`} />
          <Sub label="译文缓存" name={`cache${sep}`} />
          {/* 这条是 WebView2 自己的，与译文缓存无关，早先笼统叫「缓存」容易混 */}
          <Sub label="界面缓存" name={`${webviewCache}${sep}`} />
        </div>

        {/* 引擎：显示真正在运行的那个可执行文件，而不是存放目录 */}
        <div
          className="mt-2 border-t pt-2"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="flex items-center gap-2">
            <span
              className="w-[44px] shrink-0"
              style={{ color: "var(--text-muted)" }}
            >
              引擎
            </span>
            <span
              className="min-w-0 flex-1 truncate"
              style={{ ...mono, color: "var(--text)" }}
              title={enginePath ?? ""}
            >
              {enginePath ?? "未配置"}
            </span>
            <button
              className={btnClass}
              style={btnStyle}
              onClick={() => void openEngineDir()}
              title="打开当前引擎所在的目录"
            >
              打开
            </button>
          </div>
        </div>

        <p
          className="m-0 mt-2 border-t pt-2"
          style={{ color: "var(--text-muted)", borderColor: "var(--border)" }}
        >
          清除数据目录可恢复默认设置；模型目录由用户指定，不受影响。
        </p>
      </div>

      <TranslateCache refreshToken={refreshToken} />
    </div>
  );
}

/** 字节数转成人看的单位。缓存文件通常几百 KB，只在异常大时才到 MB。 */
function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * 「批量翻译缓存」区块：显示条数与占用，并提供清空。
 *
 * 为什么叫「批量」：**只有批量会落盘**。
 * 翻译页的临时小翻译只进内存，关掉程序就没了——
 * 那是有意的：用户用它做一次性小翻译，不该在磁盘上攒垃圾。
 *
 * 为什么值得暴露给用户：批量落盘后有一个新的失败模式——
 * 改了模型或觉得译文不对，但同样的原文会一直命中旧译文，
 * 用户会觉得「改了设置没效果」，而且从界面上看不出原因。
 * 给一个条数 + 一个清空按钮，这个问题就有了自助解法。
 *
 * 落盘不可用时（`persistent = false`）要明确说出来，否则用户
 * 以为「翻过的下次还在」，结果重启后从头再来。
 */
function TranslateCache({ refreshToken = 0 }: { refreshToken?: number }) {
  const [status, setStatus] = useState<CacheStatus | null>(null);
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await getCacheStatus());
      setError(null);
    } catch (e) {
      // 后端还没起来时拿不到，不阻塞其它设置项
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh, refreshToken]);

  const onClear = async () => {
    setClearing(true);
    setError(null);
    try {
      const removed = await clearTranslateCache();
      setCleared(removed);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setClearing(false);
    }
  };

  const entries = status?.entries ?? 0;
  const disabled = clearing || entries === 0;

  return (
    <div className="mt-3">
      {/*
        标题与说明分两行。
        早先两者放在一个 `flex items-baseline` 里，说明文字太长被挤，
        末尾的单字（"存"）掉到第二行，看起来像排版事故。
        标题是分组标识、说明是补充信息，本来就该分行。
      */}
      <div className="section-title mb-0.5">批量翻译缓存</div>
      <p
        className="m-0 mb-1.5 text-[10px] leading-relaxed"
        style={{ color: "var(--text-muted)" }}
      >
        批量翻过的段落重跑时直接复用；翻译页的临时译文不写入此处。
      </p>

      <div
        className="rounded-[10px] border p-2.5 text-[11px]"
        style={{
          background: "var(--surface-muted)",
          borderColor: "var(--border)",
        }}
      >
        <div className="flex items-center gap-2">
          <span style={{ color: "var(--text-muted)" }}>条数</span>
          <span style={{ ...mono, color: "var(--text)" }}>
            {status ? entries : "—"}
          </span>
          <span className="ml-2" style={{ color: "var(--text-muted)" }}>
            占用
          </span>
          <span style={{ ...mono, color: "var(--text)" }}>
            {status ? formatBytes(status.bytes) : "—"}
          </span>
          <button
            className={`${btnClass} ml-auto`}
            style={btnStyle}
            onClick={() => void onClear()}
            disabled={disabled}
            title={
              entries === 0
                ? "还没有缓存内容"
                : "清空后同样的原文会重新请求模型"
            }
          >
            {clearing ? "清理中…" : "清空"}
          </button>
        </div>

        {status && !status.persistent && (
          <p className="m-0 mt-1.5" style={{ color: "var(--danger)" }}>
            落盘不可用，缓存只在本次运行内有效（重启即失效）。
          </p>
        )}

        {cleared !== null && entries === 0 && (
          <p
            className="m-0 mt-1.5"
            style={{ color: "var(--brand-text)" }}
          >
            已清空 {cleared} 条。
          </p>
        )}

        {error && (
          <p className="m-0 mt-1.5" style={{ color: "var(--danger)" }}>
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
