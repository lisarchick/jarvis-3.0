// Голосовой вход: захват микрофона, энергийный VAD с адаптивным порогом шума
// (порт логики v1), сегментация фраз, wake word, barge-in, кодирование 16 кГц WAV.
// Экспорт в JS — без внешних зависимостей.

import type { AgentStateEvent } from "./types";

const SAMPLE_RATE = 16000;
const PRE_ROLL_BLOCKS = 5; // 0.5 c до начала речи
const SILENCE_BLOCKS = 7; // 0.7 c тишины = конец фразы
const MAX_BLOCKS = 150; // 15 c максимум
const MIN_BLOCKS = 3; // короче 0.3 c — шум

export interface VoiceEngineEvents {
  onPhrase: (wav: ArrayBuffer, rmsPeak: number) => void;
  onLevel: (level: number, speakingDetected: boolean) => void;
  onState: (listening: boolean) => void;
}

export class VoiceEngine {
  private ctx: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private worklet: AudioWorkletNode | null = null;
  private running = false;

  // VAD-состояние
  private noise = 0.008;
  private phrase: Int16Array[] = [];
  private phraseLen = 0;
  private preRoll: Int16Array[] = [];
  private inSpeech = false;
  private silenceRun = 0;
  private blocksRun = 0;

  // Barge-in: ассистент говорит (из agent://state), и пользователь начал речь.
  private assistantSpeaking = false;
  private bargeFired = false;

  constructor(private events: VoiceEngineEvents) {}

  setAssistantSpeaking(speaking: boolean) {
    this.assistantSpeaking = speaking;
    if (speaking) this.bargeFired = false;
  }

  get active() {
    return this.running;
  }

  async start(): Promise<boolean> {
    if (this.running) return true;
    try {
      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
    } catch {
      return false;
    }
    this.ctx = new AudioContext({ sampleRate: SAMPLE_RATE });
    if (this.ctx.state === "suspended") await this.ctx.resume();
    const url = URL.createObjectURL(new Blob([VAD_WORKLET_SRC], { type: "application/javascript" }));
    await this.ctx.audioWorklet.addModule(url);
    URL.revokeObjectURL(url);

    const source = this.ctx.createMediaStreamSource(this.stream);
    this.worklet = new AudioWorkletNode(this.ctx, "vad-proc", {
      numberOfInputs: 1,
      numberOfOutputs: 0,
    });
    this.worklet.port.onmessage = (e) => this.onBlock(e.data as Int16Array);
    source.connect(this.worklet);

    this.running = true;
    this.events.onState(true);
    return true;
  }

  stop() {
    this.running = false;
    this.worklet?.disconnect();
    this.worklet = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    this.ctx?.close().catch(() => {});
    this.ctx = null;
    this.reset();
    this.events.onState(false);
  }

  private reset() {
    this.phrase = [];
    this.phraseLen = 0;
    this.inSpeech = false;
    this.silenceRun = 0;
    this.blocksRun = 0;
    this.noise = 0.008;
  }

  private onBlock(block: Int16Array) {
    if (!this.running) return;
    // RMS нормализованного блока (int16 → [-1,1]).
    let sum = 0;
    for (let i = 0; i < block.length; i++) {
      const v = block[i] / 32768;
      sum += v * v;
    }
    const rms = Math.sqrt(sum / block.length);

    // Адаптивный уровень шума (как в v1: 0.95*noise + 0.05*rms).
    const thr = Math.max(0.009, this.noise * 2.5);
    const isSpeech = rms >= thr;
    if (!isSpeech && rms < thr * 0.7) {
      this.noise = this.noise * 0.95 + rms * 0.05;
    }
    this.events.onLevel(Math.min(1, rms / 0.12), isSpeech);

    // Barge-in: ассистент говорит, а голос громче порога.
    if (this.assistantSpeaking && rms >= thr * 1.4 && !this.bargeFired) {
      this.bargeFired = true;
      window.dispatchEvent(new CustomEvent("jarvis-barge-in"));
    }

    if (this.inSpeech) {
      this.phrase.push(block);
      this.phraseLen += block.length;
      this.blocksRun++;
      if (isSpeech) {
        this.silenceRun = 0;
      } else {
        this.silenceRun++;
      }
      const done = this.silenceRun >= SILENCE_BLOCKS || this.blocksRun >= MAX_BLOCKS;
      if (done) {
        const total = this.phraseLen;
        if (this.blocksRun - this.silenceRun >= MIN_BLOCKS && total > 0) {
          const wav = encodeWav(this.phrase, total);
          let peak = 0;
          for (const b of this.phrase) for (let i = 0; i < b.length; i++) peak = Math.max(peak, Math.abs(b[i]));
          this.events.onPhrase(wav, peak / 32768);
        }
        this.phrase = [];
        this.phraseLen = 0;
        this.inSpeech = false;
        this.silenceRun = 0;
        this.blocksRun = 0;
      }
    } else if (isSpeech) {
      // Начало фразы: pre-roll в начало.
      this.phrase = this.preRoll.splice(0);
      this.phrase.push(block);
      this.phraseLen = this.phrase.reduce((a, b) => a + b.length, 0);
      this.inSpeech = true;
      this.silenceRun = 0;
      this.blocksRun = 1;
    } else {
      // Тихо: копим pre-roll.
      this.preRoll.push(block);
      if (this.preRoll.length > PRE_ROLL_BLOCKS) this.preRoll.shift();
    }
  }
}

