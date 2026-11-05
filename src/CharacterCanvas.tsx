// 2.5D-персонаж JARVIS: слоистый canvas-риг с параллаксом, дыханием, миганием,
// лёгким наклоном за курсором и плавными переходами эмоций (никаких резких
// переключений — все параметры интерполируются к целевым значениям).
// Стиль: геометричный, индустриально-минимальный, сдержанная палитра.

import { useEffect, useRef } from "react";
import type { Emotion } from "./types";

interface RigParams {
  // поза
  headTiltX: number; // -1..1 наклон к курсору
  headTiltY: number;
  breathe: number; // 0..1 фаза дыхания
  eyeOpen: number; // 0..1 (моргание)
  eyeNarrow: number; // 0..1 прищур (думает)
  eyeScan: number; // -1..1 «чтение» глазами
  mouthOpen: number; // 0..1 липсинк
  browLift: number; // 0..1
  glowCore: number; // 0..1 ядро-статус
  posture: number; // 0..1 собранность (исполнение)
  sorryDip: number; // 0..1 наклон головы вниз
  sleepFade: number; // 0..1 сон
  hue: number; // акцентный оттенок по эмоции
}

const PALETTE = {
  bg: "#0A0A0C",
  facePlate: "#17171B",
  faceEdge: "#222228",
  inner: "#101014",
  accent: "#8AB4C8", // единственный сдержанный акцент (сталь)
  accentWarm: "#C8A97E",
  eye: "#DEE6EA",
};

const EMOTION_TARGETS: Record<Emotion, Partial<RigParams> & { hue?: number }> = {
  idle: { eyeNarrow: 0.05, browLift: 0.1, posture: 0.4, glowCore: 0.25 },
  listening: { eyeNarrow: 0, browLift: 0.35, posture: 0.7, glowCore: 0.55, eyeScan: 0 },
  thinking: { eyeNarrow: 0.55, browLift: 0.25, posture: 0.6, glowCore: 0.45 },
  searching: { eyeNarrow: 0.3, eyeScan: 1, posture: 0.55, glowCore: 0.5 },
  speaking: { eyeNarrow: 0.05, browLift: 0.2, posture: 0.6, glowCore: 0.7 },
  executing: { eyeNarrow: 0.2, browLift: 0.3, posture: 0.95, glowCore: 0.85 },
  confused: { eyeNarrow: 0.2, browLift: 0.6, posture: 0.35, glowCore: 0.3, sorryDip: 0.25 },
  happy: { eyeNarrow: 0, browLift: 0.5, posture: 0.7, glowCore: 0.6, hue: 1 },
  sleep: { eyeOpen: 0.06, posture: 0.15, glowCore: 0.08, sleepFade: 1 },
  permission: { eyeNarrow: 0, browLift: 0.45, posture: 0.6, glowCore: 0.5 },
  sorry: { eyeNarrow: 0.25, browLift: 0.4, posture: 0.3, glowCore: 0.25, sorryDip: 0.45 },
};

