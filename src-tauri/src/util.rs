//! Утилиты: русская морфология, нормализация фраз, wake-word, разбивка потока
//! на предложения для TTS, очистка markdown «для голоса», лёгкие эмбеддинги
//! (хэш триграмм, 256 измерений) и нечёткий поиск приложений.

use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};

// ------------------------------------------------------------------ морфология

/// Русская плюрализация: plural(5, ("минута","минуты","минут")) → "минут".
pub fn plural<'a>(n: i64, forms: (&'a str, &'a str, &'a str)) -> &'a str {
    let n = n.abs() % 100;
    let n1 = n % 10;
    if (11..=14).contains(&n) {
        return forms.2;
    }
    if n1 == 1 {
        return forms.0;
    }
    if (2..=4).contains(&n1) {
        return forms.1;
    }
    forms.2
}

const DUR_FORMS: [(&str, &str, &str); 3] =
    [("час", "часа", "часов"), ("минута", "минуты", "минут"), ("секунда", "секунды", "секунд")];

/// 3725 → «1 час 2 минуты 5 секунд».
pub fn fmt_duration(sec: i64) -> String {
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    let parts: Vec<String> = [(h, 0usize), (m, 1), (s, 2)]
        .iter()
        .filter(|(v, _)| *v > 0)
        .map(|(v, i)| format!("{} {}", v, plural(*v, DUR_FORMS[*i])))
        .collect();
    if parts.is_empty() { "0 секунд".into() } else { parts.join(" ") }
}

// ------------------------------------------------------------------ нормализация

pub fn normalize(text: &str) -> String {
    let mut t = text.to_lowercase().replace('ё', "е");
    t = t.replace(['“', '”', '«', '»', '"', '\'', '!', '?', ',', ';', ':', '(', ')'], " ");
    t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t = replace_number_words(&t);
    // вежливые приставки
    loop {
        let new = t
            .strip_prefix("пожалуйста ")
            .or_else(|| t.strip_prefix("слушай "))
            .or_else(|| t.strip_prefix("ну "))
            .unwrap_or(&t)
            .to_string();
        if new == t { return t; }
        t = new;
    }
}

/// «двадцать пять» → 25, «полчаса» → «30 минут». Покрывает частые случаи.
fn replace_number_words(t: &str) -> String {
    let mut t = t.replace("полчаса", "30 минут").replace("полминуты", "30 секунд");
    t = t.replace("четверть часа", "15 минут");
    t = t.replace("полтора", "1.5").replace("полторы", "1.5");
    static WORDS: Lazy<std::collections::HashMap<&'static str, u32>> = Lazy::new(|| {
        [
            ("один", 1), ("одна", 1), ("два", 2), ("две", 2), ("три", 3), ("четыре", 4), ("пять", 5),
            ("шесть", 6), ("семь", 7), ("восемь", 8), ("девять", 9), ("десять", 10),
            ("одиннадцать", 11), ("двенадцать", 12), ("тринадцать", 13), ("четырнадцать", 14),
            ("пятнадцать", 15), ("шестнадцать", 16), ("семнадцать", 17), ("восемнадцать", 18),
            ("девятнадцать", 19), ("двадцать", 20), ("тридцать", 30), ("сорок", 40),
            ("пятьдесят", 50), ("шестьдесят", 60), ("семьдесят", 70), ("восемьдесят", 80),
            ("девяносто", 90), ("сто", 100), ("двести", 200), ("триста", 300), ("тысячу", 1000),
        ]
        .into_iter()
        .collect()
    });
    let mut out: Vec<String> = Vec::new();
    let mut pending: u32 = 0;
    let mut changed = false;
    for word in t.split(' ') {
        if let Some(v) = WORDS.get(word) {
            pending += v;
            changed = true;
            continue;
        }
        if pending > 0 {
            // число, только если следующее слово — единица измерения или конец
            out.push(pending.to_string());
            pending = 0;
        }
        out.push(word.to_string());
    }
    if pending > 0 {
        out.push(pending.to_string());
    }
    if changed { out.join(" ") } else { t }
}

