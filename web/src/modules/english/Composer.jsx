import React, { useState } from "react";
import { SessionActions } from "./SessionActions.jsx";

/** 渲染 Session 输入框，例如 `Enter -> onSend(text)`。 */
export function Composer({ disabled, compact = false, onReset, onSend, placeholder = "输入消息" }) {
  const [text, setText] = useState("");

  /** 提交非空消息，例如 `hello -> send + clear`。 */
  const submit = () => {
    const value = text.trim();
    if (!value || disabled) return;
    onSend(value);
    setText("");
  };

  /** 处理 Enter 发送，例如 `Enter -> submit`。 */
  const keyDown = (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  };

  const className = ["session-composer", compact ? "compact" : "", onReset ? "with-actions" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={className}>
      {onReset && <SessionActions disabled={disabled} onReset={onReset} />}
      <textarea
        disabled={disabled}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={keyDown}
        placeholder={disabled ? "生成中…" : placeholder}
        rows="1"
        value={text}
      />
      <button
        className="session-send"
        disabled={disabled || !text.trim()}
        onClick={submit}
        type="button"
      >
        发送
      </button>
    </div>
  );
}
