//! Тексты на языках (шаг И.2в, `docs/game-ui.md`).
//!
//! В экране пишется ключ: `Text(tr("menu.title"))`, а слова лежат в файлах языков
//! `ui/lang/ru.ron`, `ui/lang/en.ron` — по файлу на язык, имя файла и есть код языка
//! (тот же, что в `.when(lang: en)`):
//!
//! ```text
//! (
//!     name: "English",
//!     texts: {
//!         "menu.title": "Main menu",
//!         "hud.coins": "Coins: {n}",
//!     },
//! )
//! ```
//!
//! Один язык главный (обычно русский): если в другом языке ключа нет, берётся слово
//! главного, а список таких пропусков виден в консоли. Ключа нет и в главном — это
//! ошибка в экране, с местом `tr` в файле.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::UiError;
use crate::syntax::Pos;

/// Все языки игры.
#[derive(Clone, Debug, Default)]
pub struct Texts {
    /// Первый — главный.
    langs: Vec<Lang>,
    files: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct Lang {
    code: String,
    name: String,
    file: String,
    words: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LangFile {
    name: String,
    texts: BTreeMap<String, String>,
}

impl Texts {
    /// Читает все `*.ron` из папки `dir` (обычно `ui/lang`). Главный язык — `main`, если
    /// такой файл есть, иначе первый по имени.
    pub fn load(dir: &Path, main: &str) -> Result<Self, UiError> {
        let folder = dir.file_name().map_or_else(
            || dir.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| file_error(&folder, &format!("папку не прочитать: {e}")))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "ron"))
            .collect();
        found.sort();
        if let Some(i) = found
            .iter()
            .position(|p| p.file_stem().is_some_and(|s| s == main))
        {
            let p = found.remove(i);
            found.insert(0, p);
        }
        let mut texts = Self::default();
        for p in found {
            let code = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = format!("{folder}/{code}.ron");
            let src = std::fs::read_to_string(&p)
                .map_err(|e| file_error(&name, &format!("файл не прочитать: {e}")))?;
            texts.add(&code, &src, &name)?;
            texts.files.push(p);
        }
        Ok(texts)
    }

    /// Добавляет язык `code` из текста файла `src` (`file` — его имя для ошибок).
    /// Первый добавленный — главный.
    pub fn add(&mut self, code: &str, src: &str, file: &str) -> Result<(), UiError> {
        if self.langs.iter().any(|l| l.code == code) {
            return Err(file_error(file, &format!("язык «{code}» уже есть")));
        }
        let parsed: LangFile = ron::from_str(src).map_err(|e| UiError::File {
            file: file.to_owned(),
            source: Box::new(UiError::Syntax {
                pos: Pos {
                    line: u32::try_from(e.span.start.line).unwrap_or(u32::MAX),
                    column: u32::try_from(e.span.start.col).unwrap_or(u32::MAX),
                    file: 0,
                },
                message: format!(
                    "файл языка пишется так: ( name: \"English\", texts: {{ \"menu.title\": \"Main menu\", }} ). Не так: {}",
                    e.code
                ),
            }),
        })?;
        if let Some(k) = parsed.texts.keys().find(|k| k.trim().is_empty()) {
            return Err(file_error(file, &format!("пустой ключ «{k}»")));
        }
        self.langs.push(Lang {
            code: code.to_owned(),
            name: parsed.name,
            file: file.to_owned(),
            words: parsed.texts.into_iter().collect(),
        });
        Ok(())
    }

    /// Нет ни одного языка.
    pub fn is_empty(&self) -> bool {
        self.langs.is_empty()
    }

    /// Языки по порядку: код (`ru`) и имя для игрока («Русский»). Первый — главный.
    pub fn languages(&self) -> impl Iterator<Item = (&str, &str)> {
        self.langs
            .iter()
            .map(|l| (l.code.as_str(), l.name.as_str()))
    }

