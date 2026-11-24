// The habit report's data. A port of ReportBuilder.swift (itself a port of study_report.py's summarise()):
// same keys, same rules, same rounding, so the one HTML template (assets/report_template.html, copied
// from ApexAgent/scripts/study_report_template.html) and the shared web page draw either app's data.
//
// for_sharing: true leaves out the raw session rows: a shared page shows habits and totals, not when
// each session started and stopped.

use crate::store::{parse_time, CustomCategory, Store, Subject};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const TEMPLATE: &str = include_str!("../assets/report_template.html");
const DAYS: i64 = 14;
const QUICK_MIN: f64 = 1.0;
const HEAT_WEEKS: i64 = 26;

fn env(key: &str, fallback: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}
fn daily_goal() -> f64 {
    env("STUDY_GOAL_MIN", 100.0)
}

// Python round(x, 1): rounds the exact binary value, ties to even. Rust's "{:.1}" does the same.
pub fn r1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap_or(x)
}

// Python 3.12+ sum() of floats: Neumaier compensated summation (a plain sum can round differently)
pub fn pysum<I: IntoIterator<Item = f64>>(xs: I) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for x in xs {
        let t = s + x;
        c += if s.abs() >= x.abs() {
            (s - t) + x
        } else {
            (x - t) + s
        };
        s = t;
    }
    s + c
}

fn fmt(d: NaiveDate, pattern: &str) -> String {
    d.format(pattern).to_string()
}
fn add(d: NaiveDate, n: i64) -> NaiveDate {
    d + Duration::days(n)
}
fn week_of(today: NaiveDate) -> Vec<NaiveDate> {
    let monday = add(today, -(today.weekday().num_days_from_monday() as i64));
    (0..7).map(|i| add(monday, i)).collect()
}
fn num(x: f64) -> Value {
    json!(x)
}

#[derive(Clone)]
struct Row {
    subject: String,
    category: String,
    detail: String,
    planned: f64,
    actual: f64,
    start: DateTime<Local>,
    end: DateTime<Local>,
    done: bool,
}
impl Row {
    fn day(&self) -> NaiveDate {
        self.start.date_naive()
    }
}

struct Cat {
    key: String,
    label: String,
    goal: f64,
    color: Option<String>,
}

fn categories(custom: &[CustomCategory]) -> Vec<Cat> {
    let mut out = vec![
        Cat {
            key: "academic".into(),
            label: "Academic".into(),
            goal: daily_goal(),
            color: None,
        },
        Cat {
            key: "health".into(),
            label: "Health".into(),
            goal: env("STUDY_GOAL_HEALTH", 30.0),
            color: None,
        },
        Cat {
            key: "soul".into(),
            label: "Soul".into(),
            goal: env("STUDY_GOAL_SOUL", 25.0),
            color: None,
        },
        Cat {
            key: "personal".into(),
            label: "Personal".into(),
            goal: env("STUDY_GOAL_PERSONAL", 30.0),
            color: None,
        },
    ];
    out.extend(custom.iter().map(|c| Cat {
        key: c.key.clone(),
        label: c.label.clone(),
        goal: env("STUDY_GOAL_DEFAULT", 30.0),
        color: Some(c.color.clone()),
    }));
    out
}

fn load_rows(store: &Store, subjects: &[Subject], cat_keys: &HashSet<String>) -> Vec<Row> {
    let mut fallback: HashMap<&str, &str> = HashMap::new();
    for s in subjects {
        fallback.entry(&s.name).or_insert(s.category());
    }
    let mut rows = vec![];
    for r in store.read_rows() {
        let get = |k: &str| r.get(k).map(String::as_str);
        let (Some(subject), Some(planned), Some(actual), Some(start), Some(end)) = (
            get("subject"),
            get("planned_min").and_then(|v| v.parse::<f64>().ok()),
            get("actual_min").and_then(|v| v.parse::<f64>().ok()),
            get("started_at").and_then(parse_time),
            get("ended_at").and_then(parse_time),
        ) else {
            continue;
        };
        // The row's own category wins; old rows look it up; unknown names were study
        let mut cat = get("category")
            .filter(|c| !c.is_empty())
            .or(fallback.get(subject).copied())
            .unwrap_or("academic")
            .trim()
            .to_string();
        if !cat_keys.contains(&cat) {
            cat = "academic".into();
        }
        rows.push(Row {
            subject: subject.to_string(),
            category: cat,
            detail: get("detail").unwrap_or("").trim().to_string(),
            planned,
            actual,
            start,
            end,
            done: get("completed") == Some("1"),
        });
    }
    rows
}

