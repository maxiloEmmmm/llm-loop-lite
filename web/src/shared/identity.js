/** 读取或生成浏览器稳定 ID，例如 `llm-loop.client-id -> uuid`。 */
export function persistentId(key) {
  const stored = localStorage.getItem(key);
  if (stored) return stored;
  const value = randomId();
  localStorage.setItem(key, value);
  return value;
}

/** 生成单次消息 ID，例如 `messageId() -> uuid`。 */
export function messageId() {
  return randomId();
}

/** 使用浏览器随机源生成 128 位 ID，例如 `random bytes -> 32 hex chars`。 */
function randomId() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
}
