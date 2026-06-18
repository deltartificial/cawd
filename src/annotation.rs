//! Code review annotations: data model, persistence, and status tracking.
//!
//! Annotations are stored as individual markdown files under `<root>/.cawd/`.
//! Each file carries a parseable header (id, status, file, line range, worker
//! pid) followed by the selected code excerpt and the user's comment.

use std::path::{Path, PathBuf};

/// The lifecycle status of an annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnnotationStatus {
    /// Not yet picked up.
    #[default]
    Open,
    /// A worker is currently addressing it.
    InProgress,
    /// Addressed and closed.
    Resolved,
}

impl AnnotationStatus {
    /// Parses a status from its serialized lowercase form.
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "in_progress" => Self::InProgress,
            "resolved" => Self::Resolved,
            _ => Self::Open,
        }
    }

    /// Returns the serialized lowercase form.
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Resolved => "resolved",
        }
    }

    /// Returns the badge glyph shown in the list.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Open => "○",
            Self::InProgress => "◐",
            Self::Resolved => "●",
        }
    }

    /// Cycles to the next status (open → in_progress → resolved → open).
    pub fn next(self) -> Self {
        match self {
            Self::Open => Self::InProgress,
            Self::InProgress => Self::Resolved,
            Self::Resolved => Self::Open,
        }
    }
}

/// A single code review annotation.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// Stable identifier, equal to the file stem (a timestamp).
    pub id: String,
    /// Current lifecycle status.
    pub status: AnnotationStatus,
    /// Project-relative path of the annotated file.
    pub file: String,
    /// Human-readable line range label, e.g. `42-45`.
    pub lines: String,
    /// First annotated line (1-based), used to scroll the viewer.
    pub start_line: usize,
    /// Creation date string.
    pub date: String,
    /// PID of a running worker, if any.
    pub worker_pid: Option<u32>,
    /// The annotated code excerpt (with line-number prefixes).
    pub excerpt: String,
    /// The user's comment.
    pub comment: String,
    /// Absolute path to the backing `.md` file.
    pub path: PathBuf,
}

impl Annotation {
    /// Returns the directory where annotations live for a given project root.
    pub fn dir(root: &Path) -> PathBuf {
        root.join(".cawd")
    }

    /// Returns the 1-based inclusive line range `(start, end)` this annotation
    /// covers, parsed from the `lines` label (e.g. `42-45` or a single `42`).
    pub fn line_range(&self) -> (usize, usize) {
        let end = self
            .lines
            .rsplit('-')
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(self.start_line);
        (self.start_line, end.max(self.start_line))
    }

    /// Serializes the annotation back to its markdown representation.
    pub fn to_markdown(&self) -> String {
        format!(
            "id: {}\nstatus: {}\nfile: {}\nlines: {}\ndate: {}\nworker: {}\n---\n{}\n---\ncomment:\n{}\n",
            self.id,
            self.status.as_str(),
            self.file,
            self.lines,
            self.date,
            self.worker_pid.map_or_else(|| "-".to_string(), |p| p.to_string()),
            self.excerpt,
            self.comment,
        )
    }

    /// Writes the annotation to its backing file.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, self.to_markdown())
    }

    /// Deletes the backing file.
    pub fn delete(&self) -> std::io::Result<()> {
        std::fs::remove_file(&self.path)
    }

    /// Parses an annotation from a file path and its contents.
    ///
    /// Returns `None` when the required header fields are missing.
    fn parse(path: PathBuf, content: &str) -> Option<Self> {
        let mut id = None;
        let mut status = AnnotationStatus::Open;
        let mut file = None;
        let mut lines = None;
        let mut date = String::new();
        let mut worker_pid = None;

        let mut sections = content.splitn(3, "\n---\n");
        let header = sections.next().unwrap_or_default();
        let excerpt = sections.next().unwrap_or_default().to_string();
        let rest = sections.next().unwrap_or_default();

        for line in header.lines() {
            let Some((key, value)) = line.split_once(':') else { continue };
            let value = value.trim();
            match key.trim() {
                "id" => id = Some(value.to_string()),
                "status" => status = AnnotationStatus::from_str(value),
                "file" => file = Some(value.to_string()),
                "lines" => lines = Some(value.to_string()),
                "date" => date = value.to_string(),
                "worker" => worker_pid = value.parse::<u32>().ok(),
                _ => {}
            }
        }

        let lines = lines?;
        let start_line = lines
            .split('-')
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1);

        // The comment follows a leading `comment:` marker line.
        let comment = rest
            .strip_prefix("comment:\n")
            .or_else(|| rest.strip_prefix("comment:"))
            .unwrap_or(rest)
            .trim_matches('\n')
            .to_string();

        let fallback_id = path.file_stem().map(|s| s.to_string_lossy().into_owned());

        Some(Self {
            id: id.or(fallback_id)?,
            status,
            file: file?,
            lines,
            start_line,
            date,
            worker_pid,
            excerpt: excerpt.trim_matches('\n').to_string(),
            comment,
            path,
        })
    }

    /// Loads all annotations from the `.cawd/` directory under `root`.
    ///
    /// Results are sorted by status (open first) then by id.
    pub fn load_all(root: &Path) -> Vec<Self> {
        let dir = Self::dir(root);
        let mut annotations = Vec::new();

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return annotations;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(annotation) = Self::parse(path, &content) {
                    annotations.push(annotation);
                }
            }
        }

        annotations.sort_by(|a, b| {
            let order = |s: AnnotationStatus| match s {
                AnnotationStatus::Open => 0,
                AnnotationStatus::InProgress => 1,
                AnnotationStatus::Resolved => 2,
            };
            order(a.status)
                .cmp(&order(b.status))
                .then_with(|| a.id.cmp(&b.id))
        });

        annotations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Annotation {
        Annotation {
            id: "2026-06-18T14-30-00".to_string(),
            status: AnnotationStatus::InProgress,
            file: "src/app.rs".to_string(),
            lines: "42-45".to_string(),
            start_line: 42,
            date: "2026-06-18 14:30:00".to_string(),
            worker_pid: Some(12345),
            excerpt: "  42 | let foo = bar();\n  43 | let baz = qux();".to_string(),
            comment: "needs a refactor\nsecond line".to_string(),
            path: PathBuf::from("/tmp/.cawd/2026-06-18T14-30-00.md"),
        }
    }

    #[test]
    fn round_trips_through_markdown() {
        let original = sample();
        let markdown = original.to_markdown();
        let parsed = Annotation::parse(original.path.clone(), &markdown).expect("parse");

        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.status, original.status);
        assert_eq!(parsed.file, original.file);
        assert_eq!(parsed.lines, original.lines);
        assert_eq!(parsed.start_line, 42);
        assert_eq!(parsed.date, original.date);
        assert_eq!(parsed.worker_pid, Some(12345));
        assert_eq!(parsed.excerpt, original.excerpt);
        assert_eq!(parsed.comment, original.comment);
    }

    #[test]
    fn parses_dash_worker_as_none() {
        let mut annotation = sample();
        annotation.worker_pid = None;
        let markdown = annotation.to_markdown();
        assert!(markdown.contains("worker: -"));
        let parsed = Annotation::parse(annotation.path.clone(), &markdown).expect("parse");
        assert_eq!(parsed.worker_pid, None);
    }

    #[test]
    fn missing_required_fields_returns_none() {
        let parsed = Annotation::parse(PathBuf::from("/tmp/x.md"), "status: open\n");
        assert!(parsed.is_none());
    }
}
