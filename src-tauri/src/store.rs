// The data files, in the same format as StudyNotch on the Mac, so a folder can move between the two:
//   subjects.json        the activities (name, minutes, break, category, choices, habit or not)
//   categories.json      the user's own categories (key, label, icon, colour)
//   study_sessions.csv   one row per session: the single source of truth for the report
// Windows: %APPDATA%\KaizeNotch\   macOS: ~/Library/Application Support/KaizeNotch/

use chrono::{DateTime, Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CSV_HEADER: &str =
    "id,subject,planned_min,actual_min,started_at,ended_at,completed,category,detail\n";
pub const BUILT_IN: [&str; 4] = ["academic", "health", "soul", "personal"];
pub const MAX_CATEGORY_LABEL: usize = 16;
pub const MAX_SUBJECT_NAME: usize = 24;

// Colours and icons handed out in turn to new categories (same order as the Mac app)
pub const PALETTE: [(&str, &str); 6] = [
    ("#8e6be8", "star"),
    ("#e8743b", "flame"),
    ("#2aa8c4", "leaf"),
    ("#e0574f", "heart"),
    ("#7fae2e", "bolt"),
    ("#5b6ee1", "music"),
];

pub fn default_folder() -> PathBuf {
    if let Ok(f) = std::env::var("KAIZENOTCH_DATA") {
        return PathBuf::from(f);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("KaizeNotch")
}

// MARK: activities

fn five() -> f64 {
    5.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    pub name: String,
    pub minutes: f64,
    #[serde(default = "five")]
    pub break_minutes: f64,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub needs_all: Option<bool>,
    #[serde(default)]
    pub is_habit: Option<bool>,
}

impl Subject {
    pub fn new(name: &str, minutes: f64, category: &str) -> Subject {
        Subject {
            name: name.to_string(),
            minutes,
            break_minutes: 5.0,
            category: Some(category.to_string()),
            options: Some(default_options(name)),
            needs_all: Some(default_needs_all(name)),
            is_habit: Some(category != "academic"),
        }
    }
    // Older files leave fields out; these are the Mac app's defaults
    pub fn category(&self) -> &str {
        self.category.as_deref().unwrap_or("academic")
    }
    pub fn options(&self) -> Vec<String> {
        self.options
            .clone()
            .unwrap_or_else(|| default_options(&self.name))
    }
    pub fn needs_all(&self) -> bool {
        self.needs_all
            .unwrap_or_else(|| default_needs_all(&self.name))
    }
    pub fn is_habit(&self) -> bool {
        self.is_habit.unwrap_or(self.category() != "academic")
    }

    // Every field written out, so the file reads the same in either app
    pub fn filled(&self) -> Subject {
        Subject {
            name: self.name.clone(),
            minutes: self.minutes,
            break_minutes: self.break_minutes,
            category: Some(self.category().to_string()),
            options: Some(self.options()),
            needs_all: Some(self.needs_all()),
            is_habit: Some(self.is_habit()),
        }
    }
}

// Pray / Solat: the five daily prayers, all needed. Sunnah: common sunnah prayers, any one counts.
pub fn default_options(name: &str) -> Vec<String> {
    let list: &[&str] = match name.to_lowercase().as_str() {
        "pray" | "solat" | "prayer" => &["Subuh", "Zohor", "Asar", "Maghrib", "Isyak"],
        "sunnah" | "sunat" | "solat sunat" | "sunnah prayer" | "nawafil" => {
            &["Dhuha", "Rawatib", "Tahajjud", "Witir", "Hajat"]
        }
        _ => &[],
    };
    list.iter().map(|s| s.to_string()).collect()
}

pub fn default_needs_all(name: &str) -> bool {
    !["sunnah", "sunat", "solat sunat", "sunnah prayer", "nawafil"]
        .contains(&name.to_lowercase().as_str())
}

// MARK: categories

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomCategory {
    pub key: String,
    pub label: String,
    pub icon: String,
    pub color: String,
}

// "Quran Club" -> "c-quran-club"; the "c-" prefix can never clash with a built-in key
pub fn category_key(label: &str, taken: &HashSet<String>) -> String {
    let slug: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = format!("c-{}", if slug.is_empty() { "category" } else { &slug });
    let (mut key, mut n) = (base.clone(), 2);
    while taken.contains(&key) {
        key = format!("{base}-{n}");
        n += 1;
    }
    key
}

pub fn built_in_label(key: &str) -> Option<&'static str> {
    match key {
        "academic" => Some("Academic"),
        "health" => Some("Health"),
        "soul" => Some("Soul"),
        "personal" => Some("Personal"),
        _ => None,
    }
}

// MARK: sessions

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    pub subject: String,
    pub category: String,
    pub detail: Option<String>,
    pub planned_min: f64,
    pub actual_min: f64,
    pub started_at: DateTime<Local>,
    pub ended_at: DateTime<Local>,
    pub completed: bool,
}

