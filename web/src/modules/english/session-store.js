import { messageId } from "../../shared/identity.js";

const SESSION_KEY = "llm-loop.english.sessions";

/** 返回英语模块空会话，例如 `new browser -> main + no cards`。 */
export function emptyEnglishSessions() {
  return { main: { id: "english-main", messages: [] }, cards: [], activeCardId: null };
}

/** 读取英语模块会话，例如 `localStorage -> main + cards`。 */
export function loadEnglishSessions() {
  try {
    return { ...emptyEnglishSessions(), ...JSON.parse(localStorage.getItem(SESSION_KEY) ?? "{}") };
  } catch {
    return emptyEnglishSessions();
  }
}

/** 保存英语模块会话，例如 `state -> localStorage`。 */
export function saveEnglishSessions(sessions) {
  localStorage.setItem(SESSION_KEY, JSON.stringify(sessions));
}

/** 创建理解 Card，例如 `selected text -> independent card session`。 */
export function createUnderstandCard(selectedText) {
  return {
    id: `understand-${messageId()}`,
    selectedText,
    messages: [],
    createdAt: Date.now(),
  };
}
