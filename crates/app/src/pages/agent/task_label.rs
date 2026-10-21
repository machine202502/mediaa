//! What a task of the queue is called on screen. The queue stores only the data part of the name
//! (`Task::title`: a file name, a topic title, the start of a question, a step number); the label
//! is built here from the kind of the task and that data, in the language of the interface.

use rust_i18n::t;

/// Every kind of task the program puts into the queue.
#[cfg(test)]
pub const KINDS: &[&str] = &[
    "analysis",
    "transcribe",
    "diarize",
    "index",
    "topics",
    "topics-plan",
    "topic-digest",
    "topics-merge",
    "details",
    "topic-detail",
    "chat",
    "chat-title",
    "model-step",
    "tool",
];

/// The label of a task of `kind` with the data `title`: «Анализ: interview.mp4». A kind the
/// program does not know is shown as it is stored.
pub fn label(kind: &str, title: &str) -> String {
    label_in(&rust_i18n::locale(), kind, title)
}

/// [`label`] in a given language.
fn label_in(locale: &str, kind: &str, title: &str) -> String {
    let text: String = match kind {
        "analysis" => t!("queue.kind.analysis", locale = locale, title = title),
        "transcribe" => t!("queue.kind.transcribe", locale = locale, title = title),
        "diarize" => t!("queue.kind.diarize", locale = locale, title = title),
        "index" => t!("queue.kind.index", locale = locale, title = title),
        "topics" => t!("queue.kind.topics", locale = locale, title = title),
        "topics-plan" => t!("queue.kind.topics-plan", locale = locale, title = title),
        "topic-digest" => t!("queue.kind.topic-digest", locale = locale, title = title),
        "topics-merge" => t!("queue.kind.topics-merge", locale = locale, title = title),
        "details" => t!("queue.kind.details", locale = locale, title = title),
        "topic-detail" => t!("queue.kind.topic-detail", locale = locale, title = title),
        "chat" => t!("queue.kind.chat", locale = locale, title = title),
        "chat-title" => t!("queue.kind.chat-title", locale = locale, title = title),
        "model-step" => t!("queue.kind.model-step", locale = locale, title = title),
        "tool" => t!("queue.kind.tool", locale = locale, title = title),
        other => return if title.is_empty() { other.to_string() } else { format!("{other}: {title}") },
    }
    .into();
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_label_in_both_languages() {
        for locale in ["ru", "en"] {
            for kind in KINDS {
                let text = label_in(locale, kind, "DATA");
                assert!(text.contains("DATA"), "{locale} {kind}: {text}");
                assert!(!text.contains("queue.kind"), "{locale} {kind}: the key is missing: {text}");
                assert!(!text.contains("%{"), "{locale} {kind}: {text}");
            }
        }
    }

    #[test]
    fn a_label_is_built_from_the_kind_and_the_data() {
        assert_eq!(label_in("ru", "analysis", "интервью.mp4"), "Анализ: интервью.mp4");
        assert_eq!(label_in("ru", "topic-digest", "#3"), "Сводка темы #3");
        assert_eq!(label_in("ru", "model-step", "2"), "Шаг 2");
        assert_eq!(label_in("ru", "chat", "Когда отчёт?"), "Ответ: Когда отчёт?");
        assert_eq!(label_in("en", "analysis", "talk.mp4"), "Analysis: talk.mp4");
        assert_eq!(label_in("en", "chat", "When?"), "Answer: When?");
        // Not a known kind: shown as stored.
        assert_eq!(label_in("ru", "count", "to ten"), "count: to ten");
        assert_eq!(label_in("ru", "count", ""), "count");
    }
}
