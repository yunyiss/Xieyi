import { describe, expect, it } from "vitest";
import { TRANSLATE_STYLES } from "./translateStyles";

describe("TRANSLATE_STYLES", () => {
  it("顺序与后端 prompts::Style 一致", () => {
    expect(TRANSLATE_STYLES.map((s) => s.value)).toEqual([
      "general",
      "formal",
      "technical",
      "literary",
      "subtitle",
    ]);
  });

  it("每一项都有可读的文案与右侧提示", () => {
    for (const style of TRANSLATE_STYLES) {
      expect(style.label.length).toBeGreaterThan(0);
      expect(style.hint.length).toBeGreaterThan(0);
    }
  });
});