// Worklet: пересылает сырые int16-блоки по 100 мс.
const VAD_WORKLET_SRC = `
class VadProc extends AudioWorkletProcessor {
  constructor() {
    super();
    this.block = new Int16Array(128);
    this.pos = 0;
  }
  process(inputs) {
    const ch = inputs[0][0];
    if (ch) {
      for (let i = 0; i < ch.length; i++) {
        let s = Math.max(-1, Math.min(1, ch[i]));
        this.block[this.pos++] = s < 0 ? s * 0x8000 : s * 0x7fff;
        if (this.pos >= this.block.length) {
          this.port.postMessage(this.block.slice(0));
          this.pos = 0;
        }
      }
    }
    return true;
  }
}
registerProcessor('vad-proc', VadProc);
`;

/** PCM-блоки → WAV-файл (16 кГц, моно, 16 бит) с нормализацией громкости. */
function encodeWav(blocks: Int16Array[], totalSamples: number): ArrayBuffer {
  // Пиковая нормализация: тихие фразы усиливаем (как _normalize в v1).
  let peak = 0;
  for (const b of blocks) for (let i = 0; i < b.length; i++) peak = Math.max(peak, Math.abs(b[i]));
  const gain = peak > 0 && peak < 16000 ? Math.min(8, 20000 / peak) : 1;

  const bytes = new Uint8Array(44 + totalSamples * 2);
  const view = new DataView(bytes.buffer);
  writeAscii(view, 0, "RIFF");
  view.setUint32(4, 36 + totalSamples * 2, true);
  writeAscii(view, 8, "WAVE");
  writeAscii(view, 12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, SAMPLE_RATE, true);
  view.setUint32(28, SAMPLE_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeAscii(view, 36, "data");
  view.setUint32(40, totalSamples * 2, true);
  let off = 44;
  for (const b of blocks) {
    for (let i = 0; i < b.length; i++) {
      const v = Math.max(-32768, Math.min(32767, Math.round(b[i] * gain)));
      view.setInt16(off, v, true);
      off += 2;
    }
  }
  return bytes.buffer;
}

function writeAscii(view: DataView, offset: number, s: string) {
  for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i));
}

// ------------------------------------------------------------------ wake word

// Регексп учитывает частые ошибки распознавания «джарвис» (порт WAKE_RE из v1).
const WAKE_RE =
  /(?:^|\s)(?:д?ж|ч|ш|г|д)[аеэиоя]р[вб][иеы](?:[сзш]\S*)?(?=\s|$)|джарвис|джавис|джарви(?=\s|$)/;
const STOP_RE = /(?:^|\s)стоп(?:\s|$|!|\.)/;

export function stripWake(text: string): { kind: "stop" } | { kind: "listen" } | { kind: "command"; text: string } | null {
  const low = text.toLowerCase().replace(/ё/g, "е");
  if (STOP_RE.test(low)) return { kind: "stop" };
  const m = WAKE_RE.exec(low);
  if (m) {
    const cmd = (low.slice(0, m.index) + " " + low.slice(m.index + m[0].length))
      .replace(/\s+/g, " ")
      .trim()
      .replace(/^[ ,.\-!]+|[ ,.\-!]+$/g, "");
    if (!cmd) return { kind: "listen" };
    return { kind: "command", text: cmd };
  }
  return null;
}

export function base64Wav(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

export function stateIsSpeaking(s: AgentStateEvent): boolean {
  return s.status === "speaking";
}