// ------------------------------------------------------------------ wake word / стоп

/// Wake word по одному слову: ловит и ошибки распознавания (жарвис, джервис,
/// чарвис, гарвис, джарвиз…). Rust regex не поддерживает lookahead — поэтому
/// проверяем пословно с анкерами.
pub static WAKE_WORD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:д?ж|ч|ш|г|д)[аеэиоя]р[вб][иеы](?:[сзш]\w*)?$|^(?:д?ж[ае]в[иы][сзш]\w*)$|^(?:джарвис|джавис|джарви|жавис)$").unwrap()
});

pub static STOP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?u)\bстоп\b").unwrap());

/// Есть ли wake word в фразе (пословно).
pub fn has_wake_word(text: &str) -> bool {
    text.to_lowercase().replace('ё', "е").split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        !w.is_empty() && WAKE_WORD_RE.is_match(w)
    })
}

pub static NEW_SESSION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"нов\w+\s+(?:сесси\w+|чат\w*|диалог\w*|разговор\w*)|начн\w+\s+(?:сначала|заново)").unwrap()
});

pub static GOODBYE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:^|\s)(?:до свидания|пока|отбой|до связи|всего доброго|заверш\w+ диалог|законч\w+ разговор)(?:\s|$|!|\.|$)").unwrap()
});

/// Извлекает команду после wake word; None — если wake word нет.
pub fn strip_wake(text: &str) -> Option<String> {
    let low = text.to_lowercase().replace('ё', "е");
    if STOP_RE.is_match(&low) {
        return Some("__stop__".into());
    }
    let words: Vec<&str> = low.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let clean = w.trim_matches(|c: char| !c.is_alphanumeric());
        if !clean.is_empty() && WAKE_WORD_RE.is_match(clean) {
            let mut cmd = words[..i].join(" ");
            if !cmd.is_empty() {
                cmd.push(' ');
            }
            cmd.push_str(&words[i + 1..].join(" "));
            let cmd = cmd.trim_matches(|c: char| c == ' ' || c == ',' || c == '.' || c == '-' || c == '!').to_string();
            if cmd.is_empty() {
                return Some("__listen__".into());
            }
            return Some(cmd);
        }
    }
    None
}

// ------------------------------------------------------------------ предложения для TTS

// Rust regex не поддерживает lookbehind — пунктуацию оставляем в «голове» фразы.
static SENT_END_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([.!?…]+)\s+|\n+").unwrap());

/// Режет накопленный текст на пригодные для озвучки предложения.
/// Граница: точка/!/?/… + пробел, либо перевод строки.
pub fn split_sentences(buffer: &str, early: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = buffer.to_string();
    loop {
        let Some(caps) = SENT_END_RE.captures(&rest) else { break };
        let whole = caps.get(0).unwrap();
        // Если группа пунктуации есть — голова кончается после неё.
        let head_end = match caps.get(1) {
            Some(punct) => whole.start() + punct.as_str().len(),
            None => whole.start(),
        };
        if rest[..head_end].trim().is_empty() {
            // пустая голова (например, \n в начале) — пропускаем разделитель
            rest = rest[whole.end()..].to_string();
            continue;
        }
        let head = rest[..head_end].to_string();
        rest = rest[whole.end()..].to_string();
        push_sentence(&mut out, &head, early);
    }
    if !rest.trim().is_empty() {
        push_sentence(&mut out, &rest, usize::MAX);
    }
    out
}

fn push_sentence(out: &mut Vec<String>, s: &str, _early: usize) {
    let s = s.trim();
    if s.chars().count() >= 2 {
        out.push(speakable(s));
    }
}

