use mediaa_agent::assistant::Recording;
use mediaa_core::speakers::{self, Piece, Who};

pub fn unnamed(index: u32) -> String {
    format!("Говорящий {}", index + 1)
}

pub fn roster(recording: &Recording) -> Vec<Who> {
    let mut ids: Vec<u32> = recording.replies.iter().filter_map(|r| r.speaker).collect();
    ids.extend(recording.names.keys().copied());
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .map(|index| Who {
            index,
            recording: None,
            name: recording.names.get(&index).cloned().unwrap_or_default(),
            title: String::new(),
        })
        .collect()
}

pub fn shown(text: &str, recording: &Recording) -> String {
    let plain = roster(recording);
    let mut all = plain.clone();
    for number in scope_numbers(text) {
        all.extend(plain.iter().cloned().map(|who| Who {
            recording: Some(number),
            title: recording.title.clone(),
            ..who
        }));
    }
    speakers::pieces(text, &all, unnamed)
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.to_string(),
            Piece::Mention { text, .. } => text,
        })
        .collect()
}

fn scope_numbers(text: &str) -> Vec<u32> {
    let mut numbers: Vec<u32> = text
        .match_indices('@')
        .filter_map(|(at, _)| {
            let rest = text[at + 1..].strip_prefix('s')?;
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let rest = rest[digits..].strip_prefix('#')?;
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            rest[..digits].parse().ok().filter(|n| *n > 0)
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::transcript::Reply;
    use std::collections::BTreeMap;

    fn recording(names: &[(u32, &str)], speakers: &[u32], title: &str) -> Recording {
        let mut map = BTreeMap::new();
        for (index, name) in names {
            map.insert(*index, (*name).to_string());
        }
        Recording {
            title: title.into(),
            duration: 100.0,
            diarized: true,
            names: map,
            replies: speakers
                .iter()
                .map(|speaker| Reply {
                    start: 0.0,
                    end: 1.0,
                    speaker: Some(*speaker),
                    words: vec![],
                })
                .collect(),
            chapters: vec![],
            position: None,
        }
    }

    #[test]
    fn labels_become_the_names_the_chat_chips_show() {
        let rec = recording(&[(0, "Антон")], &[0, 1], "Запись");
        assert_eq!(
            shown("сказал @s1 и @s2, повторил @s1.", &rec),
            "сказал @Антон и @Говорящий 2, повторил @Антон."
        );
        assert_eq!(
            shown("сказал @s1", &recording(&[], &[0], "")),
            "сказал @Говорящий 1"
        );
    }

    #[test]
    fn a_name_typed_in_the_text_stays_typed() {
        let named = recording(&[(0, "Олег")], &[0], "");
        assert_eq!(shown("привет @Антон", &named), "привет @Антон");
        assert_eq!(shown("@Антон, скажи", &named), "@Антон, скажи");
    }

    #[test]
    fn a_folder_label_finds_the_name_and_the_recording_title() {
        let rec = recording(&[(0, "Антон"), (1, "Анна")], &[0, 1], "Планёрка");
        assert_eq!(
            shown("см. @s2#3 и @s1#7", &rec),
            "см. @Анна (Планёрка) и @Антон (Планёрка)"
        );
        assert_eq!(
            shown("см. @s2#3", &recording(&[], &[1], "Планёрка")),
            "см. @Говорящий 2 (Планёрка)"
        );
        assert_eq!(
            shown("см. @s2#3", &recording(&[], &[1], "")),
            "см. @Говорящий 2"
        );
    }

    #[test]
    fn a_label_inside_an_address_is_not_a_mention() {
        let rec = recording(&[(0, "Антон")], &[0], "");
        assert_eq!(shown("пиши a@s1.ru", &rec), "пиши a@s1.ru");
    }
}
