//! The prompts of the models, kept as templates in `crates/agent/prompts/` (MiniJinja: Jinja
//! syntax with `{% if %}`, `{% for %}` and `{% include %}`) so they read apart from the code that
//! fills them. They are built into the binary; the snapshot tests show each one rendered.

use minijinja::{Environment, UndefinedBehavior};
use serde::Serialize;
use std::sync::LazyLock;

static ENV: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    // A missing value is a bug, not an empty string; block tags leave no blank lines behind.
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    // Every `.j2` file under `prompts/`, embedded by build.rs, named by its path there.
    minijinja_embed::load_templates!(&mut env);
    env
});

/// The prompt `name` (a path under `prompts/`) filled with `ctx`.
///
/// Templates are part of the binary: a failure is a bug, caught by the tests.
pub fn render(name: &str, ctx: impl Serialize) -> String {
    ENV.get_template(name).and_then(|t| t.render(ctx)).unwrap_or_else(|e| panic!("prompt template {name}: {e:#}"))
}

/// The description of a tool for the model: `prompts/tools/<name>.j2`.
pub fn tool(name: &str) -> String {
    render(&format!("tools/{name}.j2"), ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    #[test]
    fn every_template_parses() {
        let names: Vec<&str> = ENV.templates().map(|(name, _)| name).collect();
        assert!(names.contains(&"agent/system.j2") && names.contains(&"tools/folder/search.j2"), "{names:?}");
    }

    #[test]
    #[should_panic(expected = "undefined")]
    fn a_missing_value_is_an_error() {
        render("delegate_task.j2", context! { task => "t" });
    }

    #[test]
    fn nudges() {
        insta::assert_snapshot!("answer_now", render("nudges/answer_now.j2", ()));
        insta::assert_snapshot!("cut_off", render("nudges/cut_off.j2", ()));
        insta::assert_snapshot!("bad_citations", render("nudges/bad_citations.j2", context! { bad => ["[42:00]", "[1:02:03]"], kind => "recording" }));
    }

    #[test]
    fn memory() {
        // Exactly what the code wrote before the move to templates.
        assert_eq!(render("memory.j2", context! { facts => ["a", "b"], summary => "s" }), "\nRemembered in this chat:\n- a\n- b\n\nThe earlier conversation, summarised:\ns\n");
        assert_eq!(render("memory.j2", context! { facts => Vec::<String>::new(), summary => "" }), "");
        insta::assert_snapshot!("memory_fold_system", render("memory_fold_system.j2", ()));
        let exchanges = [context! { question => "Кто ведёт?", answer => "Анна [0:10]." }, context! { question => "Когда отчёт?", answer => "В пятницу [1:20]." }];
        insta::assert_snapshot!("memory_fold_task", render("memory_fold_task.j2", context! { summary => "Говорили о сроках.", exchanges }));
        assert!(!render("memory_fold_task.j2", context! { summary => "", exchanges }).contains("Summary so far"));
    }

    #[test]
    fn scan() {
        let system = render("scan/system.j2", context! { json_only => false });
        assert!(system.contains("an empty list is a fine answer."));
        assert!(system.ends_with("Speakers are labels (@s1, @s2…): name a speaker in a fact only by the label."), "{system}");
        assert_eq!(render("scan/system.j2", context! { json_only => true }), format!("{system} Answer with the JSON object only."));
        insta::assert_snapshot!("scan_system", system);
        let part = render("scan/part.j2", context! { instruction => "все сроки", number => 2, parts => 5, start => "5:00", end => "10:30", text => "[5:10] @s1: Отчёт в пятницу." });
        assert_eq!(part, "Instruction: все сроки\n\nPart 2 of 5 (5:00 – 10:30):\n[5:10] @s1: Отчёт в пятницу.\n\nReturn a JSON object {\"facts\": [{\"time\": \"m:ss\", \"text\": \"...\"}]}.");
    }

    #[test]
    fn outline() {
        insta::assert_snapshot!("outline_system", render("outline/system.j2", ()));
        let tail = "\n\nWrite a JSON object:\ntitle — 3 to 8 words naming the subject, specific to this conversation (not \"Discussion\" or \"Questions\");\nsummary — 1 to 3 sentences: what is discussed and what came of it;\ndecisions — what was decided (empty when nothing was decided);\nactions — who will do what (a speaker by the label) (empty when nothing);\nquestions — questions left open (empty when none);\ntopics — 1 to 5 short keywords;\nquotes — 0 to 3 key lines, copied word for word from the transcript above, each with the time of its line (empty when nothing stands out).";
        let part = |previous: Option<&str>, number, parts| {
            render("outline/part.j2", context! { number, parts, start => "20 мин", end => "40 мин", language => "Russian", previous, text => "[20:00] Анна: Сроки." })
        };
        let lines = "one line per reply, \"[time] @label: text\" (each speaker is a label: @s1, @s2…; write the label wherever you mean that speaker):\n[20:00] Анна: Сроки.";
        assert_eq!(
            part(None, 2, 3),
            format!("Part 2 of 3 of one topic of the recording (20 мин – 40 мин).\nWrite everything in Russian.\n\nTranscript (this part), {lines}{tail}")
        );
        assert_eq!(
            part(Some("бюджет"), 2, 3),
            format!("Part 2 of 3 of one topic of the recording (20 мин – 40 мин).\nWrite everything in Russian.\nThe previous part of this topic was about: бюджет\nDescribe what this part adds; do not repeat the previous part.\n\nTranscript (this part), {lines}{tail}")
        );
        // The first part of a topic sees the previous topic.
        assert_eq!(
            part(Some("бюджет"), 1, 3),
            format!("Part 1 of 3 of one topic of the recording (20 мин – 40 мин).\nWrite everything in Russian.\nThe previous topic of the recording was: бюджет\nName this topic by what is new in it; do not reuse that title.\n\nTranscript (this part), {lines}{tail}")
        );
        // A topic that fits one window has no part number and is read as a whole.
        assert_eq!(
            part(None, 1, 1),
            format!("One topic of the recording (20 мин – 40 мин).\nWrite everything in Russian.\n\nTranscript (the whole topic), {lines}{tail}")
        );

        let parts = [
            context! { number => 1, title => "Отчёт", summary => "Сроки.", decisions => ["Сдать в пятницу"], actions => Vec::<String>::new(), questions => Vec::<String>::new(), topics => ["сроки", "отчёт"] },
            context! { number => 2, title => "Бюджет", summary => "Цифры.", decisions => Vec::<String>::new(), actions => ["Олег проверит"], questions => ["Когда созвон?"], topics => Vec::<String>::new() },
        ];
        insta::assert_snapshot!("outline_reduce_system", render("outline/reduce_system.j2", ()));
        let reduce = render("outline/reduce.j2", context! { language => "Russian", previous => Option::<String>::None, parts });
        insta::assert_snapshot!("outline_reduce", reduce);
        assert!(reduce.contains("[1] Отчёт — Сроки.\nDecisions: Сдать в пятницу\n") && reduce.contains("[2] Бюджет — Цифры.\nActions: Олег проверит\n"), "{reduce}");
        assert!(!reduce.contains("Part 2"), "empty lists leave no line");
        assert!(!reduce.contains("previous topic"), "the first topic has none: {reduce}");
        // After a merge the previous topic is named, so the new title does not repeat it.
        let after = render("outline/reduce.j2", context! { language => "Russian", previous => "Смета", parts });
        assert!(after.contains("The previous topic of the recording was: Смета\nName this topic by what is new in it; do not reuse that title.\n"), "{after}");

        insta::assert_snapshot!("outline_pair_system", render("outline/pair_system.j2", ()));
        let first = context! { start => "0:00", end => "10:00", title => "Отчёт", summary => "Сроки.", decisions => ["Сдать в пятницу"], actions => Vec::<String>::new(), questions => Vec::<String>::new(), topics => ["сроки", "отчёт"] };
        let second = context! { start => "10:00", end => "20:00", title => "Бюджет", summary => "Цифры.", decisions => Vec::<String>::new(), actions => ["Олег проверит"], questions => ["Когда созвон?"], topics => Vec::<String>::new() };
        let pair = render("outline/pair.j2", context! { first, second });
        insta::assert_snapshot!("outline_pair", pair);
        assert!(pair.starts_with(
            "Two neighbouring chapters of the recording.\n\nChapter A (0:00 – 10:00):\nTitle: Отчёт\nSummary: Сроки.\nDecisions: Сдать в пятницу\nTopics: сроки; отчёт\n\n\
             Chapter B (10:00 – 20:00):\nTitle: Бюджет\nSummary: Цифры.\nActions: Олег проверит\nQuestions: Когда созвон?\n\nIs chapter B the same subject as chapter A, continued?"
        ), "{pair}");
        assert!(pair.ends_with("Write a JSON object:\nsame — true or false, as above."), "{pair}");
    }

    /// The details templates carry the topic and its text (or its points); their wording is a
    /// placeholder until the lead writes it (P3a), so only the data wiring is tested.
    #[test]
    fn outline_details() {
        insta::assert_snapshot!("outline_detail_system", render("outline/detail_system.j2", ()));
        let window = render(
            "outline/detail_window.j2",
            context! { title => "Отчёт", summary => "Сроки.", start => "0:00", end => "10:00", number => 2, parts => 3, language => "Russian", text => "[0:10] Анна: Сроки." },
        );
        insta::assert_snapshot!("outline_detail_window", window);
        assert!(window.starts_with("Part 2 of 3 of one topic of the recording (0:00 – 10:00).\nThe topic: Отчёт — Сроки.\nWrite everything in Russian.\n\n"), "{window}");
        assert!(window.contains("\"[time] @label: text\" (each speaker is a label: @s1, @s2…; write the label wherever you mean that speaker):\n[0:10] Анна: Сроки.\n\nList the concrete points"), "{window}");

        let short = render(
            "outline/detail.j2",
            context! { title => "Отчёт", summary => "Сроки.", start => "0:00", end => "10:00", language => "Russian", parts => Vec::<String>::new(), points => Vec::<String>::new(), text => "[0:10] Анна: Сроки." },
        );
        insta::assert_snapshot!("outline_detail_transcript", short);
        assert!(short.starts_with("One topic of the recording (0:00 – 10:00).\nThe topic: Отчёт — Сроки.\nWrite everything in Russian.\n\nTranscript of the topic"), "{short}");
        assert!(!short.contains("subtopics") && !short.contains("The points said"), "{short}");

        let points = [context! { time => "0:10", text => "Сроки обсудили" }, context! { time => "0:20", text => "Олег проверит" }];
        let parts = [context! { start => "0:00", end => "5:00", title => "Сроки" }, context! { start => "5:00", end => "10:00", title => "Бюджет" }];
        let assembled = render(
            "outline/detail.j2",
            context! { title => "Отчёт", summary => "Сроки.", start => "0:00", end => "10:00", language => "Russian", parts, points, text => "" },
        );
        insta::assert_snapshot!("outline_detail_points", assembled);
        assert!(
            assembled.contains("It joins these subtopics: 0:00–5:00 Сроки; 5:00–10:00 Бюджет\n\nThe points said in the topic, in order (\"time text\"):\n0:10 Сроки обсудили\n0:20 Олег проверит\n"),
            "{assembled}"
        );
        assert!(assembled.contains("The subtopics may become sections") && !assembled.contains("Transcript of the topic"), "{assembled}");

        // The corrective ask after a cut-off breakdown; `tokens`
        // is the limit the previous answer hit.
        let shorter = render("outline/detail_shorter.j2", context! { tokens => 1819 });
        insta::assert_snapshot!("outline_detail_shorter", shorter);
        assert_eq!(
            shorter,
            "Your answer was cut off at the limit of 1819 tokens, so it is not valid JSON. Write the whole answer again, shorter, so that it fits: fewer and shorter items, the same sections, nothing new."
        );
    }

    fn tools() -> Vec<minijinja::Value> {
        vec![context! { name => "search", description => tool("search"), example => r#"{"query":"сроки"}"# }]
    }

    #[test]
    fn agent_system_prompt() {
        let topics = [
            context! { number => 1, start => "0:00", end => "5:00", title => "Сроки", summary => "Когда сдавать." },
            context! { number => 2, start => "5:00", end => "27:10", title => "Бюджет", summary => "Сколько." },
        ];
        let card = render("agent/card_recording.j2", context! { title => "Планёрка", length => "27:10", minutes => 27.2, speakers => ["@s1", "@s2"], topics, position => "3:15" });
        let plain = render("agent/system.j2", context! { kind => "recording", web => false, card, tools => tools(), facts => Vec::<String>::new(), summary => "" });
        insta::assert_snapshot!("agent_system_recording", plain);
        assert!(!plain.contains("web_search") && plain.contains("Cite the moments you rely on as [m:ss], with times"));
        let pointed = "cite the replies.\n- The user may point at a part of the recording:";
        assert!(plain.contains(pointed), "the part-of-the-recording rule follows the topic map: {plain}");
        assert!(plain.contains("in playback.\n\nTools:\n- search: "), "one blank line between the card and the tools");

        let card = render(
            "agent/card_folder.j2",
            context! { items => [
                context! { title => "Планёрка", length => "27:10", transcribed => true, chapters => ["Сроки", "Бюджет"] },
                context! { title => "Звонок", length => "3:05", transcribed => true, chapters => Vec::<String>::new() },
                context! { title => "Новое", length => "1:00", transcribed => false, chapters => Vec::<String>::new() },
            ] },
        );
        let full = render("agent/system.j2", context! { kind => "folder", web => true, card, tools => tools(), facts => ["Анна — руководитель"], summary => "Говорили о сроках." });
        insta::assert_snapshot!("agent_system_folder_web_memory", full);
        assert!(full.contains("#1 «Планёрка», 27:10. About: Сроки; Бюджет\n#2 «Звонок», 3:05\n#3 «Новое», 1:00 — not transcribed yet\n"));
        assert!(full.contains(pointed), "the part-of-the-recording rule follows the topic map: {full}");
        assert!(full.contains("- When the user asks to check the internet") && full.contains("[#2 12:30] (the recording's number"));
        assert!(full.contains("\n\nRemembered in this chat:\n- Анна — руководитель\n\nThe earlier conversation, summarised:\nГоворили о сроках."));
    }

    #[test]
    fn recording_card_without_speakers_or_outline() {
        let card = render("agent/card_recording.j2", context! { title => "Звонок", length => "1:02:03", minutes => 62.0, speakers => Vec::<String>::new(), topics => Vec::<String>::new(), position => () });
        assert_eq!(card, "Recording: «Звонок», 1:02:03 long (long).\nSpeakers were not told apart.\nNo outline yet: use search and read.\n");
    }

    #[test]
    fn results() {
        assert_eq!(render("results/plan.j2", context! { items => [context! { text => "найти", done => true }, context! { text => "написать", done => false }] }), "Plan:\n[x] найти\n[ ] написать");
        assert_eq!(render("results/remember.j2", context! { new => false }), "Already remembered (or empty).");
        assert!(render("results/say_uncited.j2", context! { kind => "folder" }).contains("as [#2 12:30]"));
        assert!(render("results/step.j2", context! { step => 3, tool => "search", text => "x" }).starts_with("Result of step 3 (search):\nx"));
        let hits = [context! { title => "ТК РФ", url => "https://example.org", snippet => "Статья 66.1" }];
        assert_eq!(render("results/web_search.j2", context! { hits }), "- ТК РФ — https://example.org\n  Статья 66.1\nCite pages as [title](address).\n");
        assert_eq!(render("results/web_search.j2", context! { hits => Vec::<String>::new() }), "Nothing found on the web.\n");
        let page = |empty| render("results/web_page.j2", context! { title => "T", url => "u", text => "текст", empty });
        assert_eq!(page(false), "T — u\n\nтекст");
        assert_eq!(page(true), "T — u\n\nтекст\n\n(Almost no text: the site loads it with a script. Read another address.)");
        for name in ["outline", "search", "read", "mentions", "speakers", "calculator", "time", "scan", "rename_speakers", "delegate", "remember", "plan", "say", "web_search", "web_read", "folder/recordings", "folder/search"] {
            assert!(!tool(name).is_empty(), "{name}");
        }
    }

    #[test]
    fn recording_results_and_errors() {
        // Exactly what the code wrote before the move to templates.
        let chapters = [
            context! { start => "0:00", end => "10:00", title => "Отчёт", summary => "Сроки.", sections => Vec::<String>::new() },
            context! { start => "10:00", end => "20:00", title => "Бюджет", summary => "Цифры.", sections => Vec::<String>::new() },
        ];
        assert_eq!(render("results/recording/outline.j2", context! { chapters }), "[0:00–10:00] Отчёт — Сроки.\n[10:00–20:00] Бюджет — Цифры.");
        let hits = [context! { start => "5:00", end => "5:30", who => ["@s1", "@s2"], text => "Бюджет." }, context! { start => "6:00", end => "6:30", who => Vec::<String>::new(), text => "Ещё." }];
        assert_eq!(render("results/recording/search.j2", context! { hits }), "[5:00–5:30] (@s1, @s2) Бюджет.\n\n[6:00–6:30] () Ещё.");
        assert_eq!(render("results/recording/line.j2", context! { time => "0:10", speaker => "@s1", text => "Привет." }), "[0:10] @s1: Привет.");
        assert_eq!(render("results/recording/line.j2", context! { time => "0:10", speaker => (), text => "Привет." }), "[0:10] Привет.");
        let speakers = [context! { name => "@s1", talk => "0:07", replies => 2, first => "0:10" }, context! { name => "@s2", talk => "1:00", replies => 1, first => "2:00" }];
        assert_eq!(render("results/recording/speakers.j2", context! { speakers }), "@s1: talks 0:07 in 2 replies, first at [0:10]\n@s2: talks 1:00 in 1 replies, first at [2:00]");
        let found = [context! { time => "0:10", speaker => "@s1", context => "а б" }, context! { time => "0:20", speaker => (), context => "в г" }];
        let mentions = |more| render("results/recording/mentions.j2", context! { query => "отчёт", count => 3, first => "0:10", last => "6:45", more, found => found.clone() });
        assert_eq!(mentions(0), "«отчёт»: 3 mentions, first at [0:10], last at [6:45].\n[0:10] @s1: …а б…\n[0:20] …в г…");
        assert_eq!(mentions(4), "«отчёт»: 3 mentions, first at [0:10], last at [6:45].\n[0:10] @s1: …а б…\n[0:20] …в г…\n… and 4 more");
        assert_eq!(render("results/recording/named.j2", context! { done => [context! { speaker => "@s2", name => "Олег" }, context! { speaker => "@s1", name => "Анна" }] }), "Named: @s2 → Олег, @s1 → Анна.");
        assert_eq!(render("results/recording/no_outline.j2", ()), "No outline yet (it is being built). Use search and read.");
        assert_eq!(render("results/recording/nothing.j2", context! { query => "q" }), "Nothing found for «q». Try other words or read the outline.");
        assert_eq!(render("results/recording/silence.j2", context! { start => "2:00", end => "3:00" }), "Nobody speaks between [2:00] and [3:00].");
        assert_eq!(render("results/recording/stopped.j2", context! { time => "1:00" }), "… (stopped here; read on from [1:00])");
        assert_eq!(render("results/recording/never_said.j2", context! { query => "q" }), "«q» is never said (no word form of it). Try a synonym or search.");
        assert_eq!(render("results/recording/no_speakers.j2", ()), "Speakers were not told apart in this recording.");
        assert_eq!(render("results/recording/time.j2", context! { clock => "4:50", seconds => "290" }), "4:50 (290 seconds)");
        for (name, text) in [("chapters", "(more chapters: read by time)"), ("search", "(cut; read a range for the rest)"), ("plain", "(cut)"), ("scan", "(cut; ask a narrower instruction)")] {
            assert_eq!(render(&format!("results/cut/{name}.j2"), ()), text);
        }
        for (what, text) in [("query", "search needs a query"), ("names", "search needs names"), ("task", "search needs a task"), ("instruction", "search needs an instruction"), ("word", "search needs a word"), ("address", "search needs an address")] {
            assert_eq!(render(&format!("errors/needs_{what}.j2"), context! { tool => "search" }), text);
        }
        assert_eq!(render("errors/no_tool.j2", context! { tool => "dance" }), "there is no tool «dance»");
        assert_eq!(render("errors/not_a_time.j2", context! { value => "вчера" }), "\"вчера\" is not a time (use m:ss)");
        assert_eq!(render("errors/recording_length.j2", context! { length => "10:00" }), "the recording is only 10:00 long");
        assert_eq!(render("errors/empty_name.j2", context! { label => "X" }), "an empty name for «X»");
        assert_eq!(render("errors/no_speaker.j2", context! { label => "X" }), "there is no speaker «X»");
        assert_eq!(render("errors/cannot_rename.j2", ()), "speakers cannot be named here");
        assert_eq!(render("errors/no_helper.j2", ()), "there is no helper");
        assert_eq!(render("errors/no_scan.j2", ()), "the scan is not available");
    }

    #[test]
    fn folder_results_and_errors() {
        let items = [
            context! { number => 1, title => "Планёрка", length => "10:00", transcribed => true, chapters => ["Отчёт", "Бюджет"] },
            context! { number => 2, title => "Созвон", length => "2:00", transcribed => true, chapters => Vec::<String>::new() },
            context! { number => 3, title => "Новое", length => "1:00", transcribed => false, chapters => Vec::<String>::new() },
        ];
        assert_eq!(render("results/folder/recordings.j2", context! { items }), "#1 «Планёрка», 10:00. Chapters: Отчёт; Бюджет\n#2 «Созвон», 2:00\n#3 «Новое», 1:00 — not transcribed yet");
        let hits = [context! { number => 2, start => "0:30", end => "0:40", title => "Созвон", who => ["@s2#2"], text => "Графики." }, context! { number => 1, start => "1:00", end => "1:10", title => "Планёрка", who => ["@s1#1"], text => "Отчёт." }];
        assert_eq!(render("results/folder/search.j2", context! { hits }), "[#2 0:30–0:40] «Созвон» (@s2#2) Графики.\n\n[#1 1:00–1:10] «Планёрка» (@s1#1) Отчёт.");
        assert_eq!(render("results/folder/mentions.j2", context! { parts => [context! { number => 1, text => "a" }, context! { number => 2, text => "b" }] }), "#1: a\n\n#2: b");
        assert_eq!(render("results/folder/nothing.j2", context! { query => "q" }), "Nothing found for «q» in the folder.");
        assert_eq!(render("results/folder/never_said.j2", ()), "Never said in any recording.");
        assert_eq!(render("errors/which_recording.j2", ()), "say which recording (its number from the list)");
        assert_eq!(render("errors/no_recording_number.j2", context! { number => 5, count => 2 }), "there is no recording #5 (there are 2)");
        assert_eq!(render("errors/no_recording_title.j2", context! { title => "x" }), "no recording is called «x»");
        assert_eq!(tool("folder/recording_param"), "the recording's number from the list");
    }

    #[test]
    fn scan_result() {
        let facts = [context! { time => "0:10", text => "Отчёт в пятницу" }, context! { time => "5:00", text => "Бюджет" }];
        let render_scan = |failed: Vec<minijinja::Value>, facts: &[minijinja::Value]| {
            render("results/scan.j2", context! { instruction => "все сроки", parts => 3, done => 3 - failed.len(), count => facts.len(), failed, facts })
        };
        assert_eq!(
            render_scan(vec![], &facts),
            "Scanned the whole recording (3 parts) for «все сроки»: 2 findings.\nFindings — the material for your answer: list the ones that answer the question, with their times.\n[0:10] Отчёт в пятницу\n[5:00] Бюджет\n"
        );
        let failed = vec![context! { start => "5:00", end => "10:00", why => "the model broke" }, context! { start => "10:00", end => "12:00", why => "x" }];
        assert_eq!(
            render_scan(failed, &facts[..1]),
            "Scanned 1 of 3 parts for «все сроки»: 1 findings. NOT checked: [5:00–10:00] (the model broke), [10:00–12:00] (x).\nFindings — the material for your answer: list the ones that answer the question, with their times.\n[0:10] Отчёт в пятницу\n"
        );
        assert_eq!(render_scan(vec![], &[]), "Scanned the whole recording (3 parts) for «все сроки»: 0 findings.\nNothing in the recording matches.\n");
    }

    #[test]
    fn calculator_time_and_provider_texts() {
        assert_eq!(render("errors/arguments_not_json.j2", context! { tool => "search", error => "EOF", raw => "{" }), "the arguments of search are not JSON (EOF): {");
        assert_eq!(render("errors/empty_expression.j2", ()), "empty expression");
        assert_eq!(render("errors/cannot_evaluate.j2", context! { expr => "2+", error => "bad" }), "cannot evaluate \"2+\": bad");
        assert_eq!(render("errors/not_a_clock_time.j2", context! { time => "x" }), "\"x\" is not a time (use 5:59, 1:02:30 or seconds)");
        assert!(crate::tools::between("x", "1:00").unwrap_err().to_string().contains("\"x\" is not a time"));
        assert!(crate::tools::calculate("").unwrap_err().to_string().contains("empty expression"));
    }

    #[test]
    fn delegate() {
        insta::assert_snapshot!("delegate_system", render("delegate_system.j2", ()));
        insta::assert_snapshot!("delegate_task", render("delegate_task.j2", context! { task => "Кратко перескажи", text => "[0:10] (Анна) Отчёт в пятницу." }));
    }

    #[test]
    fn chat_title() {
        insta::assert_snapshot!("chat_title_system", render("chat/title_system.j2", ()));
        let user = render("chat/title.j2", context! { language => "Russian", question => "привет как дела" });
        insta::assert_snapshot!("chat_title", user);
        assert!(user.starts_with("The first message of a chat:
привет как дела

Write its title in Russian:"), "{user}");
        assert!(user.ends_with("Title:") && !user.contains("→"), "no example to copy: {user}");
    }

    #[test]
    fn quote_and_step_title_prompts() {
        let quote = render("agent/quote.j2", context! { label => "@s1", start => "1:40", end => "2:29", text => "Отчёт в пятницу" });
        insta::assert_snapshot!("agent_quote", quote);
        assert_eq!(quote.trim_end_matches('\n'), "(quoting @s1 1:40–2:29: \"Отчёт в пятницу\")");
        let system = render("chat/step_title_system.j2", ());
        insta::assert_snapshot!("step_title_system", system);
        assert!(system.contains("Keep speaker labels like @s1 as they are."));
        let phrase = render(
            "chat/step_title.j2",
            context! { question => "Когда отчёт?", tool => "search", purpose => "Finds places.", arguments => "{\"query\":\"отчёт\"}", language => "Russian" },
        );
        insta::assert_snapshot!("step_title", phrase);
        assert!(phrase.contains("The question: Когда отчёт?\nThe tool: search — Finds places.\nIts arguments: {\"query\":\"отчёт\"}\n\nWrite in Russian"), "{phrase}");
        assert!(phrase.trim_end().ends_with("Phrase:"), "{phrase}");
        assert!(!phrase.contains("Ищу"), "no example a small model could copy: {phrase}");
    }

    #[test]
    fn verify_prompt_carries_the_draft_and_its_evidence() {
        insta::assert_snapshot!("agent_verify_system", render("agent/verify_system.j2", ()));
        let full = render("agent/verify.j2", context! { question => "Когда?", draft => "В пятницу [0:10].", fragments => "[0:10] Дана: Отчёт в пятницу.", web => "https://example.org — закон" });
        insta::assert_snapshot!("agent_verify", full);
        assert!(full.starts_with("Question:
Когда?

Draft answer:
В пятницу [0:10].

What the recording says at the cited times:
[0:10] Дана: Отчёт в пятницу.

The web pages the draft cites:
https://example.org — закон

Check the draft"), "{full}");
        let bare = render("agent/verify.j2", context! { question => "Q", draft => "D", fragments => "", web => "" });
        assert!(bare.starts_with("Question:
Q

Draft answer:
D

Check the draft"), "{bare}");
        assert!(bare.ends_with("Put nothing else into the answer text."), "{bare}");
    }

    /// The judge templates carry the question, the reference answer, the facts and the agent's
    /// answer. The wording is a placeholder until the lead writes the rubric.
    #[test]
    fn judge_prompt_carries_the_case() {
        let system = render("eval/judge_system.j2", ());
        insta::assert_snapshot!("eval_judge_system", system);
        assert!(system.starts_with("You grade an assistant's answer") && system.ends_with("naming what is missing or wrong."), "{system}");
        let facts = ["Назаров", "три юриста"];
        let user = render(
            "eval/judge.j2",
            context! { question => "Как зовут?", reference => "Антон Назаров.", facts, answer => "Его зовут Назаров [12:00]." },
        );
        insta::assert_snapshot!("eval_judge", user);
        assert!(
            user.starts_with("Question:\nКак зовут?\n")
                && user.contains("Антон Назаров.")
                && user.contains("- Назаров\n- три юриста\n")
                && user.ends_with("Assistant's answer:\nЕго зовут Назаров [12:00]."),
            "{user}"
        );
        let bare = render("eval/judge.j2", context! { question => "Q", reference => "R", facts => Vec::<&str>::new(), answer => "A" });
        assert!(bare.contains("Question:\nQ\n") && bare.contains("Reference answer:\nR\n") && bare.contains("Assistant's answer:\nA") && !bare.contains("- "), "{bare}");
    }

    #[test]
    fn topic_map_card_and_outline_topic_result() {
        let topics = [
            context! { number => 1, start => "0:00", end => "5:00", title => "Отчёт", summary => "Сроки." },
            context! { number => 2, start => "5:00", end => "10:00", title => "Бюджет", summary => "Цифры." },
        ];
        let card = render(
            "agent/card_recording.j2",
            context! { title => "Планёрка", length => "10:00", minutes => 10.0, speakers => ["@s1"], topics, position => () },
        );
        assert_eq!(card, "Recording: «Планёрка», 10:00 long (medium).\nSpeakers: @s1.\nTopics (the map; outline with a topic number gives its breakdown):\n1. [0:00–5:00] Отчёт — Сроки.\n2. [5:00–10:00] Бюджет — Цифры.\n");
        let chapters = [
            context! { start => "0:00", end => "5:00", title => "Отчёт", summary => "Сроки.", sections => ["Советы", "Ошибки"] },
            context! { start => "5:00", end => "10:00", title => "Бюджет", summary => "Цифры.", sections => Vec::<String>::new() },
        ];
        assert_eq!(
            render("results/recording/outline.j2", context! { chapters }),
            "[0:00–5:00] Отчёт — Сроки. (Советы; Ошибки)\n[5:00–10:00] Бюджет — Цифры."
        );
        let sections = [context! {
            title => "Советы",
            kind => "list",
            items => [context! { time => "0:10", text => "Сдать в пятницу" }, context! { time => "", text => "Без времени" }],
        }];
        let quotes = [context! { time => "0:10", text => "В пятницу." }];
        assert_eq!(
            render(
                "results/recording/topic.j2",
                context! { number => 1, start => "0:00", end => "5:00", title => "Отчёт", summary => "Сроки.", sections, quotes },
            ),
            "Breakdown of topic\n1. [0:00–5:00] Отчёт — Сроки.\nСоветы (list)\n[0:10] Сдать в пятницу\nБез времени\n[0:10] В пятницу."
        );
        assert_eq!(
            render("results/recording/topic_empty.j2", context! { number => 2, title => "Бюджет" }).trim_end(),
            "Topic 2 «Бюджет» has no breakdown yet: use search and read within that topic."
        );
        assert_eq!(
            render("errors/no_topic.j2", context! { topic => "9", count => 2 }).trim_end(),
            "There is no topic 9: the recording has 2 topics, numbered from 1."
        );
        assert!(tool("outline").starts_with("The map of the recording:") && tool("outline").contains("With topic (a topic number)"));
        let system = render(
            "agent/system.j2",
            context! { kind => "recording", web => false, card => "CARD", tools => Vec::<minijinja::Value>::new(), facts => Vec::<String>::new(), summary => "" },
        );
        assert!(system.contains("- Start from the topic map in the card. When the question belongs to one or more topics, first read their breakdown"), "{system}");
    }
}
