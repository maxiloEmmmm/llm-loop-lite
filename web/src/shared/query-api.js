import { composeWebSystemPrompt, loadWebSettings } from "./settings.js";

/** 解析一个 SSE frame，例如 `data: {...} -> object`。 */
function parseFrame(frame) {
  const data = frame
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trimStart())
    .join("\n");
  return data ? JSON.parse(data) : null;
}

/** 消费 Web 查询 SSE，并强制把全局提示词放在模块提示词之前。 */
export async function streamQuery(request, handlers) {
  const webSettings = loadWebSettings();
  const payload = {
    ...request,
    systemPrompt: composeWebSystemPrompt(webSettings.systemPrompt, request.systemPrompt),
  };
  const response = await fetch("/api/query", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify(payload),
    signal: handlers.signal,
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: `HTTP ${response.status}` }));
    throw new Error(body.error ?? `HTTP ${response.status}`);
  }
  if (!response.body) throw new Error("浏览器没有收到 SSE 响应体");

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  let queuedDelta = "";
  let frameId = null;

  /** 合并同一帧内的 token，避免高频 SSE 触发过量 React 渲染，例如 `10 chunks -> 1 update`。 */
  const flushDelta = () => {
    const text = queuedDelta;
    queuedDelta = "";
    frameId = null;
    if (text) handlers.onDelta(text);
  };

  /** 将 token 排到下一绘制帧，例如 `delta -> at most 60 updates/s`。 */
  const queueDelta = (text) => {
    queuedDelta += text;
    if (frameId === null) frameId = requestAnimationFrame(flushDelta);
  };

  while (true) {
    const { value, done } = await reader.read();
    pending += decoder.decode(value ?? new Uint8Array(), { stream: !done });
    const frames = pending.split(/\r?\n\r?\n/);
    pending = done ? "" : frames.pop() ?? "";
    for (const frame of frames) {
      const event = parseFrame(frame);
      if (event?.type === "delta") queueDelta(event.text);
      else if (event?.type === "done") {
        if (frameId !== null) cancelAnimationFrame(frameId);
        flushDelta();
        handlers.onDone(event.total_tokens ?? null);
      } else if (event?.type === "error") {
        if (frameId !== null) cancelAnimationFrame(frameId);
        flushDelta();
        throw new Error(event.message);
      }
    }
    if (done) break;
  }
  if (frameId !== null) cancelAnimationFrame(frameId);
  flushDelta();
}
