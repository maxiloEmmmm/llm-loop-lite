import React, { useState } from "react";
import {
  loadEnglishSettings,
  loadWebSettings,
  saveEnglishSettings,
  saveWebSettings,
} from "../../shared/settings.js";
import { SettingsNavigation } from "./SettingsNavigation.jsx";

/** 渲染可扩展设置中心，例如 `global + module prompts -> persisted settings`。 */
export function SettingsPage() {
  const [activeSection, setActiveSection] = useState("system");
  const [webSettings, setWebSettings] = useState(loadWebSettings);
  const [englishSettings, setEnglishSettings] = useState(loadEnglishSettings);
  const [saved, setSaved] = useState(false);

  /** 更新 Web 全局提示词，例如 `text -> global system prompt`。 */
  const updateWebPrompt = (value) => {
    setSaved(false);
    setWebSettings((current) => ({ ...current, systemPrompt: value }));
  };

  /** 更新英语模块提示词，例如 `mainPrompt + text -> module settings`。 */
  const updateEnglishPrompt = (name, value) => {
    setSaved(false);
    setEnglishSettings((current) => ({ ...current, [name]: value }));
  };

  /** 一次保存全局与模块设置，例如 `submit -> both localStorage keys`。 */
  const submit = (event) => {
    event.preventDefault();
    saveWebSettings(webSettings);
    saveEnglishSettings(englishSettings);
    setSaved(true);
  };

  return (
    <section className="settings-page">
      <header className="settings-topbar">
        <div>
          <span>SETTINGS</span>
          <h1>设置</h1>
          <p>统一管理全局规则与各业务模块。</p>
        </div>
        <div className="settings-save">
          {saved && <span>已保存</span>}
          <button form="settings-editor" type="submit">保存更改</button>
        </div>
      </header>
      <div className="settings-workspace">
        <SettingsNavigation activeId={activeSection} onSelect={setActiveSection} />
        <form className="settings-editor" id="settings-editor" onSubmit={submit}>
          {activeSection === "system" ? (
            <section className="settings-panel">
              <header className="settings-panel-heading">
                <span>GLOBAL</span>
                <h2>系统提示词</h2>
                <p>所有从 Web 发出的请求都会先放入这里的内容，再拼接模块自己的提示词。</p>
              </header>
              <div className="prompt-order" aria-label="提示词拼接顺序">
                <strong>系统提示词</strong><i>→</i><span>模块提示词</span><i>→</i><span>用户内容</span>
              </div>
              <label className="prompt-field global-prompt-field">
                <span>全局内容</span>
                <small>固定处于最终 system prompt 的最前方。</small>
                <textarea
                  onChange={(event) => updateWebPrompt(event.target.value)}
                  placeholder="输入所有 Web 模块共用的系统规则"
                  value={webSettings.systemPrompt}
                />
              </label>
            </section>
          ) : (
            <section className="settings-panel">
              <header className="settings-panel-heading">
                <span>MODULE</span>
                <h2>英语</h2>
                <p>这些提示词位于系统提示词之后，只影响英语模块对应的会话。</p>
              </header>
              <div className="module-prompt-grid">
                <label className="prompt-field">
                  <span>主 Session</span>
                  <small>英语主会话使用的模块提示词。</small>
                  <textarea
                    onChange={(event) => updateEnglishPrompt("mainPrompt", event.target.value)}
                    value={englishSettings.mainPrompt}
                  />
                </label>
                <label className="prompt-field">
                  <span>理解 Card</span>
                  <small>选中文本后，新建理解 Card 使用的模块提示词。</small>
                  <textarea
                    onChange={(event) => updateEnglishPrompt("understandPrompt", event.target.value)}
                    value={englishSettings.understandPrompt}
                  />
                </label>
              </div>
            </section>
          )}
        </form>
      </div>
    </section>
  );
}
