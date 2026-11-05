// Панель настроек: модели и ключи, голос, горячие клавиши, разрешения,
// персонаж, автозапуск. Секреты приходят из бэкенда замаскированными;
// маска = «не менял», реальное значение остаётся в DPAPI-хранилище.

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Config, ConnectedModel, Note } from "./types";

const PROVIDERS = [
  { id: "gemini", label: "Google Gemini" },
  { id: "anthropic", label: "Anthropic Claude" },
  { id: "openai", label: "OpenAI GPT" },
  { id: "custom", label: "Своя модель (OpenAI-совместимый API)" },
  { id: "local", label: "Локальная (Ollama)" },
];

const CAPABILITIES = ["text", "vision", "presentation", "image", "code"];

export function SettingsPanel({
  onClose,
  onConfigSaved,
}: {
  onClose: () => void;
  onConfigSaved: (cfg: Config) => void;
}) {
  const [tab, setTab] = useState<"general" | "models" | "voice" | "permissions" | "character" | "notes">("general");
  const [cfg, setCfg] = useState<Config | null>(null);
  const [permissions, setPermissions] = useState<[string, string][]>([]);
  const [facts, setFacts] = useState<string[]>([]);
  const [notes, setNotes] = useState<Note[]>([]);
  const [saving, setSaving] = useState(false);
  const [scanCount, setScanCount] = useState<number | null>(null);
  const [newModel, setNewModel] = useState<ConnectedModel | null>(null);

  useEffect(() => {
    (async () => {
      setCfg(await invoke<Config>("get_config"));
      setPermissions(await invoke<[string, string][]>("permission_list"));
      setFacts(await invoke<string[]>("memory_facts").catch(() => []));
      setNotes(await invoke<Note[]>("notes").catch(() => []));
    })();
  }, []);

  const set = <K extends keyof Config>(key: K, value: Config[K]) => {
    setCfg((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  const save = async () => {
    if (!cfg) return;
    setSaving(true);
    try {
      await invoke("save_config", { cfg });
      const fresh = await invoke<Config>("get_config");
      setCfg(fresh);
      onConfigSaved(fresh);
    } finally {
      setSaving(false);
    }
  };

  const tabs = useMemo(
    () =>
      [
        ["general", "Общие"],
        ["models", "Модели"],
        ["voice", "Голос"],
        ["permissions", "Разрешения"],
        ["character", "Персонаж"],
        ["notes", "Память и заметки"],
      ] as const,
    [],
  );

  if (!cfg) return null;

  return (
    <div className="dialog-backdrop settings-backdrop">
      <div className="settings">
        <div className="settings-head" data-tauri-drag-region>
          <div className="settings-title">Настройки</div>
          <div className="settings-tabs">
            {tabs.map(([id, label]) => (
              <button key={id} className={`settings-tab ${tab === id ? "active" : ""}`} onClick={() => setTab(id)}>
                {label}
              </button>
            ))}
          </div>
          <button className="tb-btn tb-close" title="Закрыть" onClick={onClose} />
        </div>

        <div className="settings-body">
          {tab === "general" && (
            <Section title="Общие">
              <Field label="Как называть тебя">
                <input value={cfg.user_name} onChange={(e) => set("user_name", e.target.value)} placeholder="имя" />
              </Field>
              <Field label="Стиль ответов">
                <select value={cfg.reply_style} onChange={(e) => set("reply_style", e.target.value)}>
                  <option value="short">Коротко (1–3 предложения)</option>
                  <option value="normal">Средне (3–5)</option>
                  <option value="detailed">Подробно (до 8)</option>
                </select>
              </Field>
              <Field label="Язык ответов">
                <select value={cfg.language} onChange={(e) => set("language", e.target.value)}>
                  <option value="auto">Как в вопросе</option>
                  <option value="ru">Русский</option>
                  <option value="en">English</option>
                </select>
              </Field>
              <Field label="Характер (persona) — свободный текст">
                <textarea
                  rows={3}
                  value={cfg.persona}
                  onChange={(e) => set("persona", e.target.value)}
                  placeholder="Например: немного ироничный, отвечает по делу, обращается на «вы»"
                />
              </Field>
              <Field label="Wake word">
                <input value={cfg.wake_word} onChange={(e) => set("wake_word", e.target.value)} />
              </Field>
              <Field label="Горячая клавиша оверлея">
                <input
                  value={cfg.overlay_hotkey}
                  onChange={(e) => set("overlay_hotkey", e.target.value)}
                  placeholder="Alt+Space"
                />
              </Field>
              <Toggle label="Запускаться с Windows" value={cfg.autostart} onChange={(v) => set("autostart", v)} />
              <Toggle label="Веб-поиск" value={cfg.use_web_search} onChange={(v) => set("use_web_search", v)} />
              <Toggle label="Таймеры" value={cfg.timers_enabled} onChange={(v) => set("timers_enabled", v)} />
            </Section>
          )}

          {tab === "models" && (
            <>
              <Section title="Основная модель (диалог)">
                <Field label="Провайдер">
                  <select value={cfg.llm_provider} onChange={(e) => set("llm_provider", e.target.value)}>
                    {PROVIDERS.map((p) => (
                      <option key={p.id} value={p.id}>{p.label}</option>
                    ))}
                  </select>
                </Field>
                <Field label="API-ключ">
                  <input
                    type="password"
                    value={cfg.llm_key}
                    onChange={(e) => set("llm_key", e.target.value)}
                    placeholder="хранится в DPAPI Windows"
                  />
                </Field>
                <Field label="Модель (пусто — авто)">
                  <input value={cfg.llm_model} onChange={(e) => set("llm_model", e.target.value)} />
                </Field>
                {(cfg.llm_provider === "custom" || cfg.llm_provider === "local") && (
                  <Field label="Base URL">
                    <input
                      value={cfg.llm_base_url}
                      onChange={(e) => set("llm_base_url", e.target.value)}
                      placeholder="http://localhost:11434/v1"
                    />
                  </Field>
                )}
              </Section>

              <Section
                title="Подключённые модели (эскалация больших задач)"
                hint="Крупные задачи — презентации, картинки, код, vision — JARVIS честно делегирует подключённой модели. Без неё скажет, что не может."
                action={
                  <button
                    className="btn"
                    onClick={() =>
                      setNewModel({
                        id: `m${Date.now()}`,
                        label: "",
                        provider: "openai",
                        api_key: "",
                        model: "",
                        base_url: "",
                        capabilities: ["text"],
                      })
                    }
                  >
                    Добавить
                  </button>
                }
              >
                {cfg.connected_models.length === 0 && !newModel && (
                  <div className="hint">Пока не подключено ни одной внешней модели.</div>
                )}
                {cfg.connected_models.map((m, i) => (
                  <ModelRow key={m.id} model={m} onChange={(nm) => {
                    const list = [...cfg.connected_models];
                    list[i] = nm;
                    set("connected_models", list);
                  }} onRemove={() => {
                    set("connected_models", cfg.connected_models.filter((x) => x.id !== m.id));
                  }} />
                ))}
                {newModel && (
                  <ModelRow
                    model={newModel}
                    onChange={setNewModel}
                    onAdd={() => {
                      set("connected_models", [...cfg.connected_models, newModel]);
                      setNewModel(null);
                    }}
                    adding
                  />
                )}
              </Section>
            </>
          )}

          {tab === "voice" && (
            <Section title="Голос">
              <Toggle label="Озвучивать ответы (TTS)" value={cfg.tts_enabled} onChange={(v) => set("tts_enabled", v)} />
              <Field label="Голос">
                <select value={cfg.voice} onChange={(e) => set("voice", e.target.value)}>
                  <option value="female">Светлана (edge-tts)</option>
                  <option value="male">Дмитрий (edge-tts)</option>
                </select>
              </Field>
              <Field label="Распознавание речи (STT)">
                <select value={cfg.stt_engine} onChange={(e) => set("stt_engine", e.target.value)}>
                  <option value="google">Google (без ключа)</option>
                  <option value="openai">OpenAI Whisper (нужен ключ ниже)</option>
                </select>
              </Field>
              {cfg.stt_engine === "openai" && (
                <Field label="OpenAI ключ">
                  <input
                    type="password"
                    value={cfg.keys.openai ?? ""}
                    onChange={(e) => set("keys", { ...cfg.keys, openai: e.target.value })}
                  />
                </Field>
              )}
              <Field label="Папка заметок и документов">
                <input value={cfg.notes_dir} onChange={(e) => set("notes_dir", e.target.value)} placeholder="Документы" />
              </Field>
              <div className="dialog-row">
                <button
                  className="btn"
                  onClick={() => invoke("speak_test", { text: "Проверка голоса. Система в порядке.", voice: cfg.voice })}
                >
                  Проверить голос
                </button>
              </div>
            </Section>
          )}

          {tab === "permissions" && (
            <Section
              title="Разрешения"
              hint="Гранулярно и отзываемо. Рискованные действия (удаление) всегда требуют подтверждения — даже при выданном разрешении."
              action={
                <button
                  className="btn"
                  onClick={async () => {
                    const r = await invoke<{ count: number }>("scan_apps").catch(() => null);
                    if (r) setScanCount(r.count);
                  }}
                >
                  Пересканировать программы
                </button>
              }
            >
              {scanCount !== null && <div className="hint">Проиндексировано программ: {scanCount} (локально).</div>}
              {permissions.map(([key, label]) => (
                <Toggle
                  key={key}
                  label={label}
                  value={cfg.permissions[key] ?? false}
                  onChange={async (v) => {
                    await invoke("set_permission", { key, value: v });
                    set("permissions", { ...cfg.permissions, [key]: v });
                    if (key === "app_scan" && v) {
                      const r = await invoke<{ count: number }>("scan_apps").catch(() => null);
                      if (r) setScanCount(r.count);
                    }
                  }}
                />
              ))}
              <Field label="Дополнительные папки, где можно удалять (по одной в строке)">
                <textarea
                  rows={2}
                  value={cfg.extra_allowed_dirs.join("\n")}
                  onChange={(e) => set("extra_allowed_dirs", e.target.value.split("\n").map((s) => s.trim()).filter(Boolean))}
                />
              </Field>
            </Section>
          )}

          {tab === "character" && (
            <Section title="Персонаж">
              <Toggle label="Показывать персонажа" value={cfg.character_enabled} onChange={(v) => set("character_enabled", v)} />
            </Section>
          )}

          {tab === "notes" && (
            <>
              <Section
                title="Факты памяти"
                hint="Всё, что JARVIS запомнил о тебе. Удаляется мгновенно и безвозвратно."
              >
                {facts.length === 0 && <div className="hint">Память пуста. Скажи: «запомни, что я…».</div>}
                {facts.map((f, i) => (
                  <div key={i} className="fact-row">
                    <span>{f}</span>
                    <button
                      className="btn-mini"
                      onClick={async () => {
                        await invoke("memory_remove", { query: f });
                        setFacts(await invoke<string[]>("memory_facts"));
                      }}
                    >
                      забыть
                    </button>
                  </div>
                ))}
              </Section>
              <Section title="Заметки и голосовые записи">
                {notes.length === 0 && <div className="hint">Заметок пока нет. Кнопка диктофона — в поле ввода (иконка микрофона в оверлее) или скажи «запиши заметку».</div>}
                {notes.map((n) => (
                  <div key={n.id} className="fact-row">
                    <span>
                      <b>{n.title}</b> {n.kind === "voice" ? "(голосовая)" : ""} — {n.body.slice(0, 120)}
                    </span>
                  </div>
                ))}
              </Section>
            </>
          )}
        </div>

        <div className="settings-foot">
          <span className="hint">Ключи хранятся в DPAPI Windows и не покидают компьютер.</span>
          <button className="btn btn-primary" disabled={saving} onClick={save}>
            {saving ? "Сохраняю…" : "Сохранить"}
          </button>
        </div>
      </div>
    </div>
  );
}

function ModelRow({
  model,
  onChange,
  onRemove,
  onAdd,
  adding,
}: {
  model: ConnectedModel;
  onChange: (m: ConnectedModel) => void;
  onRemove?: () => void;
  onAdd?: () => void;
  adding?: boolean;
}) {
  const toggleCap = (cap: string) => {
    const has = model.capabilities.includes(cap);
    onChange({
      ...model,
      capabilities: has ? model.capabilities.filter((c) => c !== cap) : [...model.capabilities, cap],
    });
  };
  return (
    <div className="model-row">
      <div className="model-grid">
        <input placeholder="Название" value={model.label} onChange={(e) => onChange({ ...model, label: e.target.value })} />
        <select value={model.provider} onChange={(e) => onChange({ ...model, provider: e.target.value })}>
          {PROVIDERS.map((p) => (
            <option key={p.id} value={p.id}>{p.label}</option>
          ))}
        </select>
        <input placeholder="Модель" value={model.model} onChange={(e) => onChange({ ...model, model: e.target.value })} />
        <input
          placeholder="Base URL (для custom/local)"
          value={model.base_url}
          onChange={(e) => onChange({ ...model, base_url: e.target.value })}
        />
        <input
          type="password"
          placeholder="API-ключ"
          value={model.api_key}
          onChange={(e) => onChange({ ...model, api_key: e.target.value })}
        />
      </div>
      <div className="cap-row">
        {CAPABILITIES.map((cap) => (
          <button
            key={cap}
            className={`chip cap-chip ${model.capabilities.includes(cap) ? "chip-on" : ""}`}
            onClick={() => toggleCap(cap)}
          >
            {cap}
          </button>
        ))}
        {adding ? (
          <button className="btn btn-mini" onClick={onAdd}>Подключить</button>
        ) : (
          <button className="btn-mini" onClick={onRemove}>убрать</button>
        )}
      </div>
    </div>
  );
}

function Section({
  title, hint, action, children,
}: {
  title: string;
  hint?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="settings-section">
      <div className="section-head">
        <div className="section-title">{title}</div>
        {action}
      </div>
      {hint && <div className="hint">{hint}</div>}
      {children}
    </section>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
    </label>
  );
}

function Toggle({ label, value, onChange }: { label: string; value: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="toggle">
      <span>{label}</span>
      <button className={`switch ${value ? "on" : ""}`} onClick={() => onChange(!value)} role="switch" aria-checked={value}>
        <span className="knob" />
      </button>
    </label>
  );
}
