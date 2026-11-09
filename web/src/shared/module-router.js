/** 从 URL hash 解析已登记模块，例如 `#chat + modules -> chat`。 */
export function moduleFromHash(modules) {
  const requested = window.location.hash.slice(1);
  return modules.some((module) => module.id === requested) ? requested : modules[0].id;
}

/** 从模块清单取得页面组件，例如 `chat + modules -> ChatPage`。 */
export function moduleComponent(modules, moduleId) {
  return modules.find((module) => module.id === moduleId)?.component ?? modules[0].component;
}
