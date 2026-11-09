/** 设置页登记项；系统固定在模块之前，新增模块只需追加一项。 */
export const settingsSections = [
  {
    id: "system",
    label: "系统提示词",
    scope: "GLOBAL",
    description: "所有 Web 模块请求的最前置提示词",
  },
  {
    id: "english",
    label: "英语",
    scope: "MODULE",
    description: "主会话与理解 Card",
  },
];
