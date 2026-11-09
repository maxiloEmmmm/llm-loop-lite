import React, { useEffect, useRef, useState } from "react";
import { messageId } from "../../shared/identity.js";
import { streamQuery } from "../../shared/query-api.js";
import { loadEnglishSettings } from "../../shared/settings.js";
import { Composer } from "./Composer.jsx";
import { MessageList } from "./MessageList.jsx";
import { SelectionMenu } from "./SelectionMenu.jsx";
import { UnderstandDrawer } from "./UnderstandDrawer.jsx";
import { currentTextSelection, wordAtPoint } from "./selection.js";
import {
  createUnderstandCard,
  loadEnglishSessions,
  saveEnglishSessions,
} from "./session-store.js";

/** 渲染英语业务模块，例如 `main session + understand cards -> conversations`。 */
export function EnglishPage() {
  const [sessions, setSessions] = useState(loadEnglishSessions);
  const [settings, setSettings] = useState(loadEnglishSettings);
  const [selection, setSelection] = useState(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [running, setRunning] = useState(() => new Set());
  const moduleRef = useRef(null);

  useEffect(() => saveEnglishSessions(sessions), [sessions]);
  useEffect(() => {
    /** 同步设置模块保存的提示词，例如 `settings event -> prompts`。 */
    const updateSettings = (event) => setSettings(event.detail);
    window.addEventListener("llm-loop:english-settings", updateSettings);
    return () => window.removeEventListener("llm-loop:english-settings", updateSettings);
  }, []);

  /** 标记指定 session 正在或结束生成，例如 `english-main + true -> disabled`。 */
  const markRunning = (sessionId, value) => {
    setRunning((current) => {
      const next = new Set(current);
      if (value) next.add(sessionId);
      else next.delete(sessionId);
      return next;
    });
  };

  /** 更新主 Session 中指定消息，例如 `assistant id + delta -> appended text`。 */
  const updateMainMessage = (messageIdValue, update) => {
    setSessions((current) => ({
      ...current,
      main: {
        ...current.main,
        messages: current.main.messages.map((message) =>
          message.id === messageIdValue ? update(message) : message,
        ),
      },
    }));
  };

  /** 更新 Card Session 中指定消息，例如 `card + assistant + delta -> appended text`。 */
  const updateCardMessage = (cardId, messageIdValue, update) => {
    setSessions((current) => ({
      ...current,
      cards: current.cards.map((card) =>
        card.id === cardId
          ? {
              ...card,
              messages: card.messages.map((message) =>
                message.id === messageIdValue ? update(message) : message,
              ),
            }
          : card,
      ),
    }));
  };

  /** 执行主 Session 查询，例如 `raw text -> streamed assistant message`。 */
  const sendMain = async (content) => {
    const history = sessions.main.messages.map(({ role, content: text }) => ({ role, content: text }));
    const user = { id: messageId(), role: "user", content };
    const assistant = { id: messageId(), role: "assistant", content: "", status: "streaming" };
    setSessions((current) => ({
      ...current,
      main: { ...current.main, messages: [...current.main.messages, user, assistant] },
    }));
    markRunning(sessions.main.id, true);
    try {
      await streamQuery(
        {
          sessionId: sessions.main.id,
          systemPrompt: settings.mainPrompt,
          messages: history,
          content,
        },
        {
          onDelta: (text) =>
            updateMainMessage(assistant.id, (message) => ({
              ...message,
              content: message.content + text,
            })),
          onDone: () =>
            updateMainMessage(assistant.id, (message) => ({ ...message, status: "done" })),
        },
      );
    } catch (error) {
      updateMainMessage(assistant.id, (message) => ({
        ...message,
        content: message.content || error.message,
        status: "error",
      }));
    } finally {
      markRunning(sessions.main.id, false);
    }
  };

  /** 执行 Card Session 查询，例如 `card + follow-up -> streamed assistant message`。 */
  const sendCard = async (cardId, content, displayContent = null, suppliedHistory = null) => {
    const card = sessions.cards.find((item) => item.id === cardId);
    const priorMessages = suppliedHistory ?? card?.messages ?? [];
    const history = priorMessages.map(({ role, content: text }) => ({ role, content: text }));
    const user = { id: messageId(), role: "user", content, displayContent };
    const assistant = { id: messageId(), role: "assistant", content: "", status: "streaming" };
    setSessions((current) => ({
      ...current,
      cards: current.cards.map((item) =>
        item.id === cardId ? { ...item, messages: [...item.messages, user, assistant] } : item,
      ),
    }));
    markRunning(cardId, true);
    try {
      await streamQuery(
        {
          sessionId: cardId,
          systemPrompt: settings.understandPrompt,
          messages: history,
          content,
        },
        {
          onDelta: (text) =>
            updateCardMessage(cardId, assistant.id, (message) => ({
              ...message,
              content: message.content + text,
            })),
          onDone: () =>
            updateCardMessage(cardId, assistant.id, (message) => ({ ...message, status: "done" })),
        },
      );
    } catch (error) {
      updateCardMessage(cardId, assistant.id, (message) => ({
        ...message,
        content: message.content || error.message,
        status: "error",
      }));
    } finally {
      markRunning(cardId, false);
    }
  };

  /** 从选中文本创建独立 Card 并发送首轮理解请求，例如 `phrase -> new card`。 */
  const understand = (text) => {
    const card = createUnderstandCard(text);
    setSessions((current) => ({
      ...current,
      cards: [...current.cards, card],
      activeCardId: card.id,
    }));
    setSelection(null);
    window.getSelection()?.removeAllRanges();
    setDrawerOpen(true);
    void sendCard(card.id, `<content>${text}</content>`, text, []);
  };

  /** 清空主 Session 历史，例如 `Reset -> empty main messages`。 */
  const resetMain = () => {
    setSelection(null);
    window.getSelection()?.removeAllRanges();
    setSessions((current) => ({
      ...current,
      main: { ...current.main, messages: [] },
    }));
  };

  /** 处理拖动文本选择，例如 `pointer up -> selection menu`。 */
  const selectText = () => {
    const selected = currentTextSelection(moduleRef.current);
    if (selected) setSelection(selected);
  };

  /** 处理单击单词，例如 `click word -> selection menu`。 */
  const clickWord = (event) => {
    if (!event.target.closest(".selectable-message")) return;
    const selected = currentTextSelection(moduleRef.current);
    if (selected) {
      setSelection(selected);
      return;
    }
    const word = wordAtPoint(moduleRef.current, event.clientX, event.clientY);
    if (word) setSelection(word);
  };

  return (
    <section
      className="english-page"
      onClick={clickWord}
      onPointerUp={selectText}
      ref={moduleRef}
    >
      <div className="main-session">
        {sessions.main.messages.length === 0 ? (
          <div className="english-empty">从一句话开始。</div>
        ) : (
          <MessageList messages={sessions.main.messages} />
        )}
      </div>
      <Composer
        disabled={running.has(sessions.main.id)}
        onReset={resetMain}
        onSend={sendMain}
        placeholder="和主 Session 对话"
      />
      <SelectionMenu selection={selection} onUnderstand={understand} />
      {drawerOpen && (
        <UnderstandDrawer
          activeId={sessions.activeCardId}
          cards={sessions.cards}
          onActivate={(cardId) =>
            setSessions((current) => ({ ...current, activeCardId: cardId }))
          }
          onClose={() => setDrawerOpen(false)}
          onSend={sendCard}
          running={running.has(sessions.activeCardId)}
        />
      )}
    </section>
  );
}
