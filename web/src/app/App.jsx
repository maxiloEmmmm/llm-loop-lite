import React, { Suspense, useEffect, useState } from "react";
import { moduleComponent, moduleFromHash } from "../shared/module-router.js";
import { modules } from "./modules.js";

const menuModules = [...modules].sort((left, right) => left.menuOrder - right.menuOrder);

/** 渲染应用壳并只负责模块切换，例如 `#chat -> ChatPage`。 */
export function App() {
  const [moduleId, setModuleId] = useState(() => moduleFromHash(modules));
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    /** 同步浏览器 hash 到当前业务模块，例如 `hashchange -> status`。 */
    const onHashChange = () => setModuleId(moduleFromHash(modules));
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const CurrentPage = moduleComponent(modules, moduleId);
  const currentModule = modules.find((module) => module.id === moduleId) ?? modules[0];

  /** 切换实际业务模块，例如 `settings -> SettingsPage`。 */
  const selectModule = (nextModuleId) => {
    window.location.hash = nextModuleId;
    setMenuOpen(false);
  };

  return (
    <div className="app-shell">
      <header className="light-strip">
        <button
          aria-expanded={menuOpen}
          aria-label={menuOpen ? "关闭模块菜单" : "打开模块菜单"}
          className="light-strip-trigger"
          onClick={() => setMenuOpen((open) => !open)}
          type="button"
        >
          <span className="light-strip-glow" />
          <span className="light-strip-brand">LLM LOOP</span>
          <span className="light-strip-current">{currentModule.label}</span>
        </button>
        {menuOpen && (
          <nav className="module-menu" aria-label="模块菜单">
            {menuModules.map((item, index) => (
              <button
                className={item.id === moduleId ? "module-menu-item active" : "module-menu-item"}
                key={item.id}
                onClick={() => selectModule(item.id)}
                onFocus={item.preload}
                onPointerEnter={item.preload}
                type="button"
              >
                <span>{String(index + 1).padStart(2, "0")}</span>
                <strong>{item.label}</strong>
              </button>
            ))}
          </nav>
        )}
      </header>
      <main className="module-viewport">
        <Suspense fallback={<div className="module-loading">加载中…</div>}>
          <CurrentPage />
        </Suspense>
      </main>
    </div>
  );
}
