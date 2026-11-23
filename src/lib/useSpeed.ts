import { useEffect, useRef, useState } from "react";

export interface SpeedInfo {
  /** 平滑后的速度（字节/秒）；还没算出时为 null */
  bytesPerSec: number | null;
  /** 剩余秒数；总大小未知或速度为 0 时为 null */
  etaSecs: number | null;
}

/**
 * 从连续的进度事件估算下载速度与剩余时间。
 *
 * ## 为什么不把速度从后端传
 *
 * 后端只在写盘时回调进度，本来就有节流（约 250ms）。在这里用相邻两次
 * 回调的差值算速度，既不用改协议，也能天然适配「重试后从断点续」这种
 * 总进度会跳变的情况。
 *
 * ## 平滑
 *
 * 单次差值抖动很大（DNS、首包、代理抖动），所以做指数平滑
 * `0.7 旧值 + 0.3 新值`。第一次直接采用瞬时值，避免开头两秒一直空着。
 *
 * `active` 为 false（校验中/已暂停）时立即清空，避免显示一个已经过期的
 * 高速度。
 */
export function useSpeed(
  downloaded: number,
  total: number,
  active: boolean,
): SpeedInfo {
  const [info, setInfo] = useState<SpeedInfo>({
    bytesPerSec: null,
    etaSecs: null,
  });
  const last = useRef<{ t: number; bytes: number } | null>(null);

  useEffect(() => {
    if (!active) {
      last.current = null;
      setInfo({ bytesPerSec: null, etaSecs: null });
      return;
    }

    const now = performance.now();
    if (!last.current) {
      last.current = { t: now, bytes: downloaded };
      return;
    }

    const seconds = (now - last.current.t) / 1000;
    // 进度回调本身约 250ms 一次；间隔太短算出来全是噪声
    if (seconds < 0.4) return;

    const delta = downloaded - last.current.bytes;
    last.current = { t: now, bytes: downloaded };
    const instant = delta > 0 ? delta / seconds : 0;

    setInfo((prev) => {
      const speed =
        prev.bytesPerSec === null
          ? instant
          : prev.bytesPerSec * 0.7 + instant * 0.3;
      return {
        bytesPerSec: speed,
        etaSecs:
          speed > 1024 && total > downloaded
            ? (total - downloaded) / speed
            : null,
      };
    });
  }, [downloaded, total, active]);

  return info;
}

/** 287 KB/s / 1.2 MB/s；速度未知时返回空串，调用方直接拼进去即可。 */
export function formatSpeed(bytesPerSec: number | null): string {
  if (bytesPerSec === null || !Number.isFinite(bytesPerSec) || bytesPerSec <= 0) {
    return "";
  }
  if (bytesPerSec >= 1024 * 1024) {
    return `${(bytesPerSec / 1024 / 1024).toFixed(1)} MB/s`;
  }
  return `${Math.round(bytesPerSec / 1024)} KB/s`;
}

/**
 * 剩余时间。用「1 分 05 秒」而不是「65 秒」：下载动辄十几分钟，
 * 分钟读起来更快；不足一分钟显示秒。
 */
export function formatEta(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds <= 0) {
    return "";
  }
  const s = Math.round(seconds);
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  const rest = s % 60;
  return rest === 0 ? `${m} 分` : `${m} 分 ${rest} 秒`;
}
