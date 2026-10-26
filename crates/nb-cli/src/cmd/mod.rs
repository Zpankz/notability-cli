//! Every command, grouped by noun.

pub mod account;
pub mod audio;
pub mod chat;
pub mod content;
pub mod files;
pub mod folders;
pub mod imports;
pub mod learn;
pub mod notes;
pub mod share;
pub mod study;

use crate::spec::Spec;

/// Defines a command handler: `fn name(Ctx) -> BoxFut` around an async body.
#[macro_export]
macro_rules! handler {
    ($name:ident, |$c:ident| $body:block) => {
        fn $name($c: $crate::spec::Ctx) -> $crate::spec::BoxFut {
            Box::pin(async move $body)
        }
    };
}

pub fn all() -> Vec<Spec> {
    let mut v = Vec::new();
    v.extend(notes::specs());
    v.extend(folders::specs());
    v.extend(content::specs());
    v.extend(files::specs());
    v.extend(imports::specs());
    v.extend(audio::specs());
    v.extend(share::specs());
    v.extend(learn::specs());
    v.extend(study::specs());
    v.extend(chat::specs());
    v.extend(account::specs());
    v
}

/// Group help text for nouns and sub-groups.
pub const GROUPS: [(&str, &str); 27] = [
    (
        "notes",
        "Find, open and manage notes in your library (incl. trash)",
    ),
    (
        "folders",
        "Organise the library: folders, subjects and dividers",
    ),
    (
        "content",
        "Read and edit what is inside a note: text, math, code, ink, shapes, comments, pages",
    ),
    (
        "files",
        "Import and export: PDF, images, audio, .ntb, markdown, text, JSON, transcripts; page previews",
    ),
    (
        "files import",
        "Bring files into the library: PDF, Office, web links, Google Drive, images, audio, .ntb, YouTube",
    ),
    ("audio", "Recordings and transcripts"),
    (
        "audio record",
        "Record audio on this machine with ffmpeg and attach it to a note",
    ),
    (
        "share",
        "Who can see a note: links, collaborators, access requests",
    ),
    (
        "learn",
        "Study tools: summaries, quizzes, flashcards, TOC quizzes, finish notes, study sessions, Brain Break, syllabus",
    ),
    ("chat", "Notability AI chat threads about your notes"),
    (
        "account",
        "Sign-in session, profile, quotas, settings, calendar, raw GraphQL",
    ),
    ("notes trash", "Deleted notes: list, restore, purge, empty"),
    ("folders organizers", "Legacy subjects and dividers"),
    (
        "content text",
        "Typed text and text boxes: styles, paragraphs, checklists, code blocks",
    ),
    ("content math", "Math blocks (LaTeX)"),
    ("content comments", "Page comments and replies"),
    (
        "content pages",
        "Pages: list, add, delete, reorder, paper templates",
    ),
    (
        "content ink",
        "Handwriting: pen and highlighter strokes, eraser",
    ),
    (
        "content shapes",
        "Shapes: rectangle, ellipse, triangle, arrow, line",
    ),
    ("content images", "Images placed on pages"),
    ("learn quiz", "Quiz sessions: list, start, answer, finish"),
    ("learn cards", "Flashcards: due, rate, import"),
    (
        "learn toc",
        "Quizzes from chosen sections of a PDF's table of contents",
    ),
    (
        "learn session",
        "Guided study session: memorize, practice and test stages, graded answers",
    ),
    (
        "learn discuss",
        "Brain Break: an AI discussion about the note, turn by turn",
    ),
    (
        "audio live",
        "Live transcription: stream audio and get the transcript and smart notes",
    ),
    ("account settings", "Account settings and toggles"),
];
