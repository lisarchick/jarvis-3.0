//! Голоса персонажей: у каждого персонажа свой голос edge-tts (мультиязычные
//! для RU/EN) и стиль по эмоции (темп/высота через SSML prosody).

use crate::config::Config;

pub struct VoiceProfile {
    pub voice: &'static str,
    /// Темп в процентах (-40..40).
    pub rate: i32,
    /// Высота в герцах (-40..40).
    pub pitch: i32,
}

const TABLE: &[(&str, VoiceProfile)] = &[
    ("aria", VoiceProfile { voice: "en-US-AvaMultilingualNeural", rate: 0, pitch: 0 }),
    ("sumi", VoiceProfile { voice: "en-US-EmmaMultilingualNeural", rate: -4, pitch: 0 }),
    ("ren", VoiceProfile { voice: "en-US-AndrewMultilingualNeural", rate: 0, pitch: 0 }),
    ("kaito", VoiceProfile { voice: "en-US-BrianMultilingualNeural", rate: -6, pitch: -10 }),
    ("mika", VoiceProfile { voice: "ru-RU-DmitryNeural", rate: 10, pitch: 12 }),
    ("shizuku", VoiceProfile { voice: "ru-RU-SvetlanaNeural", rate: -6, pitch: 10 }),
];

pub fn base(id: &str) -> Option<VoiceProfile> {
    TABLE
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, p)| VoiceProfile { voice: p.voice, rate: p.rate, pitch: p.pitch })
}

fn signed(v: i32) -> String {
    if v >= 0 { format!("+{v}") } else { format!("{v}") }
}

/// Резолвит (голос, rate "+8%", pitch "+8Hz") для конфига с поправкой на эмоцию.
/// Персонаж «core»/неизвестный — классические голоса female/male.
pub fn resolve(cfg: &Config, emotion: &str) -> (String, String, String) {
    let (voice, mut rate, mut pitch) = match base(&cfg.character_id) {
        Some(p) => (p.voice.to_string(), p.rate, p.pitch),
        None => (
            crate::tts::voice_id(&cfg.voice),
            0,
            0,
        ),
    };
    match emotion {
        "happy" => {
            rate += 8;
            pitch += 8;
        }
        "sad" => {
            rate -= 10;
            pitch -= 6;
        }
        "angry" => {
            rate += 6;
            pitch += 4;
        }
        _ => {}
    }
    (voice, format!("{}%", signed(rate.clamp(-40, 40))), format!("{}Hz", signed(pitch.clamp(-40, 40))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_and_emotion_style() {
        let mut cfg = Config::default();
        cfg.character_id = "mika".into();
        let (v, r, p) = resolve(&cfg, "neutral");
        assert_eq!(v, "ru-RU-DmitryNeural");
        assert_eq!(r, "+10%");
        assert_eq!(p, "+12Hz");
        let (_, r, p) = resolve(&cfg, "happy");
        assert_eq!(r, "+18%");
        assert_eq!(p, "+20Hz");
        // фолбэк на классический голос
        cfg.character_id = "core".into();
        let (v, r, _) = resolve(&cfg, "sad");
        assert_eq!(v, "ru-RU-SvetlanaNeural");
        assert_eq!(r, "-10%");
    }
}
