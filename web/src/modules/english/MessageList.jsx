import React, { lazy, Suspense } from "react";
import { AssistantLoading } from "./AssistantLoading.jsx";

const MarkdownMessage = lazy(() =>
  import("./MarkdownMessage.jsx").then((module) => ({ default: module.MarkdownMessage })),
);

/** 渲染可选择文本的消息列表，例如 `messages -> selectable bubbles`。 */
export function MessageList({ messages, onClick, onPointerUp }) {
  return (
    <div className="english-messages" onClick={onClick} onPointerUp={onPointerUp}>
      {messages.map((message) => (
        <article className={`english-message ${message.role}`} key={message.id}>
          <div className="selectable-message">
            {message.role === "assistant" && message.status === "streaming" && !message.content ? (
              <AssistantLoading />
            ) : message.role === "assistant" ? (
              <Suspense fallback={<span className="markdown-loading">{message.content}</span>}>
                <MarkdownMessage
                  content={message.content}
                  streaming={message.status === "streaming"}
                />
              </Suspense>
            ) : (
              message.displayContent ?? message.content
            )}
          </div>
        </article>
      ))}
    </div>
  );
}
