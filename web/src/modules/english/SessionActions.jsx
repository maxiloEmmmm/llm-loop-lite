import React, { useEffect, useRef, useState } from "react";

/** 渲染 Session 操作弹层，例如 `click trigger -> Reset menu above composer`。 */
export function SessionActions({ disabled, onReset }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef(null);

  useEffect(() => {
    /** 点击组件外部时关闭操作层，例如 `outside pointer -> closed menu`。 */
    const closeOutside = (event) => {
      if (!rootRef.current?.contains(event.target)) setOpen(false);
    };
    /** 按 Escape 时关闭操作层，例如 `Escape -> closed menu`。 */
    const closeWithKeyboard = (event) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeWithKeyboard);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeWithKeyboard);
    };
  }, []);

  /** 重置当前 Session 并关闭弹层，例如 `Reset -> empty current history`。 */
  const reset = () => {
    onReset();
    setOpen(false);
  };

  return (
    <div className="session-actions" ref={rootRef}>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label="Session 操作"
        className="session-actions-trigger"
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        ···
      </button>
      {open && (
        <div className="session-actions-menu" role="menu">
          <button onClick={reset} role="menuitem" type="button">Reset</button>
        </div>
      )}
    </div>
  );
}
