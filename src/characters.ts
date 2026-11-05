// Реестр персонажей JARVIS (пакеты лежат в public/characters/<id>/).

export interface CharacterInfo {
  id: string;
  name: string;
}

export const CHARACTERS: CharacterInfo[] = [
  { id: "aria", name: "Ария" },
  { id: "sumi", name: "Суми" },
  { id: "ren", name: "Рен" },
  { id: "kaito", name: "Кайто" },
  { id: "mika", name: "Мика" },
  { id: "shizuku", name: "Сидзуку" },
];

export interface CharacterManifest {
  id: string;
  name: string;
  voice: string;
  rate: number;
  pitch: number;
  frames: Record<string, string>;
}

export async function loadManifest(id: string): Promise<CharacterManifest | null> {
  try {
    const res = await fetch(`characters/${id}/manifest.json`);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

// Теги настроения от LLM: «...текст [радость]» — вычищаются до показа и TTS.
export const EMO_TAG_RE = /\s*\[(нейтрально|радость|грусть|злость|удивление)(?::\d+(?:\.\d+)?)?\]/g;

export function stripEmotionTags(text: string): string {
  return text.replace(EMO_TAG_RE, "").trimEnd();
}
