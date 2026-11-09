import React from "react";

/** 渲染首个回复片段到达前的等待动画，例如 `empty streaming reply -> pulsing dots`。 */
export function AssistantLoading() {
  return (
    <span aria-label="正在生成回复" className="assistant-loading" role="status">
      <i />
      <i />
      <i />
    </span>
  );
}
