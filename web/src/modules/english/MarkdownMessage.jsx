import React from "react";
import { Streamdown } from "streamdown";
import "streamdown/styles.css";

/** 渲染流式 Markdown 回复，例如 `**hel -> 实时粗体且仅重算末尾块`。 */
export function MarkdownMessage({ content, streaming }) {
  return (
    <Streamdown
      caret="block"
      className="markdown-content"
      controls={false}
      isAnimating={streaming}
      lineNumbers={false}
      mode={streaming ? "streaming" : "static"}
      skipHtml
    >
      {content}
    </Streamdown>
  );
}
