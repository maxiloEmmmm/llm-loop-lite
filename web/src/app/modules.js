import { lazy } from "react";

/** 动态加载英语模块，例如 `open english -> import EnglishPage chunk`。 */
const loadEnglish = () => import("../modules/english/EnglishPage.jsx");

/** 动态加载设置模块，例如 `open settings -> import SettingsPage chunk`。 */
const loadSettings = () => import("../modules/settings/SettingsPage.jsx");

/** 将具名页面导出转换成 React 懒加载组件，例如 `loader + EnglishPage -> LazyComponent`。 */
function lazyPage(loader, exportName) {
  return lazy(async () => {
    const loadedModule = await loader();
    return { default: loadedModule[exportName] };
  });
}

/** 已启用业务模块清单；只登记明确需求，例如 `english -> EnglishPage`。 */
export const modules = [
  {
    id: "english",
    label: "英语",
    menuOrder: 2,
    component: lazyPage(loadEnglish, "EnglishPage"),
    preload: loadEnglish,
  },
  {
    id: "settings",
    label: "设置",
    menuOrder: 1,
    component: lazyPage(loadSettings, "SettingsPage"),
    preload: loadSettings,
  },
];
