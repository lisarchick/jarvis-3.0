// JARVIS 3.0 — каркас приложения: frameless-тайтлбар, панель персонажа,
// поток диалога (текст, карточки инструментов, чипы источников), лента действий,
// статус-полоса, диалоги подтверждения/разрешений, режим оверлея.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CharacterCanvas } from "./CharacterCanvas";
import { VoiceEngine, base64Wav, stripWake } from "./audio";
import type {
  ActionEntry, AgentStateEvent, ChatMessage, Confirmation, Config,
  Source, StreamChunk, ToolEvent,
} from "./types";
import { SettingsPanel } from "./SettingsPanel";
import { Overlay } from "./Overlay";

const STATUS_LABEL: Record<string, string> = {
  idle: "Ожидание",
  listening: "Слушаю",
  thinking: "Думаю",
  searching: "Ищу",
  executing: "Выполняю",
  speaking: "Говорю",
};

export default function App() {
  const isOverlay = useMemo(
    () => window.location.hash.includes("overlay"),
    [],
  );
  if (isOverlay) return <Overlay />;
  return <Main />;
}

function Main() {
  const win = useMemo(() => getCurrentWindow(), []);
  const [config, setConfig] = useState<Config | null>(null);
  const [state, setState] = useState<AgentStateEvent>({ status: "idle", emotion: "idle", detail: "" });
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [streamText, setStreamText] = useState("");
  const [sources, setSources] = useState<Source[]>([]);
  const [tools, setTools] = useState<ToolEvent[]>([]);
  const [log, setLog] = useState<ActionEntry[]>([]);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [micLevel, setMicLevel] = useState(0);
  const [listeningOn, setListeningOn] = useState(false);
  const [micDenied, setMicDenied] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [permissionPrompt, setPermissionPrompt] = useState<string | null>(null);
  const [onboarded, setOnboarded] = useState(true);
  const [speakLevel, setSpeakLevel] = useState(0);

  const engineRef = useRef<VoiceEngine | null>(null);
  const followUntilRef = useRef(0); // окно непрерывного диалога, timestamp
  const chatEndRef = useRef<HTMLDivElement | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;

  // ------------------------------------------------------------- инициализация

  useEffect(() => {
    (async () => {
      try {
        const cfg = await invoke<Config>("get_config");
        setConfig(cfg);
        // Первый запуск: пригласить выдать разрешения (ничего не сканируем до согласия).
        if (!cfg.permissions || Object.keys(cfg.permissions).length === 0) {
          setOnboarded(false);
        }
        const logEntries = await invoke<ActionEntry[]>("action_log", { n: 60 });
        setLog(logEntries);
      } catch (e) {
        console.error(e);
      }
    })();
  }, []);

  // Подписки на события бэкенда.
  useEffect(() => {
    const unsubs: Promise<() => void>[] = [
      listen<AgentStateEvent>("agent://state", (e) => {
        setState(e.payload);
        if (e.payload.status === "speaking") setSpeakLevel(0.4);
        if (e.payload.status === "idle") setSpeakLevel(0);
      }),
      listen<StreamChunk>("agent://delta", (e) => {
        const p = e.payload;
        if (p.kind === "delta") {
          setStreaming(true);
          setStreamText((prev) => prev + p.text);
        } else if (p.kind === "done") {
          setStreaming(false);
          const finalText = streamTextRef.current;
          if (finalText.trim()) {
            setMessages((prev) => [...prev, { role: "assistant", content: finalText }]);
          }
          setStreamText("");
          setFollowWindow();
        } else if (p.kind === "error") {
          setStreaming(false);
          const finalText = streamTextRef.current;
          if (finalText.trim()) setMessages((prev) => [...prev, { role: "assistant", content: finalText }]);
          setMessages((prev) => [...prev, { role: "assistant", content: p.message }]);
          setStreamText("");
          setFollowWindow();
        }
      }),
      listen<ToolEvent>("agent://tool", (e) => {
        setTools((prev) => {
          const without = prev.filter((t) => t.id !== e.payload.id);
          return [...without, e.payload].slice(-30);
        });
        // Источники поиска — в чипы.
        if (e.payload.tool === "web_search" && e.payload.type === "ok") {
          const argsSources = (e.payload.args as { sources?: Source[] }).sources;
          if (Array.isArray(argsSources)) setSources(argsSources.slice(0, 6));
        }
      }),
      listen<Confirmation>("agent://confirm", (e) => setConfirmation(e.payload)),
      listen<string>("agent://permission-needed", (e) => setPermissionPrompt(e.payload)),
      listen<{ name: string }>("agent://timer", (e) => {
        setMessages((prev) => [...prev, { role: "assistant", content: `Таймер «${e.payload.name}» сработал.` }]);
      }),
      listen("agent://screen-capture", () => {
        const card: ToolEvent = {
          id: `cap-${Date.now()}`, tool: "look_at_screen", title: "Снимок экрана",
          args: {}, type: "ok", detail: "одноразовый снимок",
        };
        setTools((prev) => [...prev, card].slice(-30));
      }),
      listen("jarvis-barge-in", () => {
        invoke("stop_speaking").catch(() => {});
      }),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()));
    };
  }, []);

  // Стрим-буфер для финализации.
  const streamTextRef = useRef("");
  useEffect(() => {
    streamTextRef.current = streamText;
  }, [streamText]);

  // iOS/mobile: озвучка через speechSynthesis webview (на desktop играет Rust).
  useEffect(() => {
    const isMobile = /iPhone|iPad|Android/i.test(navigator.userAgent);
    if (!isMobile || !("speechSynthesis" in window)) return;
    const speak = (text: string) => {
      const u = new SpeechSynthesisUtterance(text);
      u.lang = /[a-z]/i.test(text.slice(0, 60)) && !/[а-яё]/i.test(text.slice(0, 60)) ? "en-US" : "ru-RU";
      window.speechSynthesis.speak(u);
    };
    const unsubs = [
      listen<string>("agent://tts", (e) => speak(e.payload)),
      listen("agent://tts-stop", () => window.speechSynthesis.cancel()),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()));
    };
  }, []);

  const setFollowWindow = useCallback(() => {
    followUntilRef.current = Date.now() + 8000;
  }, []);

  // Автоскролл чата.
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages.length, streamText]);

  // Псевдо-липсинк во время speaking: лёгкая пульсация.
  useEffect(() => {
    if (state.status !== "speaking") return;
    const id = setInterval(() => setSpeakLevel(0.15 + Math.random() * 0.5), 140);
    return () => clearInterval(id);
  }, [state.status]);

  // ------------------------------------------------------------- голосовой движок

  const ensureEngine = useCallback(async () => {
    if (engineRef.current?.active) return engineRef.current;
    const engine = new VoiceEngine({
      onPhrase: async (wav) => {
        // Пока ассистент говорит — фразы не распознаём (barge-in уже сработал).
        if (stateRef.current.status === "speaking") return;
        try {
          const text = await invoke<string>("transcribe", { wavB64: base64Wav(wav) });
          if (!text.trim()) return;
          handleTranscript(text.trim());
        } catch (e) {
          console.error("stt", e);
        }
      },
      onLevel: (level, speaking) => {
        setMicLevel(level);
        if (speaking && stateRef.current.status === "idle") {
          // микро-статус: видим, что голос ловится
        }
      },
      onState: setListeningOn,
    });
    const ok = await engine.start();
    if (!ok) {
      setMicDenied(true);
      return null;
    }
    engineRef.current = engine;
    return engine;
  }, []);

  // Подсказываем движку, когда ассистент говорит (для barge-in).
  useEffect(() => {
    engineRef.current?.setAssistantSpeaking(state.status === "speaking");
  }, [state.status]);

  const handleTranscript = useCallback(
    (text: string) => {
      const parsed = stripWake(text);
      const inFollowWindow = Date.now() < followUntilRef.current;
      let command: string | null = null;
      if (parsed?.kind === "stop") {
        invoke("stop_speaking").catch(() => {});
        followUntilRef.current = 0;
        return;
      }
      if (parsed?.kind === "command") {
        command = parsed.text;
      } else if (parsed?.kind === "listen") {
        followUntilRef.current = Date.now() + 8000;
        return; // просто «джарвис» — слушаем следующую фразу
      } else if (inFollowWindow) {
        command = text.toLowerCase().replace("ё", "е");
      }
      if (!command) return;
      setMessages((prev) => [...prev, { role: "user", content: command }]);
      setSources([]);
      setFollowWindow();
      invoke("ask", { text: command }).catch((e) => console.error(e));
    },
    [],
  );

  // ------------------------------------------------------------- ввод текстом

  const sendText = useCallback((text: string) => {
    const t = text.trim();
    if (!t) return;
    if (t.toLowerCase().replace("ё", "е") === "стоп") {
      invoke("stop_speaking").catch(() => {});
      return;
    }
    setMessages((prev) => [...prev, { role: "user", content: t }]);
    setSources([]);
    setFollowWindow();
    invoke("ask", { text: t }).catch((e) => console.error(e));
  }, []);

  // ------------------------------------------------------------- onboarding

  const grantFirstPermissions = useCallback(async (allow: boolean) => {
    try {
      if (allow) {
        await invoke("set_permission", { key: "app_scan", value: true });
        await invoke("set_permission", { key: "open_close_apps", value: true });
        await invoke("set_permission", { key: "microphone", value: allow });
        await invoke("set_permission", { key: "file_ops", value: true });
        await invoke("scan_apps").catch(() => {});
      }
      setOnboarded(true);
      setConfig(await invoke<Config>("get_config"));
      if (allow) await ensureEngine();
    } catch (e) {
      console.error(e);
    }
  }, [ensureEngine]);

  // ------------------------------------------------------------- рендер

  return (
    <div className="app">
      <TitleBar win={win} onSettings={() => setSettingsOpen(true)} />
      <div className="app-body">
        {/* Левая колонка: персонаж + статус + голос */}
        <aside className="panel character-panel">
          <div className="character-wrap">
            <CharacterCanvas
              emotion={state.emotion}
              speakingLevel={speakLevel}
              micLevel={micLevel}
              size={248}
            />
          </div>
          <div className={`status-line status-${state.status}`}>
            <span className="status-dot" />
            <span className="status-text">{STATUS_LABEL[state.status] ?? state.status}</span>
          </div>
          <div className="status-detail mono">{state.detail}</div>
          <div className="mic-row">
            <button
              className={`btn-mic ${listeningOn ? "on" : ""}`}
              onClick={async () => {
                if (engineRef.current?.active) {
                  engineRef.current.stop();
                } else {
                  await ensureEngine();
                }
              }}
              title={listeningOn ? "Выключить микрофон" : "Включить микрофон"}
            >
              {listeningOn ? "Микрофон включён" : "Включить микрофон"}
            </button>
          </div>
          {micDenied && (
            <div className="mic-denied">
              Микрофон недоступен. Проверь разрешение Windows (Параметры → Конфиденциальность → Микрофон).
            </div>
          )}
          <div className="sources-block">
            {sources.length > 0 && (
              <>
                <div className="block-label">Источники</div>
                <div className="source-chips">
                  {sources.map((s) => (
                    <a key={s.url} href={s.url} target="_blank" rel="noreferrer" className="chip">
                      {s.host}
                    </a>
                  ))}
                </div>
              </>
            )}
          </div>
        </aside>

        {/* Центр: диалог */}
        <main className="panel chat-panel">
          <div className="chat-scroll">
            {messages.length === 0 && !streaming && (
              <div className="chat-empty">
                <div className="chat-empty-title">Скажи «Джарвис» — или напиши здесь.</div>
                <div className="chat-empty-hint">
                  Спроси что угодно · «открой калькулятор» · «поставь таймер на 10 минут» · «включи спокойную музыку» · «посмотри на экран»
                </div>
              </div>
            )}
            {messages.map((m, i) => (
              <div key={i} className={`msg msg-${m.role === "user" ? "user" : "assistant"}`}>
                <div className="msg-role">{m.role === "user" ? "Ты" : "JARVIS"}</div>
                <div className="msg-text">{m.content}</div>
              </div>
            ))}
            {streaming && (
              <div className="msg msg-assistant">
                <div className="msg-role">JARVIS</div>
                <div className="msg-text">
                  {streamText}
                  <span className="caret" />
                </div>
              </div>
            )}
            <div ref={chatEndRef} />
          </div>
          {/* Карточки инструментов последнего хода */}
          {tools.length > 0 && (
            <div className="tool-strip">
              {tools.slice(-4).map((t) => (
                <div key={t.id} className={`tool-chip tool-${t.type}`}>
                  <span className="tool-name">{t.title}</span>
                  <span className={`tool-status st-${t.type}`}>
                    {t.type === "ok" ? "готово" : t.type === "start" ? "…" : "ошибка"}
                  </span>
                </div>
              ))}
            </div>
          )}
          <Composer onSend={sendText} disabled={streaming} />
        </main>

        {/* Правая колонка: лента действий */}
        <aside className="panel log-panel">
          <div className="block-label">Журнал действий</div>
          <div className="log-scroll">
            {log.length === 0 && <div className="log-empty">Пока пусто — все действия появятся здесь.</div>}
            {log.map((l, i) => (
              <div key={i} className="log-row">
                <span className={`log-status ls-${l.status}`} />
                <span className="log-tool mono">{l.tool}</span>
                <span className="log-detail">{l.detail || l.args}</span>
              </div>
            ))}
          </div>
        </aside>
      </div>

      {/* Статус-полоса */}
      <footer className="statusbar">
        <span className={`sb-dot sb-${state.status}`} />
        <span className="sb-state">{STATUS_LABEL[state.status] ?? state.status}</span>
        <span className="sb-sep" />
        <span className="mono">
          {config ? `${config.llm_provider}${config.llm_model ? ` · ${config.llm_model}` : ""}` : "…"}
        </span>
        <span className="sb-sep" />
        <span className="mono sb-hotkey">{config?.overlay_hotkey ?? "Alt+Space"} — оверлей</span>
        <span className="sb-flex" />
        <button className="sb-btn" onClick={() => setSettingsOpen(true)}>Настройки</button>
      </footer>

      {/* Диалог подтверждения (риск-действия) */}
      {confirmation && (
        <Dialog>
          <div className="dialog-title">{confirmation.title}</div>
          <div className="dialog-text">{confirmation.detail}</div>
          <div className="dialog-row">
            <button
              className="btn btn-primary"
              autoFocus
              onClick={async () => {
                await invoke("confirm", { id: confirmation.id, approved: true });
                setConfirmation(null);
              }}
            >
              Да (Enter)
            </button>
            <button
              className="btn"
              onClick={async () => {
                await invoke("confirm", { id: confirmation.id, approved: false });
                setConfirmation(null);
              }}
            >
              Отмена (Esc)
            </button>
          </div>
        </Dialog>
      )}

      {/* Первый запуск: разрешения */}
      {!onboarded && (
        <Dialog>
          <div className="dialog-title">Разреши JARVIS работать с компьютером</div>
          <div className="dialog-text">
            JARVIS ничего не сканирует без согласия. Разрешить: сканировать список установленных
            программ (локально), открывать/закрывать программы, микрофон, файловые операции в
            пользовательских папках (удаление — только в корзину и с подтверждением).
            Всё остальное — включается по мере надобности в настройках.
          </div>
          <div className="dialog-row">
            <button className="btn btn-primary" autoFocus onClick={() => grantFirstPermissions(true)}>
              Разрешить и начать
            </button>
            <button className="btn" onClick={() => grantFirstPermissions(false)}>
              Позже (чат и поиск — и так работают)
            </button>
          </div>
        </Dialog>
      )}

      {/* Нужно разрешение (запрошено действием) */}
      {permissionPrompt && onboarded && (
        <Dialog>
          <div className="dialog-title">Нужно разрешение</div>
          <div className="dialog-text">
            Для этого действия нет разрешения: <b>{permissionPrompt}</b>. Открыть настройки,
            чтобы включить его?
          </div>
          <div className="dialog-row">
            <button
              className="btn btn-primary"
              autoFocus
              onClick={async () => {
                setPermissionPrompt(null);
                setSettingsOpen(true);
              }}
            >
              Открыть настройки (Enter)
            </button>
            <button className="btn" onClick={() => setPermissionPrompt(null)}>
              Отмена (Esc)
            </button>
          </div>
        </Dialog>
      )}

      {settingsOpen && (
        <SettingsPanel
          onClose={() => setSettingsOpen(false)}
          onConfigSaved={async (cfg) => {
            setConfig(cfg);
            const entries = await invoke<ActionEntry[]>("action_log", { n: 60 }).catch(() => []);
            setLog(entries);
          }}
        />
      )}
    </div>
  );
}