// MARK: habits

struct Card {
    json: Value,
    done: HashSet<NaiveDate>,
}

#[allow(clippy::too_many_arguments)]
fn habit_card(
    name: &str,
    category: &str,
    mine: &[&Row],
    today: NaiveDate,
    target: f64,
    options: &[String],
    studied_counts: bool,
    needs_all: bool,
) -> Card {
    let mut mins: HashMap<NaiveDate, f64> = HashMap::new();
    let mut finished: HashMap<NaiveDate, HashSet<String>> = HashMap::new();
    for r in mine {
        *mins.entry(r.day()).or_insert(0.0) += r.actual;
        if r.done {
            finished
                .entry(r.day())
                .or_default()
                .insert(r.detail.clone());
        }
    }
    let opts: HashSet<String> = options.iter().cloned().collect();
    let quick: HashSet<NaiveDate> = mins
        .iter()
        .filter(|(_, m)| **m >= QUICK_MIN)
        .map(|(d, _)| *d)
        .collect();
    let (done, partial): (HashSet<NaiveDate>, HashSet<NaiveDate>) =
        if !options.is_empty() && !needs_all {
            let done: HashSet<_> = finished
                .iter()
                .filter(|(_, v)| !v.is_disjoint(&opts))
                .map(|(d, _)| *d)
                .collect();
            let partial = quick.difference(&done).copied().collect();
            (done, partial)
        } else if !options.is_empty() {
            let done: HashSet<_> = finished
                .iter()
                .filter(|(_, v)| opts.is_subset(v))
                .map(|(d, _)| *d)
                .collect();
            let partial = finished
                .iter()
                .filter(|(d, v)| !v.is_disjoint(&opts) && !done.contains(d))
                .map(|(d, _)| *d)
                .collect();
            (done, partial)
        } else if studied_counts {
            (
                quick
                    .union(&finished.keys().copied().collect())
                    .copied()
                    .collect(),
                HashSet::new(),
            )
        } else {
            let done: HashSet<_> = finished.keys().copied().collect();
            let partial = quick.difference(&done).copied().collect();
            (done, partial)
        };
    let mut streak = 0;
    let mut d = if done.contains(&today) {
        today
    } else {
        add(today, -1)
    };
    while done.contains(&d) {
        streak += 1;
        d = add(d, -1);
    }
    let week = week_of(today);
    let prev: Vec<NaiveDate> = week.iter().map(|d| add(*d, -7)).collect();
    let empty = HashSet::new();
    let day_json = |x: &NaiveDate| -> Value {
        let n = if options.is_empty() {
            Value::Null
        } else {
            json!(finished
                .get(x)
                .unwrap_or(&empty)
                .intersection(&opts)
                .count())
        };
        json!({"wd": fmt(*x, "%a"), "d": fmt(*x, "%a %d %b"), "m": num(r1(*mins.get(x).unwrap_or(&0.0))),
               "n": n, "future": *x > today, "today": *x == today,
               "s": if done.contains(x) { 2 } else if partial.contains(x) { 1 } else { 0 }})
    };
    let sum_of = |days: &[NaiveDate]| r1(pysum(days.iter().map(|d| *mins.get(d).unwrap_or(&0.0))));
    let today_done = finished.get(&today).unwrap_or(&empty);
    Card {
        json: json!({
            "name": name, "category": category, "target": num(target),
            "today": num(r1(*mins.get(&today).unwrap_or(&0.0))), "streak": streak,
            "weekDone": week.iter().filter(|d| done.contains(d)).count(),
            "weekMin": num(sum_of(&week)),
            "options": options, "needsAll": needs_all,
            "todayOptions": options.iter().filter(|o| today_done.contains(*o)).collect::<Vec<_>>(),
            "days": week.iter().map(day_json).collect::<Vec<_>>(),
            "prevDays": prev.iter().map(day_json).collect::<Vec<_>>(),
            "prevDone": prev.iter().filter(|d| done.contains(d)).count(),
            "prevMin": num(sum_of(&prev)),
        }),
        done,
    }
}

