# Command status

111 commands. 59 have been verified end to end against the live web API, 52 have not
been run live (they are covered by unit and contract tests only).

"Verified" means the release binary ran the command against a real account, using disposable test
notes and folders for anything that writes, and the result was checked in the Notability web app.
`learn wait` was exercised through `learn generate --wait`.

Because the API is private and undocumented, a verified command can still break when Notability
changes its backend.

| Command | Status |
|---|---|
| `account calendar` | verified |
| `account flags` | verified |
| `account graphql` | not run live |
| `account login` | not run live (interactive sign-in) |
| `account logout` | not run live (ends the session) |
| `account me` | verified |
| `account quotas` | verified |
| `account refresh` | not run live |
| `account settings get` | verified |
| `account settings name` | not run live (changes the profile name) |
| `account settings set` | not run live |
| `account settings toggle` | not run live |
| `account status` | verified |
| `account stickers` | verified |
| `account survey` | verified |
| `audio delete` | verified |
| `audio list` | verified |
| `audio record start` | verified |
| `audio record status` | verified |
| `audio record stop` | verified |
| `audio transcript` | verified |
| `audio upload` | not run live |
| `chat ask` | not run live (uses AI quota) |
| `chat backfill` | not run live |
| `chat context` | not run live |
| `chat delete` | not run live |
| `chat read` | not run live |
| `chat threads` | verified |
| `content comments add` | not run live |
| `content comments delete` | not run live |
| `content comments edit` | not run live |
| `content comments list` | verified |
| `content comments move` | not run live |
| `content images crop` | verified |
| `content images flip` | verified |
| `content images list` | verified |
| `content ink draw` | not run live |
| `content ink erase` | not run live |
| `content ink list` | verified |
| `content math add` | not run live |
| `content math edit` | verified |
| `content math list` | verified |
| `content move` | not run live |
| `content pages add` | not run live |
| `content pages delete` | not run live |
| `content pages list` | verified |
| `content pages reorder` | not run live |
| `content pages template` | not run live |
| `content pages templates` | verified |
| `content shapes add` | not run live |
| `content shapes list` | verified |
| `content show` | verified |
| `content text add` | verified |
| `content text check` | not run live |
| `content text list` | verified |
| `content text paragraph` | not run live |
| `content text style` | not run live |
| `files export` | verified |
| `files import image` | verified |
| `files import media` | verified |
| `files import ntb` | verified |
| `files import pdf` | verified |
| `files import youtube` | verified |
| `files render` | verified |
| `files toc` | verified |
| `folders create` | not run live |
| `folders delete` | not run live |
| `folders edit` | not run live |
| `folders list` | verified |
| `folders move` | not run live |
| `folders order` | verified |
| `folders organizers create` | not run live |
| `folders organizers delete` | not run live |
| `folders organizers edit` | not run live |
| `folders organizers list` | verified |
| `folders rename` | not run live |
| `folders reorder` | not run live |
| `learn cards due` | verified |
| `learn cards import` | verified |
| `learn cards rate` | verified |
| `learn explain` | not run live (uses AI quota) |
| `learn generate` | verified |
| `learn materials` | verified |
| `learn quiz answer` | verified |
| `learn quiz finish` | verified |
| `learn quiz list` | verified |
| `learn quiz start` | verified |
| `learn summary` | verified |
| `learn wait` | verified |
| `notes create` | verified |
| `notes delete` | verified |
| `notes duplicate` | not run live |
| `notes favorite` | not run live |
| `notes get` | verified |
| `notes index` | verified |
| `notes layout` | not run live |
| `notes list` | verified |
| `notes move` | not run live |
| `notes rename` | not run live |
| `notes search` | verified |
| `notes trash empty` | not run live (irreversible) |
| `notes trash list` | verified |
| `notes trash purge` | not run live |
| `notes trash restore` | not run live |
| `share answer` | not run live (needs a pending access request) |
| `share collaborators` | verified |
| `share invite` | not run live (sends an email invitation) |
| `share link` | not run live |
| `share revoke` | not run live |
| `share status` | verified |
| `share user` | not run live |

## Not implemented

* Table-of-contents quiz generation (`learn toc`, `learn generate --toc`). `files toc` reads the
  outline, but no quiz is built from it.
* Live transcription. Recordings can still be transcribed after upload (`audio record stop --transcribe`).
* Office, Google Drive and PDF-URL import; invitable-user search; survey and college writes;
  brain-break discussion; syllabus import.
