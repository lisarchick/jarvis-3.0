// Спрайтовый 2.5D-персонаж: кадры эмоций из character-пакета (4 эмоции ×
// молчит/говорит) + непрерывная процедурная анимация поверх: дыхание, покачивание,
// параллакс за курсором, липсинк-каденцию по амплитуде TTS. Переходы эмоций —
// плавный кросс-фейд, рот — чередованием говорящих/тихих кадров (2–4 открывания/с).
// Рендер останавливается, когда вкладка скрыта.

import { useEffect, useRef } from "react";
import { loadManifest, type CharacterManifest } from "./characters";
import type { Emotion } from "./types";

// Эмоция агента → эмоция кадров шита
function frameEmotion(agent: Emotion, sentence: string | null): string {
  if (sentence) {
    if (sentence === "happy") return "happy";
    if (sentence === "sad") return "sad";
    if (sentence === "angry") return "angry";
    return "neutral";
  }
  switch (agent) {
    case "happy":
      return "happy";
    case "confused":
    case "sorry":
    case "sleep":
      return "sad";
    default:
      return "neutral";
  }
}

export function SpriteCharacter({
  characterId,
  emotion,
  sentenceEmotion,
  speaking,
  speakLevel,
  size = 240,
}: {
  characterId: string;
  emotion: Emotion;
  sentenceEmotion: string | null;
  speaking: boolean;
  speakLevel: number;
  size?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const manifestRef = useRef<CharacterManifest | null>(null);
  const imgsRef = useRef<Map<string, HTMLImageElement>>(new Map());
  const liveRef = useRef({ emotion, sentenceEmotion, speaking, speakLevel });
  liveRef.current = { emotion, sentenceEmotion, speaking, speakLevel };
  const mouseRef = useRef({ x: 0, y: 0 });

  useEffect(() => {
    let dead = false;
    imgsRef.current = new Map();
    manifestRef.current = null;
    (async () => {
      const m = await loadManifest(characterId);
      if (!m || dead) return;
      manifestRef.current = m;
      const entries = Object.entries(m.frames);
      await Promise.all(
        entries.map(
          ([key, file]) =>
            new Promise<void>((res) => {
              const im = new Image();
              im.onload = () => {
                if (!dead) imgsRef.current.set(key, im);
                res();
              };
              im.onerror = () => res();
              im.src = `characters/${characterId}/${file}`;
            }),
        ),
      );
    })();
    return () => {
      dead = true;
    };
  }, [characterId]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    ctx.scale(dpr, dpr);

    let raf = 0;
    let last = performance.now();
    let visible = !document.hidden;

    // липсинк-каденция
    let mouthOpen = false;
    let mouthUntil = 0;
    // кросс-фейд эмоции
    let curKey = "";
    let prevKey = "";
    let fade = 0; // 1 → 0

    const onMove = (e: MouseEvent) => {
      const rect = canvas.getBoundingClientRect();
      const cx = rect.left + rect.width / 2;
      mouseRef.current.x = Math.max(-1, Math.min(1, (e.clientX - cx) / (window.innerWidth / 2)));
    };
    const onVis = () => (visible = !document.hidden);
    window.addEventListener("mousemove", onMove);
    document.addEventListener("visibilitychange", onVis);

    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      if (!visible) return;

      const { emotion: em, sentenceEmotion: se, speaking: sp, speakLevel: lvl } = liveRef.current;
      const emo = frameEmotion(em, se);

      // --- липсинк: открывание рта по амплитуде, не чаще ~4/с
      if (sp && now > mouthUntil) {
        mouthOpen = !mouthOpen;
        if (mouthOpen) {
          const openMs = 130 + Math.min(1, lvl) * 150 + Math.random() * 60;
          mouthUntil = now + openMs;
        } else {
          mouthUntil = now + 100 + Math.random() * 90;
        }
      }
      if (!sp) mouthOpen = false;

      const key = `${emo}_${sp && mouthOpen ? "talk" : "idle"}`;
      if (key !== curKey) {
        if (curKey) {
          prevKey = curKey;
          fade = emo === frameEmotion(em, null) && !sp ? 0.55 : 1; // рот — быстрый фейд
        }
        curKey = key;
      }
      fade = Math.max(0, fade - dt / 0.24);

      ctx.clearRect(0, 0, size, size);

      const cur = imgsRef.current.get(curKey);
      if (!cur) {
        // кадры ещё грузятся — тихий плейсхолдер
        ctx.fillStyle = "#121215";
        ctx.beginPath();
        ctx.arc(size / 2, size / 2, size * 0.3, 0, Math.PI * 2);
        ctx.fill();
        return;
      }

      // --- процедурная 2.5D-жизнь
      const breath = Math.sin(now / 1700);
      const sway = Math.sin(now / 2900) * 0.012; // рад
      const tilt = mouseRef.current.x * 0.035;
      const px = mouseRef.current.x * 7 + Math.sin(now / 2300) * 2;
      const py = breath * 1.6 + Math.cos(now / 3100) * 1.2;
      const scaleY = 1 + breath * 0.006;

      const ih = size;
      const iw = (cur.width / cur.height) * ih;
      const cx = size / 2 + px;
      const cy = size / 2 + py;

      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(sway + tilt);
      ctx.scale(1 + breath * 0.004, scaleY);

      const prev = imgsRef.current.get(prevKey);
      if (prev && fade > 0.01) {
        ctx.globalAlpha = fade;
        const pw = (prev.width / prev.height) * ih;
        ctx.drawImage(prev, -pw / 2, -ih / 2, pw, ih);
        ctx.globalAlpha = 1;
      }
      ctx.drawImage(cur, -iw / 2, -ih / 2, iw, ih);

      // сон — приглушение
      if (em === "sleep") {
        ctx.globalAlpha = 0.35;
        ctx.fillStyle = "#09090b";
        ctx.fillRect(-iw / 2, -ih / 2, iw, ih);
        ctx.globalAlpha = 1;
      }
      ctx.restore();
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("mousemove", onMove);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [size]);

  return <canvas ref={canvasRef} style={{ width: size, height: size }} className="character-canvas" />;
}
