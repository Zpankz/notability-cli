# Command status

131 commands. 74 have been verified end to end against the live web API (for `files import drive`
Google's side was a local mock), 57 have not been run live (they are covered by unit and contract
tests only).

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
| `account survey-remove` | not run live (would remove profile answers) |
| `account survey-set` | verified (by writing an answer's current value and reading it back) |
| `account use-type` | verified (by writing the current value) |
| `audio delete` | verified |
| `audio list` | verified |
| `audio live finalize` | not run live on its own (`audio live start` finalizes by default) |
| `audio live start` | verified (`--input-file --realtime --attach` with a synthetic clip; microphone input not run live) |
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
| `files import drive` | verified against a local mock of the Drive API; not run with a real Google token |
| `files import image` | verified |
| `files import media` | verified |
| `files import ntb` | verified |
| `files import office` | verified (.docx) |
| `files import pdf` | verified |
| `files import url` | verified |
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
| `learn discuss end` | verified (local state only) |
| `learn discuss reply` | not run live (uses AI quota) |
| `learn discuss start` | not run live (uses AI quota) |
| `learn explain` | not run live (uses AI quota) |
| `learn finish` | verified |
| `learn generate` | verified |
| `learn materials` | verified |
| `learn quiz answer` | verified |
| `learn quiz finish` | verified |
| `learn quiz list` | verified |
| `learn quiz start` | verified |
| `learn session answer` | verified |
| `learn session end` | verified |
| `learn session next` | verified |
| `learn session start` | verified |
| `learn summary` | verified |
| `learn syllabus` | verified |
| `learn toc list` | verified |
| `learn toc quiz` | verified |
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
| `share search-users` | run live: returns `UNSUPPORTED` on accounts without a Notability business |
| `share status` | verified |
| `share user` | not run live |

## Known limits of the newer commands

* `learn discuss start|reply` (Brain Break) follow the web app's requests but have not been run
  live, to save AI quota.
* `files import drive` needs a Google OAuth access token with the `drive.readonly` (or `drive.file`)
  scope in `GOOGLE_DRIVE_TOKEN` (or the variable named by `--token-env`). Notability's own Drive
  picker token is not available outside the web app.
* `share search-users` only works for members of a Notability business; personal accounts get
  `UNSUPPORTED` (exit 8).
* `account survey-set|survey-remove|use-type` change your profile. Only same-value writes were run
  live.
* `audio live start` reads the microphone with ffmpeg on this machine; it is hidden over the HTTP
  MCP transport.
* `learn toc quiz` and `learn finish` need enough source text; very short sections fail with
  `INPUT_TOO_SHORT`.