fn habits(rows: &[Row], subjects: &[Subject], cats: &[Cat], today: NaiveDate) -> Vec<Card> {
    let week_start = week_of(today)[0];
    let mut info: HashMap<&str, &Subject> = HashMap::new();
    for s in subjects {
        info.entry(&s.name).or_insert(s);
    }
    let habit_cats: HashSet<&str> = cats
        .iter()
        .map(|c| c.key.as_str())
        .filter(|k| *k != "academic")
        .collect();
    let academic: Vec<&Row> = rows.iter().filter(|r| r.category == "academic").collect();
    let mut out = vec![habit_card(
        "Study",
        "academic",
        &academic,
        today,
        daily_goal(),
        &[],
        true,
        true,
    )];

    let mut names: Vec<String> = subjects.iter().map(|s| s.name.clone()).collect();
    let extra: BTreeSet<String> = rows
        .iter()
        .filter(|r| r.day() >= week_start)
        .map(|r| r.subject.clone())
        .filter(|n| !names.contains(n))
        .collect();
    names.extend(extra);

    let mut cards = vec![];
    for name in &names {
        let mine: Vec<&Row> = rows.iter().filter(|r| &r.subject == name).collect();
        let latest = mine.iter().max_by_key(|r| r.start);
        let entry = info.get(name.as_str());
        let cat = entry
            .map(|e| e.category().to_string())
            .or(latest.map(|r| r.category.clone()))
            .unwrap_or_else(|| "academic".into());
        if !habit_cats.contains(cat.as_str()) {
            continue;
        }
        if entry.map(|e| e.is_habit()) == Some(false) {
            continue;
        } // time only (toilet, coffee)
        let target = entry
            .map(|e| e.minutes)
            .or(latest.map(|r| r.planned))
            .unwrap_or(25.0);
        let needs_all = entry
            .map(|e| e.needs_all())
            .unwrap_or_else(|| crate::store::default_needs_all(name));
        let options = entry.map(|e| e.options()).unwrap_or_default();
        cards.push(habit_card(
            name, &cat, &mine, today, target, &options, false, needs_all,
        ));
    }
    let order: HashMap<&str, usize> = cats
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key.as_str(), i))
        .collect();
    // stable sort keeps the list order inside a category
    cards.sort_by_key(|c| {
        *order
            .get(c.json["category"].as_str().unwrap_or(""))
            .unwrap_or(&99)
    });
    out.extend(cards);
    out
}

fn heat(cards: &[Card], today: NaiveDate) -> Value {
    let total = cards.len();
    let end = week_of(today)[6];
    let days: Vec<NaiveDate> = (0..HEAT_WEEKS * 7).rev().map(|i| add(end, -i)).collect();
    let mut counts: HashMap<NaiveDate, usize> = HashMap::new();
    for d in days.iter().filter(|d| **d <= today) {
        counts.insert(*d, cards.iter().filter(|c| c.done.contains(d)).count());
    }
    let perfect: HashSet<NaiveDate> = counts
        .iter()
        .filter(|(_, n)| total > 0 && **n == total)
        .map(|(d, _)| *d)
        .collect();
    let mut streak = 0;
    let mut d = if perfect.contains(&today) {
        today
    } else {
        add(today, -1)
    };
    while perfect.contains(&d) {
        streak += 1;
        d = add(d, -1);
    }
    let (mut best, mut run) = (0, 0);
    for d in &days {
        run = if perfect.contains(d) { run + 1 } else { 0 };
        best = best.max(run);
    }
    json!({"total": total, "perfect": perfect.len(), "streak": streak, "best": best,
           "days": days.iter().map(|d| json!({
               "d": fmt(*d, "%a %d %b"), "m": fmt(*d, "%b"),
               "first": d.day() <= 7 && d.weekday().num_days_from_monday() == 0,
               "n": counts.get(d).map(|n| json!(n)).unwrap_or(Value::Null), "today": *d == today,
           })).collect::<Vec<_>>()})
}

