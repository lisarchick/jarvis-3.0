// Компактный always-on-top оверлей: персонаж, последняя реплика, быстрый ввод,
// кнопка диктофона. Открывается глобальной клавишей (по умолчанию Alt+Space).

import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CharacterCanvas } from "./CharacterCanvas";
import type { AgentStateEvent, StreamChunk } from "./types";

export function Overlay() {
  const win = useMemo(() => getCurrentWindow(), []);
  const [state, setState] = useState<AgentStateEvent>({ status: "idle", emotion: "idle", detail: "" });
  const [lastReply, setLastReply] = useState("");
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const replyRef = useRef("");

  useEffect(() => {
    const unsubs = [
      listen<AgentStateEvent>("agent://state", (e) => setState(e.payload)),
      listen<StreamChunk>("agent://delta", (e) => {
        const p = e.payload;
        if (p.kind === "delta") {
          setBusy(true);
          replyRef.current += p.text;
          // В оверлее держим только хвост последней реплики.
          setLastReply(replyRef.current.slice(-180));
        } else if (p.kind === "done" || p.kind === "error") {
          setBusy(false);
          if (p.kind === "error") setLastReply(p.message.slice(-180));
        }
      }),
    ];
    return () => {
      unsubs.forEach((u) => u.then((f) => f()));
    };
  }, []);

  const send = () => {
    const t = input.trim();
    if (!t) return;
    replyRef.current = "";
    setInput("");
    invoke("ask", { text: t }).catch(() => {});
  };

  return (
    <div className="overlay" data-tauri-drag-region>
      <div className="overlay-side">
        <CharacterCanvas emotion={state.emotion} speakingLevel={state.status === "speaking" ? 0.4 : 0} micLevel={0} size={108} />
      </div>
      <div className="overlay-main">
        <div className="overlay-status mono">{state.detail || statusLabel(state.status)}</div>
        <div className="overlay-reply">{lastReply || "Скажи «Джарвис» — или набери здесь."}</div>
        <div className="overlay-row">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && send()}
            placeholder={busy ? "Думаю…" : "Спроси или прикажи…"}
            disabled={busy}
          />
          <button className="overlay-stop" title="Замолчать" onClick={() => invoke("stop_speaking").catch(() => {})} />
          <button className="overlay-hide" title="Скрыть (Esc)" onClick={() => win.hide()} />
        </div>
      </div>
    </div>
  );
}

function statusLabel(s: string): string {
  return (
    {
      idle: "Ожидание",
      listening: "Слушаю",
      thinking: "Думаю",
      searching: "Ищу",
      executing: "Выполняю",
      speaking: "Говорю",
    } as Record<string, string>
  )[s] ?? s;
}
