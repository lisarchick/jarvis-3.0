// Общие типы фронтенда, зеркалят Rust-структуры.

export type Status =
  | "idle"
  | "listening"
  | "thinking"
  | "searching"
  | "executing"
  | "speaking";

export type Emotion =
  | "idle"
  | "listening"
  | "thinking"
  | "searching"
  | "speaking"
  | "executing"
  | "confused"
  | "happy"
  | "sleep"
  | "permission"
  | "sorry";

export interface AgentStateEvent {
  status: Status;
  emotion: Emotion;
  detail: string;
}

export type StreamChunk =
  | { kind: "delta"; text: string }
  | { kind: "done"; message_id: number }
  | { kind: "error"; message: string };

export interface ToolEvent {
  id: string;
  tool: string;
  title: string;
  args: Record<string, unknown>;
  type: "start" | "ok" | "error" | "need-confirm";
  detail: string;
}

export interface Source {
  title: string;
  url: string;
  host: string;
}

export interface Confirmation {
  id: string;
  title: string;
  detail: string;
  risk: "low" | "medium" | "high";
}

export interface ConnectedModel {
  id: string;
  label: string;
  provider: "openai" | "anthropic" | "gemini" | "local" | string;
  api_key: string;
  model: string;
  base_url: string;
  capabilities: string[];
}

export interface Config {
  user_name: string;
  reply_style: "short" | "normal" | "detailed" | string;
  language: "auto" | "ru" | "en" | string;
  persona: string;
  wake_word: string;
  dialogue_timeout_sec: number;
  llm_provider: string;
  llm_key: string;
  llm_model: string;
  llm_base_url: string;
  connected_models: ConnectedModel[];
  keys: Record<string, string>;
  tts_enabled: boolean;
  voice: "female" | "male" | string;
  stt_engine: "google" | "openai" | string;
  mic_device: string;
  commands_enabled: boolean;
  use_web_search: boolean;
  timers_enabled: boolean;
  music_service: string;
  notes_dir: string;
  extra_allowed_dirs: string[];
  permissions: Record<string, boolean>;
  overlay_hotkey: string;
  autostart: boolean;
  character_enabled: boolean;
  character_style: string;
}

export interface ActionEntry {
  ts: string;
  tool: string;
  args: string;
  status: string;
  detail: string;
}

export interface Note {
  id: number;
  title: string;
  body: string;
  kind: string;
  audio_path: string;
  ts: string;
}

export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}