// MARK: summary

pub fn build(store: &Store, for_sharing: bool, now: DateTime<Local>) -> Value {
    let subjects = store.load_subjects();
    let cats = categories(&store.load_categories());
    let cat_keys: HashSet<String> = cats.iter().map(|c| c.key.clone()).collect();
    let rows = load_rows(store, &subjects, &cat_keys);
    let today = now.date_naive();
    let day_list: Vec<NaiveDate> = (0..DAYS).rev().map(|i| add(today, -i)).collect();

    let mut subject_order: Vec<String> = vec![];
    for r in &rows {
        if !subject_order.contains(&r.subject) {
            subject_order.push(r.subject.clone());
        }
    }
    let mut per_day: HashMap<NaiveDate, HashMap<String, f64>> = HashMap::new();
    let mut per_day_cat: HashMap<NaiveDate, HashMap<String, f64>> = HashMap::new();
    let mut per_subject: HashMap<String, (f64, usize, usize)> = HashMap::new();
    let mut subject_cat: HashMap<String, String> = HashMap::new();
    let mut by_start = rows.clone();
    by_start.sort_by_key(|r| r.start);
    for r in &by_start {
        subject_cat.insert(r.subject.clone(), r.category.clone());
    }
    for r in &rows {
        *per_day
            .entry(r.day())
            .or_default()
            .entry(r.subject.clone())
            .or_insert(0.0) += r.actual;
        *per_day_cat
            .entry(r.day())
            .or_default()
            .entry(r.category.clone())
            .or_insert(0.0) += r.actual;
        let s = per_subject.entry(r.subject.clone()).or_insert((0.0, 0, 0));
        s.0 += r.actual;
        if r.done {
            s.1 += 1
        } else if r.actual >= QUICK_MIN {
            s.2 += 1
        }
    }
    let real: Vec<&Row> = rows
        .iter()
        .filter(|r| r.done || r.actual >= QUICK_MIN)
        .collect();
    let studied: HashSet<NaiveDate> = real.iter().map(|r| r.day()).collect();
    let mut streak = 0;
    let mut d = if studied.contains(&today) {
        today
    } else {
        add(today, -1)
    };
    while studied.contains(&d) {
        streak += 1;
        d = add(d, -1);
    }
    // per_day values in insertion order would match Python exactly; a HashMap's order can differ, but
    // these sums only feed rounded totals, and pysum is order-insensitive to within the rounding.
    let day_total = |d: NaiveDate| {
        pysum(
            per_day
                .get(&d)
                .map(|m| m.values().copied().collect::<Vec<_>>())
                .unwrap_or_default(),
        )
    };
    let window = |a: i64, b: i64| r1(pysum((a..=b).map(|i| day_total(add(today, -i)))));
    let cat_window = |k: &str, a: i64, b: i64| {
        r1(pysum((a..=b).map(|i| {
            per_day_cat
                .get(&add(today, -i))
                .and_then(|m| m.get(k))
                .copied()
                .unwrap_or(0.0)
        })))
    };

    let cards = habits(&rows, &subjects, &cats, today);
    let mut subject_category: Map<String, Value> = Map::new();
    for s in &subjects {
        subject_category
            .entry(s.name.clone())
            .or_insert(json!(s.category()));
    }
    for (k, v) in &subject_cat {
        subject_category.insert(k.clone(), json!(v));
    }

    let mut by_subject = subject_order.clone();
    by_subject.sort_by(|a, b| {
        per_subject[b]
            .0
            .partial_cmp(&per_subject[a].0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut result = json!({
        "subjects": subject_order,
        "subjectCategory": subject_category,
        "habits": cards.iter().map(|c| c.json.clone()).collect::<Vec<_>>(),
        "heat": heat(&cards, today),
        "categories": cats.iter().map(|c| json!({
            "key": c.key, "label": c.label, "goal": num(c.goal), "color": c.color,
            "today": num(r1(per_day_cat.get(&today).and_then(|m| m.get(&c.key)).copied().unwrap_or(0.0))),
            "week": num(cat_window(&c.key, 0, 6)),
            "total": num(r1(pysum(rows.iter().filter(|r| r.category == c.key).map(|r| r.actual)))),
            "sessions": real.iter().filter(|r| r.category == c.key).count(),
        })).collect::<Vec<_>>(),
        "days": day_list.iter().map(|d| {
            let values: Map<String, Value> = subject_order.iter().map(|s| {
                (s.clone(), num(r1(per_day.get(d).and_then(|m| m.get(s)).copied().unwrap_or(0.0))))
            }).collect();
            let cats_map: Map<String, Value> = cats.iter().map(|c| {
                (c.key.clone(), num(r1(per_day_cat.get(d).and_then(|m| m.get(&c.key)).copied().unwrap_or(0.0))))
            }).collect();
            json!({"label": fmt(*d, "%d %b"), "weekday": fmt(*d, "%a"), "values": values, "cats": cats_map})
        }).collect::<Vec<_>>(),
        "bySubject": by_subject.iter().map(|s| {
            let p = per_subject[s];
            let mine: Vec<&&Row> = real.iter().filter(|r| &r.subject == s).collect();
            json!({"subject": s, "category": subject_cat.get(s).cloned().unwrap_or_else(|| "academic".into()),
                   "min": num(r1(p.0)), "done": p.1, "stopped": p.2,
                   "avg": num(if mine.is_empty() { 0.0 } else { r1(pysum(mine.iter().map(|r| r.actual)) / mine.len() as f64) }),
                   "last": mine.iter().map(|r| r.start).max().map(|t| t.format("%d %b").to_string()).unwrap_or_else(|| "-".into())})
        }).collect::<Vec<_>>(),
        "totals": {
            "minutes": num(r1(pysum(rows.iter().map(|r| r.actual)))),
            "today": num(r1(day_total(today))),
            "done": rows.iter().filter(|r| r.done).count(), "attempts": real.len(), "quick": rows.len() - real.len(),
            "week": num(window(0, 6)), "lastWeek": num(window(7, 13)), "streak": streak, "goal": num(daily_goal()),
        },
        "generated": now.format("%d %b %Y, %H:%M").to_string(),
    });
    let sessions: Vec<Value> = if for_sharing {
        vec![]
    } else {
        let mut sorted: Vec<&Row> = rows.iter().collect();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.start));
        sorted.iter().map(|r| {
            let d = r.day();
            let group = if d == today { "Today".to_string() } else if d == add(today, -1) { "Yesterday".to_string() } else { fmt(d, "%a %d %b") };
            json!({"subject": r.subject, "category": r.category, "detail": r.detail, "date": r.start.format("%d %b").to_string(),
                   "hour": r.start.hour(), "group": group, "start": r.start.format("%H:%M").to_string(),
                   "end": r.end.format("%H:%M").to_string(), "planned": num(r.planned), "actual": num(r.actual), "done": r.done})
        }).collect()
    };
    result["sessions"] = json!(sessions);
    if for_sharing {
        result["shared"] = json!(true);
    }
    result
}

pub fn json_bytes(store: &Store, for_sharing: bool) -> Vec<u8> {
    serde_json::to_vec(&build(store, for_sharing, Local::now())).unwrap_or_else(|_| b"{}".to_vec())
}

// report.html next to the data, from the same template as the web page
pub fn write_local(store: &Store) -> std::io::Result<std::path::PathBuf> {
    let data = String::from_utf8(json_bytes(store, false)).unwrap_or_else(|_| "{}".into());
    // "</" inside a string would end the <script> early; JSON allows the escaped form
    let html = TEMPLATE.replace("__DATA__", &data.replace("</", "<\\/"));
    let path = store.folder.join("report.html");
    crate::store::write_atomic(&path, html.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewSession;

    fn folder_with_data() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut pray = Subject::new("Pray", 5.0, "soul");
        pray.options = Some(vec!["Subuh".into(), "Zohor".into()]);
        store
            .save_subjects(&[
                Subject::new("SQL", 25.0, "academic"),
                pray,
                Subject::new("Tadarus", 20.0, "c-quran-club"),
            ])
            .unwrap();
        store
            .save_categories(&[CustomCategory {
                key: "c-quran-club".into(),
                label: "Quran Club".into(),
                icon: "star".into(),
                color: "#8e6be8".into(),
            }])
            .unwrap();
        let now = Local::now();
        let session =
            |subject: &str, category: &str, detail: Option<&str>, min: f64, ago: i64| NewSession {
                subject: subject.into(),
                category: category.into(),
                detail: detail.map(String::from),
                planned_min: min,
                actual_min: min,
                started_at: now - Duration::seconds(ago),
                ended_at: now - Duration::seconds(ago) + Duration::minutes(min as i64),
                completed: true,
            };
        // keep every session inside today, even just after midnight
        let since_midnight = (now
            - now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_local_timezone(Local)
                .unwrap())
        .num_seconds();
        let ago = |s: i64| s.min(since_midnight.max(0));
        store
            .append_session(&session("SQL", "academic", None, 25.0, ago(3000)))
            .unwrap();
        for p in ["Subuh", "Zohor"] {
            store
                .append_session(&session("Pray", "soul", Some(p), 5.0, ago(900)))
                .unwrap();
        }
        (dir, store)
    }

    #[test]
    fn report_has_habits_categories_and_totals() {
        let (_d, store) = folder_with_data();
        let d = build(&store, false, Local::now());
        let names: Vec<&str> = d["habits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Study", "Pray", "Tadarus"]);
        let today = d["habits"][1]["days"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["today"] == true)
            .unwrap();
        assert_eq!(today["s"], 2); // both prayers done
        let keys: Vec<&str> = d["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            ["academic", "health", "soul", "personal", "c-quran-club"]
        );
        assert_eq!(d["categories"][4]["color"], "#8e6be8");
        assert_eq!(d["totals"]["minutes"], 35.0);
        assert_eq!(d["sessions"].as_array().unwrap().len(), 3);
        assert_eq!(d["heat"]["days"].as_array().unwrap().len(), 182);
    }

    #[test]
    fn shared_report_leaves_out_sessions() {
        let (_d, store) = folder_with_data();
        let d = build(&store, true, Local::now());
        assert_eq!(d["sessions"].as_array().unwrap().len(), 0);
        assert_eq!(d["shared"], true);
        // only keys the share API accepts
        let allowed = [
            "subjects",
            "subjectCategory",
            "habits",
            "heat",
            "categories",
            "days",
            "bySubject",
            "totals",
            "generated",
            "sessions",
            "shared",
        ];
        for k in d.as_object().unwrap().keys() {
            assert!(allowed.contains(&k.as_str()), "unexpected key {k}");
        }
    }

    #[test]
    fn local_report_fills_the_template() {
        let (_d, store) = folder_with_data();
        let html = std::fs::read_to_string(write_local(&store).unwrap()).unwrap();
        assert!(!html.contains("__DATA__"));
        assert!(html.contains("\"Quran Club\""));
        assert!(TEMPLATE.contains("const D = __DATA__;"));
    }

    #[test]
    fn python_style_rounding_and_sums() {
        assert_eq!(r1(19.35), 19.4); // like Python round(19.35, 1)
        assert_eq!(r1(0.25), 0.2);
        assert_eq!(pysum([0.1, 0.2, 0.3]), 0.6); // a plain sum gives 0.6000000000000001
    }
}