// ------------------------------------------------------------------ тайтлбар

function TitleBar({ win, onSettings }: { win: ReturnType<typeof getCurrentWindow>; onSettings: () => void }) {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    const p = win.onResized(async () => setMaximized(await win.isMaximized()));
    return () => {
      p.then((f) => f());
    };
  }, [win]);
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="tb-title" data-tauri-drag-region>
        <span className="tb-logo">J</span> JARVIS
        <span className="tb-sub mono">3.0</span>
      </div>
      <div className="tb-actions">
        <button className="tb-btn tb-settings" title="Настройки" onClick={onSettings} />
        <button className="tb-btn" title="Свернуть" onClick={() => win.minimize()} />
        <button
          className={`tb-btn ${maximized ? "tb-restore" : "tb-max"}`}
          title={maximized ? "Восстановить" : "Развернуть"}
          onClick={async () => (maximized ? await win.unmaximize() : await win.maximize())}
        />
        <button className="tb-btn tb-close" title="Свернуть в трей" onClick={() => win.hide()} />
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ поле ввода

function Composer({ onSend, disabled }: { onSend: (t: string) => void; disabled: boolean }) {
  const [text, setText] = useState("");
  const ref = useRef<HTMLTextAreaElement | null>(null);
  const send = () => {
    if (!text.trim()) return;
    onSend(text);
    setText("");
  };
  return (
    <div className="composer">
      <textarea
        ref={ref}
        rows={1}
        placeholder="Напиши JARVIS… (Enter — отправить, Shift+Enter — новая строка)"
        value={text}
        disabled={disabled}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          }
        }}
      />
      <button className="btn-send" onClick={send} disabled={disabled || !text.trim()} title="Отправить" />
    </div>
  );
}

// ------------------------------------------------------------------ диалог

function Dialog({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        const cancel = document.querySelector<HTMLButtonElement>(".dialog-row .btn:not(.btn-primary)");
        cancel?.click();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  return (
    <div className="dialog-backdrop">
      <div className="dialog">{children}</div>
    </div>
  );
}
