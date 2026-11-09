import React, { useState } from "react";
import { settingsSections } from "./sections.js";

/** 渲染可搜索的设置目录，例如 `hundreds of modules -> filtered navigation`。 */
export function SettingsNavigation({ activeId, onSelect }) {
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleSections = settingsSections.filter((section) =>
    `${section.label} ${section.description}`.toLocaleLowerCase().includes(normalizedQuery),
  );

  return (
    <aside className="settings-navigation">
      <label className="settings-search">
        <span>查找设置</span>
        <input
          onChange={(event) => setQuery(event.target.value)}
          placeholder="搜索模块"
          type="search"
          value={query}
        />
      </label>
      <nav className="settings-section-list" aria-label="设置分类">
        {visibleSections.map((section) => (
          <button
            className={section.id === activeId ? "settings-section active" : "settings-section"}
            key={section.id}
            onClick={() => onSelect(section.id)}
            type="button"
          >
            <span>{section.scope}</span>
            <strong>{section.label}</strong>
            <small>{section.description}</small>
          </button>
        ))}
        {visibleSections.length === 0 && <p className="settings-search-empty">没有匹配项</p>}
      </nav>
    </aside>
  );
}
