import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Tailwind 扫描范围的守卫。
 *
 * `src/index.css` 用了 `source(none)` + 显式 `@source`，只扫
 * `index.html` 与 `src/`。这样做能让 dev 冷启动从 47 秒降到 0.2 秒，
 * 但代价是：**放到 src 外面的界面文件不会生成样式，而且不会报错**。
 *
 * 这条测试扫描仓库（跳过 node_modules / vendor / docs / src-tauri 等
 * 大目录），一旦发现 `src/` 之外出现带 `className` 的界面文件就失败，
 * 并直接告诉你该补哪一条 `@source`。
 *
 * 变异验证：在仓库根目录新建一个含 `className="flex"` 的 .tsx，本测试失败。
 */

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** 不参与前端、也可能是巨型目录的，一律跳过。 */
const SKIP_DIRS = new Set([
  ".git",
  ".github",
  ".screenshots",
  ".testtmp",
  ".trae",
  ".vscode",
  "assets",
  "dist",
  "docs",
  "node_modules",
  "public",
  "src-tauri",
  "vendor",
]);

/** 可能承载界面代码的扩展名。 */
const UI_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".html", ".vue", ".svelte"]);

function extensionOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i < 0 ? "" : name.slice(i).toLowerCase();
}

/** 递归收集根目录下除 `src/` 之外的候选文件。 */
function collect(dir: string, out: string[]): void {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return; // 权限问题不该让测试炸掉；这些文件本来也不会被 Vite 打包
  }

  for (const entry of entries) {
    if (entry.isSymbolicLink()) continue;
    const full = join(dir, entry.name);

    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      // src 是允许的根，里面的 className 都合法，不必逐个读
      if (dir === ROOT && entry.name === "src") continue;
      collect(full, out);
      continue;
    }

    if (entry.isFile() && UI_EXTENSIONS.has(extensionOf(entry.name))) {
      out.push(full);
    }
  }
}

describe("Tailwind 扫描范围", () => {
  it("src 之外的界面文件必须显式加入 @source", () => {
    const candidates: string[] = [];
    collect(ROOT, candidates);

    const offenders: string[] = [];
    for (const file of candidates) {
      const rel = relative(ROOT, file).split(sep).join("/");
      // 根 index.html 是已声明 @source 的，允许
      if (rel === "index.html") continue;

      let text = "";
      try {
        text = readFileSync(file, "utf8");
      } catch {
        continue;
      }

      const looksLikeUi =
        /\bclassName\s*=/.test(text) || /\bclass\s*=/.test(text);
      if (looksLikeUi) offenders.push(rel);
    }

    if (offenders.length > 0) {
      throw new Error(
        [
          "发现 src/ 之外的界面文件，Tailwind 不会扫描它们（样式会静默丢失）：",
          ...offenders.map((f) => `  - ${f}`),
          "",
          "处理方式（二选一）：",
          "  1. 把界面代码移到 src/ 下；",
          "  2. 在 src/index.css 增加对应的 @source，例如：",
          '     @source "../packages/ui";',
        ].join("\n"),
      );
    }

    expect(offenders).toEqual([]);
  });
});
