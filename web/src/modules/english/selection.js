const WORD_CHARACTER = /[A-Za-z'-]/;

/** 读取当前有效文本选择，例如 `dragged text -> text + viewport rect`。 */
export function currentTextSelection(container) {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!container.contains(range.commonAncestorContainer)) return null;
  const text = selection.toString().trim();
  if (!text) return null;
  return { text, rect: range.getBoundingClientRect() };
}

/** 从点击位置选择英文单词，例如 `pointer x/y -> word + rect`。 */
export function wordAtPoint(container, x, y) {
  const range = document.caretRangeFromPoint?.(x, y);
  const node = range?.startContainer;
  if (!range || !node || node.nodeType !== Node.TEXT_NODE || !container.contains(node)) return null;
  const source = node.textContent ?? "";
  let offset = Math.min(range.startOffset, Math.max(0, source.length - 1));
  if (!WORD_CHARACTER.test(source[offset] ?? "") && offset > 0) offset -= 1;
  if (!WORD_CHARACTER.test(source[offset] ?? "")) return null;
  let start = offset;
  let end = offset + 1;
  while (start > 0 && WORD_CHARACTER.test(source[start - 1])) start -= 1;
  while (end < source.length && WORD_CHARACTER.test(source[end])) end += 1;
  const wordRange = document.createRange();
  wordRange.setStart(node, start);
  wordRange.setEnd(node, end);
  const selection = window.getSelection();
  selection.removeAllRanges();
  selection.addRange(wordRange);
  return { text: wordRange.toString(), rect: wordRange.getBoundingClientRect() };
}