pub struct Store {
    pub folder: PathBuf,
}

impl Store {
    pub fn new(folder: &Path) -> Store {
        let _ = fs::create_dir_all(folder);
        Store {
            folder: folder.to_path_buf(),
        }
    }

    pub fn subjects_path(&self) -> PathBuf {
        self.folder.join("subjects.json")
    }
    pub fn sessions_path(&self) -> PathBuf {
        self.folder.join("study_sessions.csv")
    }
    pub fn categories_path(&self) -> PathBuf {
        self.folder.join("categories.json")
    }

    pub fn load_subjects(&self) -> Vec<Subject> {
        let Ok(text) = fs::read_to_string(self.subjects_path()) else {
            // first run: one of each kind, so every tab shows what it is for
            return vec![
                Subject::new("Study", 25.0, "academic"),
                Subject::new("Walk", 20.0, "health"),
                Subject::new("Read", 15.0, "personal"),
            ];
        };
        if let Ok(list) = serde_json::from_str::<Vec<Subject>>(&text) {
            return list;
        }
        // older file: just names, each 25 minutes of study
        if let Ok(names) = serde_json::from_str::<Vec<String>>(&text) {
            return names
                .iter()
                .map(|n| Subject::new(n, 25.0, "academic"))
                .collect();
        }
        vec![]
    }

    pub fn save_subjects(&self, list: &[Subject]) -> std::io::Result<()> {
        let filled: Vec<Subject> = list.iter().map(Subject::filled).collect();
        write_atomic(
            &self.subjects_path(),
            serde_json::to_string_pretty(&filled)?.as_bytes(),
        )
    }

    pub fn load_categories(&self) -> Vec<CustomCategory> {
        fs::read_to_string(self.categories_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save_categories(&self, list: &[CustomCategory]) -> std::io::Result<()> {
        write_atomic(
            &self.categories_path(),
            serde_json::to_string_pretty(list)?.as_bytes(),
        )
    }

    // Quotes around text fields so a comma in a name cannot break the columns
    pub fn append_session(&self, s: &NewSession) -> std::io::Result<()> {
        let quote = |t: &str| format!("\"{}\"", t.replace('"', "\"\""));
        let row = [
            uuid::Uuid::new_v4().to_string().to_uppercase(),
            quote(&s.subject),
            format!("{:.1}", s.planned_min),
            format!("{:.1}", s.actual_min),
            s.started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            s.ended_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            if s.completed { "1".into() } else { "0".into() },
            s.category.clone(),
            s.detail.as_deref().map(quote).unwrap_or_default(),
        ]
        .join(",")
            + "\n";
        let path = self.sessions_path();
        let new = !path.exists();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        if new {
            f.write_all(CSV_HEADER.as_bytes())?;
        }
        f.write_all(row.as_bytes())
    }

    // Rows as column-name -> value maps (header-driven, so older files with fewer columns still read)
    pub fn read_rows(&self) -> Vec<HashMap<String, String>> {
        let Ok(text) = fs::read_to_string(self.sessions_path()) else {
            return vec![];
        };
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return vec![];
        };
        let names = parse_csv_line(header.trim_start_matches('\u{feff}'));
        lines
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let cols = parse_csv_line(l);
                names.iter().cloned().zip(cols).collect()
            })
            .collect()
    }

    // Minutes today per activity, and "name|choice" for each choice finished today (Pray|Subuh)
    pub fn today(&self) -> (HashMap<String, f64>, HashSet<String>) {
        let today = Local::now().date_naive();
        let (mut mins, mut done) = (HashMap::new(), HashSet::new());
        for r in self.read_rows() {
            let (Some(subject), Some(started)) = (
                r.get("subject"),
                r.get("started_at").and_then(|s| parse_time(s)),
            ) else {
                continue;
            };
            if started.date_naive() != today {
                continue;
            }
            if let Some(m) = r.get("actual_min").and_then(|m| m.parse::<f64>().ok()) {
                *mins.entry(subject.clone()).or_insert(0.0) += m;
            }
            let detail = r.get("detail").map(|d| d.trim()).unwrap_or("");
            if !detail.is_empty() && r.get("completed").map(String::as_str) == Some("1") {
                done.insert(format!("{subject}|{detail}"));
            }
        }
        (mins, done)
    }
}

