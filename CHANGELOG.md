# Changelog

## 0.3.0

### Added

* `learn toc list` and `learn toc quiz`: quizzes and flashcards built per section of a PDF's
  table of contents.
* `learn finish` ("Finish my notes"): an AI write-up of a note's typed text, PDFs and transcribed
  recordings, inserted into the note (`--yes`; `--no-insert` to only print it).
* `learn session start|next|answer|end` ("Start study session"): guided memorize, practice and
  master stages over a note's study materials.
* `learn discuss start|reply|end`: Brain Break discussions about a note.
* `learn syllabus`: import a syllabus PDF into a course folder with its dates read out.
* `audio live start|finalize`: live transcription from the microphone or an audio file, with
  `--attach` to keep the audio as a recording.
* `files import office|url|drive`: Office documents (via LibreOffice), PDFs by link and Google
  Drive files.
* `share search-users`: search people you can invite (business accounts).
* `account survey-set|survey-remove|use-type`: profile writes, behind `--yes`.
* Skills: a root `notability` skill and one skill per noun, generated from the command
  definitions and committed in `skills/`; `notability skills add` installs them.

### Changed

* Confirmation prompts say a command changes your data rather than calling every write
  destructive.
* MCP: 43 read-only tools by default, 115 with `--allow-writes`, 48 with `--allow-ai`, 125 with
  both.
