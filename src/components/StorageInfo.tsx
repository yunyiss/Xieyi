import { useCallback, useEffect, useState } from "react";
import { getAppInfo, openDataSubdir, type AppInfo } from "../lib/ipc";

/**
 * 「存储位置」区块：告诉用户各类文件到底在哪。
 *
 * 单独成组件而不是塞进设置抽屉的 JSX：抽屉里已经有引擎、模型、术语三块，
 * 再往里加会让本就深的嵌套更难维护；这里只关心路径展示，逻辑独立。
 *
 * 只给「需要打开」的项配按钮：
 * - 配置和缓存是内部文件，用户不必进去翻
 * - 会话历史在应用内就能看
 * - 只有引擎目录有"我想自己看看/替换"的真实需求
 */
const btnClass =
  "btn h-7 rounded-md border px-2 text-[11px] transition-colors disabled:opacity-50";

const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

export function StorageInfo() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await getAppInfo());
    } catch {
      // 拿不到就显示占位符，不影响其它设置项
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 路径可能很长，截断显示，完整值放 title。 */
  const Row = ({
    label,
    sub,
    onOpen,
  }: {
    label: string;
    sub: string;
    onOpen?: () => void;
  }) => {
    const full = info ? `${info.dataDir}\\${sub}` : "";
    return (
      <div className="mt-1.5 flex items-center gap-2 first:mt-0">
        <span
          className="w-[44px] shrink-0"
          style={{ color: "var(--text-muted)" }}
        >
          {label}
        </span>
        <span
          className="min-w-0 flex-1 truncate"
          style={{ color: "var(--text)" }}
          title={full}
        >
          {full || "—"}
        </span>
        {onOpen && (
          <button className={btnClass} style={btnStyle} onClick={onOpen}>
            打开
          </button>
        )}
      </div>
    );
  };

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
        <Row label="配置" sub="config.json" />
        <Row label="引擎" sub="runtime" onOpen={() => openDataSubdir("runtime")} />
        <Row label="会话" sub="sessions" />
        <Row label="缓存" sub="EBWebView" />

        <div
          className="mt-2 border-t pt-2"
          style={{ borderColor: "var(--border)" }}
        >
          <button
            className={btnClass}
            style={btnStyle}
            onClick={() => openDataSubdir("root")}
            title="打开这个目录；删掉它等于还原出厂设置"
          >
            打开数据目录
          </button>
          <span className="ml-2" style={{ color: "var(--text-muted)" }}>
            以上文件都在此目录内，删除即还原
          </span>
        </div>
      </div>
    </div>
  );
}