export function CharacterCanvas({
  emotion,
  speakingLevel,
  micLevel,
  size = 260,
}: {
  emotion: Emotion;
  speakingLevel: number; // 0..1 амплитуда TTS
  micLevel: number; // 0..1 микрофон
  size?: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stateRef = useRef<RigParams>({
    headTiltX: 0, headTiltY: 0, breathe: 0, eyeOpen: 1, eyeNarrow: 0, eyeScan: 0,
    mouthOpen: 0, browLift: 0.1, glowCore: 0.25, posture: 0.4, sorryDip: 0, sleepFade: 0, hue: 0,
  });
  const emotionRef = useRef<Emotion>(emotion);
  const speakRef = useRef(0);
  const micRef = useRef(0);
  const mouseRef = useRef({ x: 0, y: 0 });
  const visibleRef = useRef(true);

  emotionRef.current = emotion;
  speakRef.current = speakingLevel;
  micRef.current = micLevel;

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
    let blinkAt = last + 2500 + Math.random() * 3000;
    let blinking = 0; // 0..1..0

    const onMove = (e: MouseEvent) => {
      const rect = canvas.getBoundingClientRect();
      const cx = rect.left + rect.width / 2;
      const cy = rect.top + rect.height / 2;
      mouseRef.current.x = Math.max(-1, Math.min(1, (e.clientX - cx) / (window.innerWidth / 2)));
      mouseRef.current.y = Math.max(-1, Math.min(1, (e.clientY - cy) / (window.innerHeight / 2)));
    };
    window.addEventListener("mousemove", onMove);

    const onVis = () => (visibleRef.current = !document.hidden);
    document.addEventListener("visibilitychange", onVis);

    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      if (!visibleRef.current) return; // пауза рендера в фоне

      const s = stateRef.current;
      const t = { ...stateRef.current };
      const em = emotionRef.current;
      const target = EMOTION_TARGETS[em] ?? EMOTION_TARGETS.idle;

      // --- целевые параметры
      t.eyeNarrow = target.eyeNarrow ?? 0.1;
      t.browLift = target.browLift ?? 0.1;
      t.posture = target.posture ?? 0.4;
      t.glowCore = target.glowCore ?? 0.3;
      t.eyeScan = em === "searching" ? Math.sin(now / 420) : 0;
      t.sorryDip = target.sorryDip ?? 0;
      t.sleepFade = target.sleepFade ?? 0;
      t.hue = target.hue ?? 0;
      t.headTiltX = mouseRef.current.x * 0.5;
      t.headTiltY = mouseRef.current.y * 0.35 + t.sorryDip * 0.6;
      t.breathe = (Math.sin(now / 1600) + 1) / 2;
      t.mouthOpen = em === "speaking" ? speakRef.current : 0;

      // Моргание.
      if (now > blinkAt && em !== "sleep") {
        blinking = Math.min(1, blinking + dt * 10);
        if (blinking >= 1) {
          blinking = 1;
          blinkAt = now + 2200 + Math.random() * 3500;
          setTimeout(() => (blinking = 0), 130);
        }
      }
      t.eyeOpen = Math.max(t.sleepFade > 0.5 ? 0.06 : 0.12, 1 - blinking - t.sleepFade * 0.9);

      // --- интерполяция (плавность, без резких переключений)
      const k = 1 - Math.pow(0.0015, dt); // ~экспоненциальное сглаживание
      for (const key of Object.keys(s) as (keyof RigParams)[]) {
        s[key] += (t[key] - s[key]) * k;
      }

      render(ctx, s, size, now, micRef.current);
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

function render(ctx: CanvasRenderingContext2D, s: RigParams, size: number, now: number, mic: number) {
  ctx.clearRect(0, 0, size, size);
  const cx = size / 2;
  const cy = size / 2;
  const R = size * 0.36;
  const accent = s.hue > 0.5 ? PALETTE.accentWarm : PALETTE.accent;

  // Параллакс-слои: задняя пластина смещается меньше лицевой.
  const tilt = s.headTiltX;
  const tiltY = s.headTiltY;
  const breath = (s.breathe - 0.5) * size * 0.012;

  // --- слой 1: задняя «техническая» пластина
  drawPlate(ctx, cx - tilt * 6, cy + tiltY * 4 + breath, R * 1.22, PALETTE.bg, "#1B1B21", 1);
  // тонкие засечки по окружности (индустриальный мотив)
  ctx.save();
  ctx.translate(cx - tilt * 6, cy + tiltY * 4 + breath);
  ctx.strokeStyle = "rgba(255,255,255,0.06)";
  ctx.lineWidth = 1;
  for (let i = 0; i < 24; i++) {
    const a = (i / 24) * Math.PI * 2 + now / 60000;
    ctx.beginPath();
    ctx.moveTo(Math.cos(a) * R * 1.14, Math.sin(a) * R * 1.14);
    ctx.lineTo(Math.cos(a) * R * 1.2, Math.sin(a) * R * 1.2);
    ctx.stroke();
  }
  ctx.restore();

  // --- слой 2: корпус-голова (слегка вытянутый скруглённый прямоугольник)
  const bodyY = cy + tiltY * 10 + breath * 1.4 + s.sorryDip * 8;
  const bodyX = cx + tilt * 12;
  roundRect(ctx, bodyX - R * 0.98, bodyY - R * 1.02, R * 1.96, R * 2.04, R * 0.42);
  ctx.fillStyle = PALETTE.facePlate;
  ctx.fill();
  ctx.strokeStyle = PALETTE.faceEdge;
  ctx.lineWidth = 1;
  ctx.stroke();

  // Внутренняя панель-«экран лица»
  const fx = bodyX + tilt * 5;
  const fy = bodyY + tiltY * 4;
  roundRect(ctx, fx - R * 0.76, fy - R * 0.72, R * 1.52, R * 1.2, R * 0.3);
  ctx.fillStyle = PALETTE.inner;
  ctx.fill();
  ctx.strokeStyle = "rgba(255,255,255,0.05)";
  ctx.stroke();

  // --- ядро-статус под лицом (спокойный, не «неоновый»)
  const coreR = R * 0.1 + s.glowCore * R * 0.06;
  const coreY = fy + R * 0.62;
  const grad = ctx.createRadialGradient(fx, coreY, 0, fx, coreY, coreR * 2.4);
  grad.addColorStop(0, accent);
  grad.addColorStop(0.45, hexAlpha(accent, 0.28 * (0.5 + s.glowCore * 0.5)));
  grad.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = grad;
  ctx.beginPath();
  ctx.arc(fx, coreY, coreR * 2.4, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = accent;
  ctx.beginPath();
  ctx.arc(fx, coreY, coreR, 0, Math.PI * 2);
  ctx.fill();
  // пульс ядра при прослушивании — от микрофона
  if (mic > 0.02) {
    ctx.strokeStyle = hexAlpha(accent, 0.35);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.arc(fx, coreY, coreR * (1.6 + mic * 1.8), 0, Math.PI * 2);
    ctx.stroke();
  }

  // --- глаза: узкие скруглённые «полосы» (Nothing OS-like)
  const eyeW = R * 0.34;
  const eyeH = R * 0.05 + s.eyeNarrow * R * 0.028 + s.eyeOpen * R * 0.075;
  const eyeLX = fx - R * 0.36 + s.eyeScan * R * 0.07;
  const eyeRX = fx + R * 0.36 + s.eyeScan * R * 0.07;
  const eyeY = fy - R * 0.12 + tiltY * 3;
  for (const ex of [eyeLX, eyeRX]) {
    const openH = Math.max(1.2, eyeH * s.eyeOpen);
    roundRect(ctx, ex - eyeW / 2, eyeY - openH / 2, eyeW, openH, openH / 2);
    ctx.fillStyle = PALETTE.eye;
    ctx.fill();
  }
  // брови-штрихи (сдержанные, с лёгким подъёмом)
  const browY = eyeY - R * 0.2 - s.browLift * R * 0.07;
  ctx.strokeStyle = "rgba(255,255,255,0.22)";
  ctx.lineWidth = 2;
  for (const ex of [eyeLX, eyeRX]) {
    ctx.beginPath();
    ctx.moveTo(ex - eyeW * 0.45, browY);
    ctx.lineTo(ex + eyeW * 0.45, browY - s.browLift * 3);
    ctx.stroke();
  }

  // --- рот: тонкая линия, раскрывается при липсинке
  const mouthW = R * 0.34;
  const mouthY = fy + R * 0.32;
  const open = s.mouthOpen * R * 0.16;
  ctx.fillStyle = PALETTE.eye;
  if (open > 0.6) {
    roundRect(ctx, fx - mouthW / 2 + s.eyeScan * R * 0.05, mouthY - open / 2, mouthW, open, Math.min(open / 2, 6));
  } else {
    roundRect(ctx, fx - mouthW / 2 + s.eyeScan * R * 0.05, mouthY - 1, mouthW, 2, 1);
  }
  ctx.fill();

  // --- боковые «уши»-модули (усиливают 2.5D-ощущение)
  ctx.fillStyle = "#141418";
  ctx.strokeStyle = PALETTE.faceEdge;
  for (const side of [-1, 1]) {
    const ex = bodyX + side * (R * 1.02);
    roundRect(ctx, ex - 4 + tilt * side * -2, bodyY - R * 0.3, 8, R * 0.6, 4);
    ctx.fill();
    ctx.stroke();
  }

  // --- виньетка-тень снизу для объёма
  const vg = ctx.createLinearGradient(0, size * 0.7, 0, size);
  vg.addColorStop(0, "rgba(0,0,0,0)");
  vg.addColorStop(1, "rgba(0,0,0,0.5)");
  ctx.fillStyle = vg;
  ctx.fillRect(0, size * 0.7, size, size * 0.3);
}

function drawPlate(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, fill: string, edge: string, _depth: number) {
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.strokeStyle = edge;
  ctx.lineWidth = 1;
  ctx.stroke();
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.arcTo(x + w, y, x + w, y + h, rr);
  ctx.arcTo(x + w, y + h, x, y + h, rr);
  ctx.arcTo(x, y + h, x, y, rr);
  ctx.arcTo(x, y, x + w, y, rr);
  ctx.closePath();
}

function hexAlpha(hex: string, a: number): string {
  const n = parseInt(hex.slice(1), 16);
  const r = (n >> 16) & 0xff;
  const g = (n >> 8) & 0xff;
  const b = n & 0xff;
  return `rgba(${r},${g},${b},${a})`;
}
