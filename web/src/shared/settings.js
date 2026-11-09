const WEB_SETTINGS_KEY = "llm-loop.settings.web";
const ENGLISH_SETTINGS_KEY = "llm-loop.settings.english";

/** 返回 Web 全局默认设置，例如 `empty storage -> empty global prompt`。 */
export function defaultWebSettings() {
  return { systemPrompt: "" };
}

/** 读取 Web 全局设置，例如 `localStorage -> global system prompt`。 */
export function loadWebSettings() {
  try {
    return {
      ...defaultWebSettings(),
      ...JSON.parse(localStorage.getItem(WEB_SETTINGS_KEY) ?? "{}"),
    };
  } catch {
    return defaultWebSettings();
  }
}

/** 保存 Web 全局设置并通知页面，例如 `system prompt -> localStorage`。 */
export function saveWebSettings(settings) {
  localStorage.setItem(WEB_SETTINGS_KEY, JSON.stringify(settings));
  window.dispatchEvent(new CustomEvent("llm-loop:web-settings", { detail: settings }));
}

/** 把全局提示词固定放在模块提示词之前，例如 `global + module -> combined system`。 */
export function composeWebSystemPrompt(globalPrompt, modulePrompt) {
  return [globalPrompt, modulePrompt]
    .filter((prompt) => typeof prompt === "string" && prompt.trim())
    .join("\n\n");
}

/** 返回英语模块默认设置，例如 `empty storage -> hi~ prompts`。 */
export function defaultEnglishSettings() {
  return { understandPrompt: "hi~", mainPrompt: "hi~" };
}

/** 读取英语模块设置，例如 `localStorage -> prompts`。 */
export function loadEnglishSettings() {
  try {
    return {
      ...defaultEnglishSettings(),
      ...JSON.parse(localStorage.getItem(ENGLISH_SETTINGS_KEY) ?? "{}"),
    };
  } catch {
    return defaultEnglishSettings();
  }
}

/** 保存英语模块设置并通知当前页面，例如 `prompts -> localStorage`。 */
export function saveEnglishSettings(settings) {
  localStorage.setItem(ENGLISH_SETTINGS_KEY, JSON.stringify(settings));
  window.dispatchEvent(new CustomEvent("llm-loop:english-settings", { detail: settings }));
}
