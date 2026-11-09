import React from "react";
import { Composer } from "./Composer.jsx";
import { MessageList } from "./MessageList.jsx";

/** 渲染可切换的理解 Card 抽屉，例如 `cards + activeId -> active conversation`。 */
export function UnderstandDrawer({ cards, activeId, onActivate, onClose, onSend, running }) {
  const activeCard = cards.find((card) => card.id === activeId);
  if (!activeCard) return null;
  return (
    <aside className="understand-drawer">
      <header className="drawer-head">
        <div className="card-tabs">
          {cards.map((card, index) => (
            <button
              className={card.id === activeId ? "card-tab active" : "card-tab"}
              key={card.id}
              onClick={() => onActivate(card.id)}
              type="button"
            >
              理解 {index + 1}
            </button>
          ))}
        </div>
        <button className="drawer-close" onClick={onClose} type="button">收起</button>
      </header>
      <div className="drawer-card">
        <MessageList messages={activeCard.messages} />
      </div>
      <Composer
        compact
        disabled={running}
        onSend={(text) => onSend(activeCard.id, text)}
        placeholder="继续追问"
      />
    </aside>
  );
}
