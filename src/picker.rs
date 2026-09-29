use anyhow::{Context, Result};
use dialoguer::{FuzzySelect, Input, theme::ColorfulTheme};
use indicatif::HumanBytes;
use std::path::{Path, PathBuf};

const SEARCH_MAX_DEPTH: usize = 6;
const SEARCH_MAX_HITS: usize = 200;

enum Row {
    Up,
    Search,
    Enter(PathBuf),
    Pick(PathBuf),
}

pub fn pick_file(start: &Path) -> Result<PathBuf> {
    let mut current = start
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."));

    loop {
        let (mut rows, mut labels) = (Vec::new(), Vec::new());

        if current.parent().is_some() {
            rows.push(Row::Up);
            labels.push("..".to_string());
        }
        rows.push(Row::Search);
        labels.push("[ search this folder and below ]".to_string());

        for (row, label) in read_dir_sorted(&current) {
            rows.push(row);
            labels.push(label);
        }

        let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
            .with_prompt(current.display().to_string())
            .default(0)
            .items(&labels)
            .interact()?;

        match &rows[selection] {
            Row::Up => {
                current.pop();
            }
            Row::Enter(path) => current = path.clone(),
            Row::Pick(path) => return Ok(path.clone()),
            Row::Search => {
                if let Some(path) = search(&current)? {
                    return Ok(path);
                }
            }
        }
    }
}

fn read_dir_sorted(dir: &Path) -> Vec<(Row, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };

        if meta.is_dir() {
            dirs.push((name.clone(), Row::Enter(entry.path()), format!("{name}/")));
        } else if meta.is_file() {
            files.push((
                name.clone(),
                Row::Pick(entry.path()),
                format!("{name}   {}", HumanBytes(meta.len())),
            ));
        }
    }

    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    dirs.into_iter()
        .chain(files)
        .map(|(_, row, label)| (row, label))
        .collect()
}

fn search(root: &Path) -> Result<Option<PathBuf>> {
    let term: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Search for")
        .interact_text()?;
    let needle = term.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(None);
    }

    let mut hits = Vec::new();
    walk(root, 0, &needle, &mut hits);

    if hits.is_empty() {
        println!("No match for {term:?} under {}", root.display());
        return Ok(None);
    }

    let labels: Vec<String> = hits
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();

    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("{} match(es)", hits.len()))
        .default(0)
        .items(&labels)
        .interact()?;

    Ok(Some(hits[selection].clone()))
}

fn walk(dir: &Path, depth: usize, needle: &str, hits: &mut Vec<PathBuf>) {
    if depth > SEARCH_MAX_DEPTH || hits.len() >= SEARCH_MAX_HITS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };

        if meta.is_dir() {
            walk(&entry.path(), depth + 1, needle, hits);
        } else if meta.is_file() && name.to_lowercase().contains(needle) {
            hits.push(entry.path());
        }
        if hits.len() >= SEARCH_MAX_HITS {
            return;
        }
    }
}

pub fn resolve_path(input: &str) -> Result<PathBuf> {
    let expanded = match input.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join(rest),
        None => PathBuf::from(input),
    };

    expanded
        .canonicalize()
        .with_context(|| format!("no such file: {input}"))
}