/// Убирает markdown/HTML, чтобы не зачитывать «звёздочки» и теги.
/// Содержимое инлайн-кода сохраняем — снимаем только бэктики.
pub fn speakable(text: &str) -> String {
    static CODE_SPAN: Lazy<Regex> = Lazy::new(|| Regex::new(r"`+([^`]*)`+").unwrap());
    static MD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"!?\[([^\]]*)\]\([^)]*\)|\*\*?|__?|^#{1,6}\s*|^\s*[-*+]\s+|^\s*\d+\.\s+|<[^>]+>").unwrap()
    });
    let s = CODE_SPAN.replace_all(text, "$1").into_owned();
    // Текст ссылок сохраняем (группа 1), остальное сносим.
    let mut s = MD.replace_all(&s, "$1").into_owned();
    s = s.replace(['«', '»', '“', '”'], "");
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    s.trim().to_string()
}

// ------------------------------------------------------------------ эмбеддинги (хэш триграмм)

pub const EMBED_DIM: usize = 256;

/// Быстрый детерминированный эмбеддинг: мешок символьных триграмм → 256 измерений, L2-нормировка.
pub fn embed(text: &str) -> [f32; EMBED_DIM] {
    let mut v = [0f32; EMBED_DIM];
    let t: String = text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    let chars: Vec<char> = t.chars().collect();
    if chars.is_empty() {
        return v;
    }
    let mut feed = |gram: &str| {
        let mut h = Sha256::digest(gram.as_bytes());
        h[0] = h[0].wrapping_add(0x9e);
        let idx = (u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize) % EMBED_DIM;
        let sign = if h[4] & 1 == 0 { 1.0 } else { -1.0 };
        v[idx] += sign;
    };
    for i in 0..chars.len().saturating_sub(2) {
        feed(&chars[i..i + 3].iter().collect::<String>());
    }
    feed(&chars.iter().collect::<String>()); // весь текст целиком — сильный сигнал
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

pub fn cosine(a: &[f32; EMBED_DIM], b: &[f32; EMBED_DIM]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>().clamp(-1.0, 1.0)
}

/// Похожесть строк 0..1 (посимвольная, для «забудь про X»).
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    // Быстрый LCS-подобный расчёт (расстояние Левенштейна с порогом длины)
    let max = a.len().max(b.len());
    let dist = levenshtein(&a, &b);
    1.0 - (dist as f64 / max as f64)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

// ------------------------------------------------------------------ нечёткий поиск приложений

fn translit(s: &str) -> String {
    const MAP: &[(&str, &str)] = &[
        ("а", "a"), ("б", "b"), ("в", "v"), ("г", "g"), ("д", "d"), ("е", "e"), ("ё", "e"), ("ж", "zh"),
        ("з", "z"), ("и", "i"), ("й", "y"), ("к", "k"), ("л", "l"), ("м", "m"), ("н", "n"), ("о", "o"),
        ("п", "p"), ("р", "r"), ("с", "s"), ("т", "t"), ("у", "u"), ("ф", "f"), ("х", "h"), ("ц", "ts"),
        ("ч", "ch"), ("ш", "sh"), ("щ", "sch"), ("ъ", ""), ("ы", "y"), ("ь", ""), ("э", "e"), ("ю", "yu"),
        ("я", "ya"),
    ];
    let mut out = String::new();
    let lower = s.to_lowercase();
    let mut chars = lower.chars().peekable();
    while let Some(c) = chars.next() {
        let cs = c.to_string();
        if let Some((_, lat)) = MAP.iter().find(|(ru, _)| *ru == cs) {
            out.push_str(lat);
        } else {
            out.push(c);
        }
        let _ = chars.peek();
    }
    out
}

/// Ранжирует приложения по запросу (точное > подстрока > транслит > Левенштейн).
pub fn best_matches(query: &str, apps: Vec<crate::AppEntry>) -> Vec<crate::AppEntry> {
    let q = query.to_lowercase();
    let q_tr = translit(&q);
    let mut scored: Vec<(i32, crate::AppEntry)> = apps
        .into_iter()
        .map(|a| {
            let name = a.name.to_lowercase();
            let name_tr = translit(&name);
            let score = if name == q || name_tr == q_tr {
                100
            } else if name.starts_with(&q) || name_tr.starts_with(&q_tr) {
                80
            } else if name.contains(&q) || name_tr.contains(&q_tr) {
                60
            } else if q.len() >= 4 && (name.contains(&q_tr) || name_tr.contains(&q)) {
                50
            } else {
                let sim = similarity(&name, &q).max(similarity(&name_tr, &q_tr));
                if sim >= 0.72 { (sim * 40.0) as i32 } else { 0 }
            };
            (score, a)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, a)| a).collect()
}

// ------------------------------------------------------------------ id

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

pub fn short_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..8).map(|_| format!("{:x}", rng.gen::<u32>() & 0xF)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_forms() {
        assert_eq!(plural(1, DUR_FORMS[1]), "минута");
        assert_eq!(plural(3, DUR_FORMS[1]), "минуты");
        assert_eq!(plural(5, DUR_FORMS[1]), "минут");
        assert_eq!(plural(12, DUR_FORMS[1]), "минут");
        assert_eq!(plural(21, DUR_FORMS[1]), "минута");
    }

    #[test]
    fn duration_words() {
        assert_eq!(fmt_duration(3725), "1 час 2 минуты 5 секунд");
        assert_eq!(fmt_duration(0), "0 секунд");
    }

    #[test]
    fn wake_word_variants() {
        assert_eq!(strip_wake("Джарвис, открой калькулятор").as_deref(), Some("открой калькулятор"));
        assert_eq!(strip_wake("жарвис какая погода").as_deref(), Some("какая погода"));
        assert_eq!(strip_wake("Джарвис").as_deref(), Some("__listen__"));
        assert_eq!(strip_wake("стоп").as_deref(), Some("__stop__"));
        assert_eq!(strip_wake("какая погода в сочи"), None);
        // ложные срабатывания v1 исключены
        assert_eq!(strip_wake("расскажи про сервисы доставки"), None);
    }

    #[test]
    fn sentences_split() {
        let out = split_sentences(
            "Маркони получил Нобелевскую премию в 1909 году. Вместе с Брауном. Подробности?",
            100,
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "Маркони получил Нобелевскую премию в 1909 году.");
        assert_eq!(out[1], "Вместе с Брауном.");
        assert_eq!(out[2], "Подробности?");
        // Переводы строк тоже режут.
        let two = split_sentences("первая строка\nвторая строка", 100);
        assert_eq!(two, vec!["первая строка".to_string(), "вторая строка".to_string()]);
    }

    #[test]
    fn speakable_strips_markdown() {
        assert_eq!(speakable("**Жирный** и `код`"), "Жирный и код");
        assert_eq!(speakable("[ссылка](http://x.ru)"), "ссылка");
    }

    #[test]
    fn embeddings_similarity() {
        let a = embed("пользователь предпочитает spotify");
        let b = embed("предпочитаю spotify для музыки");
        let c = embed("курс валют центрального банка");
        assert!(cosine(&a, &b) > cosine(&a, &c));
    }

    #[test]
    fn fuzzy_apps() {
        let apps = vec![
            crate::AppEntry { name: "discord".into(), path: "".into(), kind: "exe".into(), is_media: false },
            crate::AppEntry { name: "дискорд".into(), path: "".into(), kind: "exe".into(), is_media: false },
            crate::AppEntry { name: "spotify".into(), path: "".into(), kind: "uri".into(), is_media: true },
        ];
        assert_eq!(best_matches("дискорд", apps.clone()).len(), 2);
        assert_eq!(best_matches("спотифай", apps.clone())[0].name, "spotify");
    }
}
