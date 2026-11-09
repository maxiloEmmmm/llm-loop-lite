import React from "react";

/** 在选择文本上方渲染操作菜单，例如 `selection -> 理解`。 */
export function SelectionMenu({ selection, onUnderstand }) {
  if (!selection) return null;
  const left = Math.min(window.innerWidth - 58, Math.max(58, selection.rect.left + selection.rect.width / 2));
  const top = Math.max(56, selection.rect.top - 12);
  return (
    <div className="selection-menu" style={{ left, top }}>
      <button
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => onUnderstand(selection.text)}
        type="button"
      >
        理解
      </button>
    </div>
  );
}
