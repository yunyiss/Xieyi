/**
 * 翻译方向的语言表。翻译页与批量页共用一份，避免两边不一致。
 *
 * 中文拆成简体/繁体两个条目：后端 `prompts::language_name_en` 会把
 * `zh` 写成 `Chinese (Simplified)`、`zh-tw` 写成 `Chinese (Traditional)`。
 * 早先只给一个 `中文`，模型拿到的目标语言是模糊的 `zh`，
 * 实测偶尔会输出繁体。
 */
export interface LanguageOption {
  value: string;
  label: string;
}

/** 原文语言：多一个「自动检测」。 */
export const SOURCE_LANGUAGES: LanguageOption[] = [
  { value: "auto", label: "自动检测" },
  { value: "zh", label: "简体中文" },
  { value: "zh-tw", label: "繁體中文" },
  { value: "en", label: "英语" },
  { value: "ja", label: "日语" },
  { value: "ko", label: "韩语" },
  { value: "fr", label: "法语" },
  { value: "de", label: "德语" },
  { value: "es", label: "西班牙语" },
  { value: "ru", label: "俄语" },
  { value: "pt", label: "葡萄牙语" },
  { value: "it", label: "意大利语" },
  { value: "ar", label: "阿拉伯语" },
  { value: "th", label: "泰语" },
  { value: "vi", label: "越南语" },
];

/** 译文语言：没有「自动检测」，其余同上。 */
export const TARGET_LANGUAGES: LanguageOption[] = SOURCE_LANGUAGES.filter(
  (l) => l.value !== "auto",
);