    /// Прочитанные файлы (для слежки за правкой).
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Ключи главного языка, которых нет в других: «lang/en.ron: нет menu.save».
    /// Игра при этом покажет слово главного языка.
    pub fn gaps(&self) -> Vec<String> {
        let Some((main, rest)) = self.langs.split_first() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for l in rest {
            let mut missing: Vec<&str> = main
                .words
                .keys()
                .filter(|k| !l.words.contains_key(*k))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                missing.sort_unstable();
                out.push(format!(
                    "{}: нет {} (будет текст из {})",
                    l.file,
                    missing.join(", "),
                    main.file
                ));
            }
            let mut extra: Vec<&str> = l
                .words
                .keys()
                .filter(|k| !main.words.contains_key(*k))
                .map(String::as_str)
                .collect();
            if !extra.is_empty() {
                extra.sort_unstable();
                out.push(format!(
                    "{}: {} нет в главном языке {}",
                    l.file,
                    extra.join(", "),
                    main.file
                ));
            }
        }
        out
    }

    /// Слово по ключу на языке `lang` (нет языка или ключа в нём — из главного).
    /// Языков нет совсем — сам ключ: экран можно смотреть и без файлов языков.
    pub(crate) fn get(&self, lang: &str, key: &str, pos: Pos) -> Result<String, UiError> {
        let Some(main) = self.langs.first() else {
            return Ok(key.to_owned());
        };
        let chosen = self.langs.iter().find(|l| l.code == lang).unwrap_or(main);
        if let Some(w) = chosen.words.get(key).or_else(|| main.words.get(key)) {
            return Ok(w.clone());
        }
        let known: Vec<&str> = main.words.keys().map(String::as_str).collect();
        let hint = match crate::model::suggest(key, &known) {
            Some(s) => format!(" Может, tr(\"{s}\")?"),
            None => format!(" Добавьте его в {}.", main.file),
        };
        Err(UiError::Meaning {
            pos,
            message: format!("нет текста «{key}» в {}.{hint}", main.file),
        })
    }
}

fn file_error(file: &str, message: &str) -> UiError {
    UiError::File {
        file: file.to_owned(),
        source: Box::new(UiError::Meaning {
            pos: Pos::default(),
            message: message.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Texts {
        let mut t = Texts::default();
        t.add(
            "ru",
            r#"(name: "Русский", texts: { "a": "Привет", "b": "Пока" })"#,
            "lang/ru.ron",
        )
        .unwrap();
        t.add(
            "en",
            r#"(name: "English", texts: { "a": "Hello" })"#,
            "lang/en.ron",
        )
        .unwrap();
        t
    }

    #[test]
    fn words_come_from_the_language_or_the_main_one() {
        let t = two();
        let p = Pos::default();
        assert_eq!(t.get("en", "a", p).unwrap(), "Hello");
        assert_eq!(t.get("en", "b", p).unwrap(), "Пока");
        assert_eq!(t.get("de", "a", p).unwrap(), "Привет");
        let e = t.get("en", "aa", p).unwrap_err().to_string();
        assert!(
            e.contains("нет текста «aa»") && e.contains("tr(\"a\")"),
            "{e}"
        );
        assert_eq!(
            t.gaps(),
            ["lang/en.ron: нет b (будет текст из lang/ru.ron)"]
        );
        assert_eq!(
            t.languages().collect::<Vec<_>>(),
            [("ru", "Русский"), ("en", "English")]
        );
    }

    #[test]
    fn bad_file_names_the_file_and_line() {
        let mut t = Texts::default();
        let e = t
            .add(
                "en",
                "(\n name: \"English\",\n texts: { \"a\" \"b\" },\n)",
                "lang/en.ron",
            )
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("lang/en.ron: строка 3"), "{e}");
    }

    #[test]
    fn no_languages_shows_the_key() {
        let t = Texts::default();
        assert_eq!(
            t.get("ru", "menu.title", Pos::default()).unwrap(),
            "menu.title"
        );
    }
}
