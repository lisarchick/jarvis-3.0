//! Защита данных: секреты не покидают машину, контент из веба трактуется как данные.
//! Код-уровневые проверки (не промпты): детект паролей/ключей/токенов на входе
//! и на выходе локальных моделей.

use once_cell::sync::Lazy;
use regex::Regex;

static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"(?i)(пароль|password|api[_ -]?key|access[_ -]?token|client[_ -]?secret|секрет|токен)\s*[:=]\s*\S+",
        r"(?i)мой\s+пароль\s+\S+",
        r"(?i)Bearer\s+[A-Za-z0-9\-_]{12,}",
        r"sk-[A-Za-z0-9\-_]{16,}",
        r"AIza[A-Za-z0-9\-_]{30,}",
        r"ghp_[A-Za-z0-9]{30,}",
        r"eyJ[A-Za-z0-9\-_]{10,}\.[A-Za-z0-9\-_]{10,}\.[A-Za-z0-9\-_]{10,}",
        r"(?i)sha1 signature[a-z0-9]{30,}",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// Есть ли в тексте секрет (пароль/ключ/токен)?
pub fn contains_secrets(text: &str) -> bool {
    SECRET_PATTERNS.iter().any(|re| re.is_match(text))
}

/// Запрос содержит намерение отправить/запомнить секрет?
pub fn sensitive_intent(text: &str) -> bool {
    static INTENT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(запомни|сохрани|отправь|передай|покажи|вставь|введи|запиши)[^\.]{0,40}(парол|api[ _]?key|секретн|токен|учетн|учётн|данные карты|номер карты)")
            .unwrap()
    });
    INTENT.is_match(text) || contains_secrets(text)
}

/// Локальная модель утверждает, что выполнила действие (галлюцинация)?
pub fn claims_action(text: &str) -> bool {
    static ACT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(я\s+)?(успешно\s+)?(удалил|стёр|стер|открыл|запустил|закрыл|отправил|сохранил|выключил|переместил|перезаписал|выполнил)")
            .unwrap()
    });
    ACT.is_match(text)
}

pub const REFUSAL: &str = "Я не принимаю и не запоминаю пароли, ключи и токены, и во внешние сервисы их не отправляю. Если секрет попал в запрос — удали его и повтори.";

pub const SAFE_FALLBACK: &str = "В этом ответе нет уверенности, поэтому зачитывать его не буду. Для действий на компьютере есть команды, а за фактами лучше обратиться к поиску.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_secrets() {
        assert!(contains_secrets("пароль: qwerty123"));
        assert!(contains_secrets("sk-abcdefghij1234567890abcd"));
        assert!(contains_secrets("мой api key = abc123xyz"));
        assert!(!contains_secrets("какой курс доллара сегодня"));
        assert!(!contains_secrets("поставь таймер на 5 минут"));
    }

    #[test]
    fn detects_intent() {
        assert!(sensitive_intent("запомни мой пароль от почты qwerty"));
        assert!(sensitive_intent("отправь токен телеграму"));
        assert!(!sensitive_intent("запомни, что я люблю кофе"));
    }

    #[test]
    fn detects_action_claims() {
        assert!(claims_action("Я удалил файл с рабочего стола"));
        assert!(claims_action("успешно запустил программу"));
        assert!(!claims_action("Файл удалён из корзины")); // пассив — ок
    }
}
