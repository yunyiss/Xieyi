import { useCallback, useEffect, useState } from "react";
import {
  getAppInfo,
  getRuntimeStatus,
  openDataSubdir,
  openEngineDir,
  openModelsDir,
  type AppInfo,
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
        title={info ? `${info.dataDir}\\${name}` : ""}
      >
        {name}
      </span>
    </div>
  );

  const enginePath = runtime?.installed ? runtime.serverPath : null;

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
          <Sub label="会话" name={"sessions\\"} />
          <Sub label="缓存" name={"EBWebView\\"} />
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
          {/* 模型目录：通常不在数据目录里，单独一行 */}
          <div className="mt-2 flex items-center gap-2">
            <span
              className="w-[44px] shrink-0"
              style={{ color: "var(--text-muted)" }}
            >
              模型
            </span>
            <span
              className="min-w-0 flex-1 truncate"
              style={{ ...mono, color: "var(--text)" }}
              title={info?.modelsDir ?? ""}
            >
              {info?.modelsDir ?? "—"}
            </span>
            <button
              className={btnClass}
              style={btnStyle}
              onClick={() => void openModelsDir()}
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
    </div>
  );
}
