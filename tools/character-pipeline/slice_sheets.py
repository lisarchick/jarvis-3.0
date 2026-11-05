#!/usr/bin/env python3
"""Нарезка expression-шиитов JARVIS на кадры персонажей (шаблонная сетка + уточнение).

Вход:  assets/characters/_source/sheet_*.jpg
Выход: public/characters/<id>/{frame_<эмоция>_<idle|talk>.png, manifest.json, preview.png}
       + assets/characters/_source/_contact.jpg — визуальная проверка.

Геометрия шита одинакова у всех персонажей (один шаблон), поэтому ячейки берём
из дробного шаблона и уточняем bbox по контенту внутри окна (отличие от фона).
Зависимости: pillow, numpy.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "assets" / "characters" / "_source"
OUT = ROOT / "public" / "characters"

CHARACTERS = {
    "sheet_composite.jpg": [
        ("aria", "Ария", "en-US-AvaMultilingualNeural", 0, 0),
        ("sumi", "Суми", "en-US-EmmaMultilingualNeural", -4, 0),
        ("ren", "Рен", "en-US-AndrewMultilingualNeural", 0, 0),
        ("kaito", "Кайто", "en-US-BrianMultilingualNeural", -6, -10),
    ],
    "sheet_catgirl.jpg": [("shizuku", "Сидзуку", "ru-RU-SvetlanaNeural", -6, 10)],
    "sheet_catboy.jpg": [("mika", "Мика", "ru-RU-DmitryNeural", 10, 12)],
}

EMOTIONS = ["neutral", "happy", "sad", "angry"]

# Дробный шаблон одной страницы (проверен по catgirl/catboy):
# 4 колонки x 2 строки (SILENT сверху, TALKING снизу).
COLS = [0.045, 0.2725, 0.500, 0.7275, 0.955]
ROW_SILENT = (0.045, 0.445)
ROW_TALKING = (0.495, 0.930)


def phase_align(ref: np.ndarray, img: np.ndarray) -> tuple[int, int]:
    a = ref.astype(np.float32) / 255.0
    b = img.astype(np.float32) / 255.0
    fa = np.fft.rfft2(a - a.mean())
    fb = np.fft.rfft2(b - b.mean())
    cross = fa * np.conj(fb)
    cross /= np.abs(cross) + 1e-9
    corr = np.fft.irfft2(cross, s=a.shape)
    peak = np.unravel_index(np.argmax(corr), corr.shape)
    dy, dx = peak
    if dy > a.shape[0] // 2:
        dy -= a.shape[0]
    if dx > a.shape[1] // 2:
        dx -= a.shape[1]
    return dx, dy


def sheet_cells(w: int, h: int, ox: int = 0, oy: int = 0) -> list[tuple[int, int, int, int]]:
    """8 окон ячеек одной страницы: silent 4 + talking 4 (в пикселях родителя)."""
    cells = []
    for (ry0, ry1) in (ROW_SILENT, ROW_TALKING):
        for i in range(4):
            x0 = int(w * COLS[i]) + ox
            x1 = int(w * COLS[i + 1]) + ox
            y0 = int(h * ry0) + oy
            y1 = int(h * ry1) + oy
            cells.append((x0, y0, x1, y1))
    return cells


def erase_labels(region: Image.Image, bg: np.ndarray) -> Image.Image:
    """Заливает ярлыки в левом верхнем углу ячейки фоном (одиночные шиты)."""
    w, h = region.size
    a = np.asarray(region).copy()
    for (fx0, fy0, fx1, fy1) in [(0.0, 0.0, 0.64, 0.125), (0.0, 0.125, 0.18, 0.15)]:
        x0, y0, x1, y1 = int(w * fx0), int(h * fy0), int(w * fx1), int(h * fy1)
        a[y0:y1, x0:x1] = bg.astype(np.uint8)
    return Image.fromarray(a)


def process_sheet(path: Path, defs: list[tuple], contact: list) -> None:
    full = Image.open(path).convert("RGB")
    W, H = full.size
    corners = np.concatenate(
        [
            np.asarray(full.crop((0, 0, 40, 40))).reshape(-1, 3),
            np.asarray(full.crop((W - 40, H - 40, W, H))).reshape(-1, 3),
        ]
    )
    bg = np.median(corners, axis=0)
    single = path.name != "sheet_composite.jpg"

    if single:
        per_char = [sheet_cells(W, H)]
    else:
        qw, qh = W // 2, H // 2
        quads = [(0, 0), (qw, 0), (0, qh), (qw, qh)]
        per_char = [sheet_cells(qw, qh, ox, oy) for ox, oy in quads]

    for (cid, title, voice, rate, pitch), windows in zip(defs, per_char):
        frames = []
        for i, win in enumerate(windows):
            inset_x = int((win[2] - win[0]) * 0.022)
            inset_y = int((win[3] - win[1]) * 0.022)
            region = full.crop((win[0] + inset_x, win[1] + inset_y, win[2] - inset_x, win[3] - inset_y))
            if single and i in (0, 4):
                region = erase_labels(region, bg)
            frames.append(region)

        side_w = int(np.median([f.size[0] for f in frames]))
        side_h = int(np.median([f.size[1] for f in frames]))
        frames = [f.resize((side_w, side_h), Image.BICUBIC) for f in frames]

        ref = np.asarray(frames[0].convert("L"))
        aligned = [frames[0]]
        for f in frames[1:]:
            g = np.asarray(f.convert("L"))
            dx, dy = phase_align(ref, g)
            aligned.append(f.transform(f.size, Image.AFFINE, (1, 0, -dx, 0, 1, -dy), resample=Image.BICUBIC))

        cdir = OUT / cid
        cdir.mkdir(parents=True, exist_ok=True)
        for i, frame in enumerate(aligned):
            emo = EMOTIONS[i % 4]
            mode = "idle" if i < 4 else "talk"
            frame.save(cdir / f"frame_{emo}_{mode}.png")
        aligned[0].save(cdir / "preview.png")
        manifest = {
            "id": cid,
            "name": title,
            "voice": voice,
            "rate": rate,
            "pitch": pitch,
            "frames": {f"{e}_{m}": f"frame_{e}_{m}.png" for e in EMOTIONS for m in ("idle", "talk")},
        }
        (cdir / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
        contact.append((cid, aligned[0]))
        print(f"  {cid}: 8 кадров ok, голос {voice}")


def main() -> int:
    contact: list = []
    for sheet, defs in CHARACTERS.items():
        p = SRC / sheet
        if p.exists():
            process_sheet(p, defs, contact)
        else:
            print(f"нет файла: {p}")
    if contact:
        size = 220
        board = Image.new("RGB", (len(contact) * size, size + 22), (18, 18, 22))
        d = ImageDraw.Draw(board)
        for i, (cid, img) in enumerate(contact):
            t = img.copy()
            t.thumbnail((size - 8, size - 8))
            board.paste(t, (i * size + 4, 4))
            d.text((i * size + 8, size + 4), cid, fill=(210, 210, 220))
        board.save(SRC / "_contact.jpg", quality=92)
        print(f"контакт-лист: {SRC / '_contact.jpg'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