pub fn parse_time(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Local))
}

pub fn parse_csv_line(line: &str) -> Vec<String> {
    let (mut out, mut field, mut quoted) = (vec![], String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' && chars.peek() == Some(&'"') {
                field.push('"');
                chars.next();
            } else if c == '"' {
                quoted = false;
            } else {
                field.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == ',' {
            out.push(std::mem::take(&mut field));
        } else if c != '\r' {
            field.push(c);
        }
    }
    out.push(field);
    out
}

// Write to a temp file then rename, so a crash never leaves half a file
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        (dir, store)
    }

    #[test]
    fn csv_quotes_and_commas_round_trip() {
        assert_eq!(
            parse_csv_line(r#"1,"Read, write","say ""hi""",,x"#),
            vec!["1", "Read, write", "say \"hi\"", "", "x"]
        );
    }

    #[test]
    fn old_subject_files_get_the_mac_defaults() {
        let (_d, store) = temp();
        fs::write(
            store.subjects_path(),
            r#"[{"name":"Pray","minutes":5,"category":"soul"},{"name":"SQL","minutes":25}]"#,
        )
        .unwrap();
        let list = store.load_subjects();
        assert_eq!(list[0].options().len(), 5);
        assert!(list[0].is_habit());
        assert_eq!(list[1].category(), "academic");
        assert!(!list[1].is_habit());
        assert_eq!(list[1].break_minutes, 5.0);
        // names-only file from the first version
        fs::write(store.subjects_path(), r#"["Swift","SQL"]"#).unwrap();
        assert_eq!(store.load_subjects().len(), 2);
    }

    #[test]
    fn sessions_append_and_count_today() {
        let (_d, store) = temp();
        let now = Local::now();
        for detail in [Some("Subuh"), Some("Zohor"), None] {
            store
                .append_session(&NewSession {
                    subject: if detail.is_some() {
                        "Pray".into()
                    } else {
                        "SQL, part 2".into()
                    },
                    category: if detail.is_some() {
                        "soul".into()
                    } else {
                        "academic".into()
                    },
                    detail: detail.map(String::from),
                    planned_min: 5.0,
                    actual_min: 5.0,
                    started_at: now - chrono::Duration::minutes(5),
                    ended_at: now,
                    completed: true,
                })
                .unwrap();
        }
        let text = fs::read_to_string(store.sessions_path()).unwrap();
        assert!(text.starts_with(CSV_HEADER));
        let (mins, done) = store.today();
        assert_eq!(mins["Pray"], 10.0);
        assert_eq!(mins["SQL, part 2"], 5.0);
        assert!(done.contains("Pray|Subuh") && done.contains("Pray|Zohor"));
    }

    #[test]
    fn category_keys_are_slugs_and_unique() {
        let taken: HashSet<String> = ["c-quran-club".to_string()].into();
        assert_eq!(category_key("Quran Club", &HashSet::new()), "c-quran-club");
        assert_eq!(category_key("Quran  Club!", &taken), "c-quran-club-2");
        assert_eq!(category_key("!!!", &HashSet::new()), "c-category");
    }
}
