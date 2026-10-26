import type { TranslateStyle } from "./ipc";

export interface TranslateStyleOption {
  value: TranslateStyle;
  label: string;
  hint: string;
}

/**
 * 翻译风格预设。与后端 `prompts::Style` 一一对应。
 *
 * 只改 system prompt，不增加请求次数与输出额度，所以默认「通用」不会拖慢翻译；
 * 想更正式、保留术语或压字幕行宽时再切。
 */
export const TRANSLATE_STYLES: TranslateStyleOption[] = [
  { value: "general", label: "通用", hint: "日常文本" },
  { value: "formal", label: "正式", hint: "公文 · 商务" },
  { value: "technical", label: "技术", hint: "保留术语" },
  { value: "literary", label: "文学", hint: "保留语气" },
  { value: "subtitle", label: "字幕", hint: "短句 · 易读" },
];
