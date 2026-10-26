# Notability web API: protocol notes

Mapped from the web app (`notability.com/app`, build `web-26.43.7`) by observing its network
traffic (HTTP and WebSocket frames) and reading its JavaScript bundles and SharedWorker. Nothing
here is official or documented by Notability; it can change with any web release. The client sends
the same client headers as the web app.

```
                    ┌─────────────── notability.com ───────────────┐
 notability CLI ──► │ POST /graphql                (account, Learn, AI jobs, chat list,
 / MCP server       │                               OAuth tokens, quotas)          │
                    │ wss /collab-api/socket.io/   ns /metadata: library sync +    │
                    │                               all library mutations (CRDT-ish)│
                    │ POST /collab-api/note/{id}/bundle   note op-log (FlatBuffers)│
                    │ GET  /media/{sha512}?noteId=  → signed asset URL (PDF/img/audio)
                    │ GET  /images/thumbnail/{id}   page-1 thumbnail               │
                    │ POST /learn/chatbotThread/{id}  AI chat (read / SSE stream)  │
                    └──────────────────────────────────────────────────────────────┘
                       Google Calendar / Microsoft Graph (via short-lived OAuth token
                       minted by GraphQL requestOAuthAccessToken)
```

## Authentication

* A single session JWT, `nb-auth-token` (HS256, claims `{userId, version, iat}`, **no `exp`**).
  The browser stores it as an HttpOnly cookie on `notability.com`.
* Every request sends it both as the `nb-auth-token` cookie **and** an `nb-auth-token` header.
* Client headers sent by the web app (and by this client):
  `client-platform: WEB`, `client-version: 26.43.7`, `client-git-hash`, `client-build-time`.
* Invalid sessions: HTTP 401/403, or GraphQL errors with `forceLogOut: true`.
* `notability account login` reads the cookie from a signed-in browser over CDP using only
  `Storage.getCookies` (no page scripting), and stores it in
  `$XDG_CONFIG_HOME/notability-cli/credentials.json` (mode 600). `NOTABILITY_TOKEN` overrides it.

## GraphQL: `POST /graphql`

* Introspection is disabled. The operation documents come from the app bundle.
* Error shape: `{"errors":[{"message", "key", "forceLogOut"}], "data": null}`. `key` values like
  `unauthenticated`, `aiQuotaExceeded` and `notFound.*` are mapped to CLI exit codes.

| Area | Operations used by this client |
|---|---|
| Account | `getMe`, `getSubscriptionInfoForUser`, `getConnectedOAuthAccounts` |
| Learn | `getStudyMaterialsBatch` (per note: transcripts, summaries, quizzes, flashcards), `getASRJob` (transcript by audio sha512), `getSummaryJobContent`, `getQuizJobContent`, `getQuizSessions`, `flashcardDueCounts` |
| Chat | `getChatbotThreads`, `deleteChatbotThread` |
| Calendar | `requestOAuthAccessToken {connectedAccountId, requiredScopes}` |

Deliberately **not** used (destructive, sharing, or billing): `deleteAccount`, `cancelTrial`,
`deleteUserCollabLibrary`, `grantNoteAccess`, `addNotePermissionsByEmail`, `generateCollabLinkToken`,
permission edits, and all `*Business*` operations. `notability account graphql` refuses raw mutations.

## Library sync: Socket.IO v4, namespace `/metadata`

`wss://notability.com/collab-api/socket.io/?EIO=4&transport=websocket`

```
→ 40/metadata,{"token":"<jwt>","query":{<client headers>, "sequenceId"?}}
← 40/metadata,{"sid":...}
← 42/metadata,["partial-update",{<Library>},"<cursor>"]     full library on connect
← 2  /  → 3                                                  engine.io ping/pong
→ 42/metadata,["change",{"mutations":[<Mutation>...]}]
← 42/metadata,["change-ack",{"successfulMutations":[key], "failedMutations":[{idempotencyKey,error}]}]
   other events: "error", "change" (pushes from other devices), "reset-sequence-id"
```

`Library` = `{sequenceId, privateSpace:{id, folders[], organizers[], notes[]}, sharedSpaces[],
sharedNotes[], permanentlyDeletedNotes[]}`. Notes carry `id, title, folderId, createdAt,
updatedAt, lastOpened, isFavorited, hasRecordings, indexTimestamps[{type: HANDWRITING|TEXT|PDF|RECORDING|IMAGE}], thumbnailUrl`.
Times are epoch ms.

A mutation is `{idempotencyKey: uuid4, timestamp: ms, <kind>: payload}`:

| kind | payload | verified live |
|---|---|---|
| `createFolder` | `{id, hexColor, isLocked, title, siblingOrder, workspaceId, parentId}` | yes |
| `modifyFolder` | `{folderId, title}` | yes |
| `deleteFolder` | `{folderId, childrenHash?}`: SHA-1 of the sorted descendant folder+note ids joined by a space; omitted if empty | empty folder only |
| `createNote` | `{id, deviceId, workspaceId, title, createdAt, linkAccessLevel:"VIEWER", linkPermissionScope:"RESTRICTED"}` | yes |
| `modifyNote` | `{noteId, moveToFolder:{parentId, workspaceId}}` or `{noteId, isFavorited}` | yes |
| `deleteNote` / `undeleteNote` / `permanentlyDeleteNote` | `{noteId}` | delete + permanent: yes |

A note title is not a library field you can change with a mutation. It lives in the note's own edit
log as a `SetMetadata.title` op (the `title` on `createNote` only seeds the library entry). Renaming
is an edit-log append; see [Edit log write protocol](#edit-log-write-protocol). The server then
updates the library title by itself, as confirmed with `notes get`.

## Note content: `POST /collab-api/note/{id}/bundle?deviceId=<uuid>`

The body is empty and the response is a FlatBuffers `NoteBundle`: the complete op log of the note.
The schema was recovered from generated FlatBuffers classes in the SharedWorker. Typed text is an RGA sequence: each character id is `(site, timestamp, index)`
and is inserted after its `location` anchor, with concurrent siblings ordered newest first.
`RemoveChar(s)` and `ReviveChars` tombstone characters. Text boxes are separate `textField`s.
Handwriting is vector ink (`CreateInk`/`AddPathElements`) and is **not** converted to text.
Handwriting recognition is uploaded by native clients to a server index that the web API cannot query.

`AssetCloudPersisted.assetHash` (SHA-512) identifies PDFs, images and audio:
`GET /media/{sha512hex}?noteId=` returns a short-lived signed URL.

## Edit log write protocol

Everything inside a note (title, typed text, text boxes, comments, pages, paper) is a FlatBuffers op
appended to the note's edit log. The CLI writes these ops directly with no browser. The op shapes match what
the web editor sends, and each was checked in the editor UI and in the web app's note model.

**Read:** `POST /collab-api/note/{id}/bundle?deviceId=<this device>` returns a `NoteBundle`. Its
`editorSite` is the u16 site id that the server has assigned to *this device for this note*. A
browser and the CLI get different sites, so their op ids never collide.

**Write:** `PUT /collab-api/note/{id}/append?siteId={editorSite}` with
`Content-Type: application/octet-stream` and body `AppendOpsEvent{ops:[Op]}`, where
`Op = {id:{site, timestamp}, clientTime: ms, payload:{type, value}}`. `timestamp` is a Lamport clock:
max timestamp in the bundle + 1, then +1 for each op. The response is
`NoteMutationResponse{noteId, acks:[OpAck{id, timestampedOp, nakError, duplicate, retryable}]}`.
There is one ack per op, and any `nakError` is treated as a rejection (exit code 9). Open editors
receive appended ops live.

Ids: entities are `Id{site,timestamp}` (the op that created them). Sequence elements (characters,
pages) are `SeqId{site,timestamp,index}`, where `index` is the position inside a multi-element op.

| Operation | Op(s) | CLI | Verified |
|---|---|---|---|
| Rename | `SetMetadata{title:{value}}` (LWW by Lamport ts, then site) | `notes rename` | UI header + library |
| Layout | `SetMetadata{layoutMode: 0 paged, 1 pageless}` | `notes layout` | UI |
| Body text | `InsertString{string, location?: SeqId of the preceding char}` (no location = start) | `content text add` | UI |
| Text box | `CreateBlock` (frame on a page) + `InsertString{textField: block id}` | `content text add --box` | model + DOM (same rendering as a UI-made box) |
| Comment | `CreateComment{text, anchor: CanvasAnchor{page: SeqId, origin:{x,y}}}` | `content comments add` | UI thread |
| Reply | `CreateComment{text, anchor: ReplyAnchor{root: Id}}` | `content comments add --reply-to` | UI thread |
| Delete comment | `DeleteEntities{entityDeletes:[Id]}` (a root removes the thread) | `content comments delete` | model |
| Add pages | `CreatePage{pageCount, location?: node SeqId, background?}` | `content pages add` | UI counter + model |
| Reorder page | `ModifyPage{pages:[page SeqId], moveTo:{toId?: node SeqId}}` (empty = first) | `content pages reorder` | model order |
| Delete page | `DeleteEntities{entityDeletes:[entities on page], pageDeletes:[SeqId]}` | `content pages delete` | UI counter + model |
| Note template | `SetMetadata{pageBackground:{value: PageBackground{paper}}}` | `content pages template` | UI (ruled lines) |
| Page template | `ModifyPage{pages:[SeqId], background:{value}}` | `content pages template --page` | model (`pages()[i].paper()`) |
| Duplicate | `createNote` mutation, then append the source log rebased to site 0 + a title op | `notes duplicate` | UI |
| New note | `createNote` + `CreatePage{pageCount:2}` @0:0 + `SetMetadata{title, pageBackground, fonts, layoutMode:1}` @0:1 | `notes create` | UI |

**Page order** is an RGA over *nodes*. `CreatePage` creates nodes `(site, ts, i)` with page ids
equal to the node ids. Each `ModifyPage.moveTo` creates a new node `(opSite, opTs, i)` for the page,
anchored after `toId`. Both `CreatePage.location` and `moveTo.toId` name a **node** (a page's current
position), not a page id. Siblings sort newest first, as for text.

**Paper:** `Paper{flair: 0 Lines | 1 Dots | 2 Grid (absent = plain), flairSpacing, backgroundColor
{bitsR,G,B,A}, legacyPaperIndex}`. The web app has no named template presets beyond these.

**A new note must be seeded.** A note created only with `createNote` has no edit log. Its bundle
returns HTTP 424, and the web editor shows it as empty until it writes the two initial ops itself.
The CLI writes them.

**Client-side only (no server call to map):**
* Undo and redo replay inverse ops locally and append them as ordinary ops.
* Version history: no server request was observed. History is implicit in the op log, which
  `content show` and the bundle expose.

**Covered elsewhere:**
* Favourite and move are `modifyNote` mutations.
* Export is `files export`.
* Media, PDF import and audio recording: see "Uploads, recording and export" below.

## AI chat: `POST /learn/chatbotThread/{threadId}`

* With no body, it returns the thread: `{messages[], nextExchangeIndex, ...}`.
* With a JSON body, it streams `text/event-stream`:
  `{exchangeIndex, role:"user", text, clientContext:{entryPoint:{location:"home"}|{location:"note",noteId}, clientInfo:{platform:"web", availableTools:[]}}}`.
  A new thread uses a fresh client-generated uuid.
* SSE events: `text` (JSON-string delta), `tool_call`, `tool_progress`, `error`,
  `exchange_complete {nextExchangeIndex, title}`.
* Server-side tools (`server_search-notes`, `server_recent-notes`, `server_recent-quizzes`) run
  without client help, so the assistant can search the user's notes, including handwriting,
  server-side. Client tools (`note_read`, `note_open`, ...) are only offered when listed in
  `availableTools`. This client offers none.

## Calendar ("Coming up")

1. Call `requestOAuthAccessToken` with the connected account id and a read-only scope.
2. Call Google Calendar `GET /calendar/v3/calendars/primary/events?timeMin&timeMax&singleEvents=true&orderBy=startTime`
   (Microsoft Graph `/me/calendarView` for Microsoft accounts).

## Rate limiting and retries (client policy)

* At least 250 ms between requests.
* Retries on 429/5xx/transport errors with exponential backoff, honouring `Retry-After`.
* One Socket.IO connection per library read or mutation batch. The MCP server caches the
  library for 60 s.

## Library management, sharing, Learn and settings

### Metadata `change` kinds (Socket.IO `/metadata`)

| kind | payload |
|---|---|
| `deleteNote` / `undeleteNote` / `permanentlyDeleteNote` | `{noteId}` |
| `modifyNote` | `{noteId}` plus one of `isFavorited`, `moveToFolder`, `linkAccessLevel` (`VIEWER`/`EDITOR`), `linkPermissionScope` (`RESTRICTED`/`PUBLIC`), `permissions: [{userId, accessLevel?}]` (omit `accessLevel` to revoke) |
| `createFolder` | `{id, title, hexColor, isLocked, siblingOrder, workspaceId, parentId}` |
| `modifyFolder` | `{folderId}` plus any of `title`, `hexColor`, `isLocked`, `emoji` (`null` clears it), `siblingOrder`, `moveToParent: {parentId, workspaceId}` |
| `deleteFolder` | `{folderId, childrenHash}`. `childrenHash` is the SHA-1 hex digest of the subtree's folder and note ids (trashed notes included), sorted and joined with a single space. The client must then send `deleteNote` for every live note in the subtree. |

Folders appear in ascending `siblingOrder`. Reordering sets the midpoint between the two neighbours, or the end value ±1 for the first and last positions. Sorting notes happens on the client only (`libraryPersisted.notesSortMode`).

### Sharing (GraphQL)
- `getAllPermissionedUsersForNote` returns `permissionedUsers {userId email name accessLevel}` and `permissionRequests {requestId userId email name accessLevel state}`.
- `addNotePermissionsByEmail` invites by email, and the invitee is sent an email.
- `grantNoteAccess` and `denyNoteAccess` answer an access request.

### Learn (GraphQL)
- `generateStudyMaterialsBatch` / `addToStudyMaterialsBatch` start generation. The server needs a few hundred words of text, otherwise it fails with `INPUT_TOO_SHORT`.
- `getStudyMaterialsBatch` is the endpoint to poll for status.
- `syncQuizSession {quizTypeParams{noteId, mode}, quizSessionUpdateParams{quizSessionId, quizQuestionCompletions, isCompleted}}` is a single endpoint that starts, resumes, answers and finishes a session. Flashcard ratings go in `userResponseFlashcard` (`AGAIN|HARD|MEDIUM|EASY`) together with `reviewDate`.
- `getQuizQuestionExplanation` explains an answer, and `uploadUserGeneratedQuiz` imports flashcards.

### Settings
- `setAppSetting {key, value}` stores JSON-encoded string values. The keys seen are `completedOnboarding`, `confirmedLayoutSwitch`, `firstWebAppVisit`, `learnEnabled`, `mixAndMatchNudgeDismissed`, `palmDetection`, `roundedCorners`, `seenCommentsCueCard`, `seenFeatureAnnouncementIds`, `seenKeepAddingTextCard`, `seenStudentPdfSharePrompt`, `seenStudentReopenSharePrompt`, `sharePromptLastShownDate` and `signupReported`.
- `updateLearnSettings` and `updateFullName` are the other settings mutations.

### Microsoft calendar (documented only)
Connecting a calendar is an interactive OAuth consent: the web app redirects to Microsoft and comes back on `/oauth/microsoft/callback`, and desktop hands off through `/calendar-handoff`. Once connected, events come through the same "Coming up" read that `notability account calendar` already uses. The CLI does not implement the consent flow, because it needs a browser and the user's Microsoft sign-in.

## Uploads, recording and export

Every write ends in the same primitive: append ops to the note's op log.

### Writing ops: `PUT /collab-api/note/{id}/append?siteId=N`

* Body: FlatBuffers `AppendOpsEvent{ops:[Op]}`; response: `NoteMutationResponse`. `siteId` = the bundle's `editorSite` (per device); op ids are `{site, timestamp}` Lamport pairs, `timestamp` = max seen + 1.
* `nb_protocol::codec` is a single schema-driven FlatBuffers encoder/decoder for every op table/struct/union (schema in `crates/nb-protocol/schema/notability-ops.fbs`). JSON shape: unions are `{"type": "<Table>", "value": {...}}`, colours stay `{bitsR, bitsG, bitsB, bitsA}`. `nb-domain` builds text/RGA ops and has one op-rebasing routine (used by .ntb import and duplicate).
* The server re-serialises with schema defaults (e.g. `CreatePage.pageCount == 1` comes back absent).

### Asset upload (images, PDFs, audio)

1. `mutation prepareMediaUpload(prepareMediaUploadParams:{md5Hash (base64), sha512Hash (hex), sizeInBytes})` -> `{uploadStatus, mediaUploadSignedUrl}` (`PENDING` = new content; content-addressed, repeats need no upload).
2. `PUT` the bytes to the GCS V4 signed URL with `Content-MD5`, `Content-Type: application/octet-stream`, `x-goog-content-length-range: 0,<size>`, **without** Notability cookies/token (a separate cookie-less HTTP client).
3. Append the op referencing `metadata{assetHash: sha512, fileName, mimeType, fileSize}`. The server then appends `AssetCloudPersisted{assetHash}` from site 65535 (also for re-referenced hashes and .ntb copies).
4. `GET /media/{sha512}?noteId=` -> signed download URL (fetch it cookie-less).

| UI action | Ops |
|---|---|
| Import PDF as new note | `SetMetadata{title}` + `CreatePage{pageCount=N, background{pdf{metadata, pageIndex}}}` |
| Insert PDF pages into note | `CreatePage{location (after page), pageCount, background.pdf}` |
| Insert image | `CreateBlock{type:1, textWrap:1, page, origin, image{metadata, size}}` |
| Home image -> new note | `CreatePage{pageCount 2}`, `SetMetadata{title, pageBackground{paper white 612x792, margins 36}, font Inter 14, layoutMode 1}`, centred `CreateBlock` |
| Home "Import files" | accept list: pdf, .ntb, doc/docx/xls/xlsx/ppt/pptx (server-converted), images. **No audio/video.** |
| Import .ntb | client `copyNote`: new note + replays the bundle ops (CLI: `rebase_ops` -> site 0, drop `AssetCloudPersisted`, keep `DeleteEntities`, append in 500-op chunks) |

### Recording

* There is no server "recording session" for normal recordings: the mic records locally, then uploads the audio asset and appends `CreateRecording{recording{metadata}, startTime, endTime, name}`. Delete = `DeleteEntities` on the recording id.
* Transcription (quota): `query getASRJob{sha512Hash}`; on `JOB_NOT_FOUND` the client calls `mutation createASRJob{sha512Hash, audioCreationDateMs, extension}` -> GCS POST policy; multipart upload (m4a gzipped with `Content-Encoding: gzip`) -> 204; poll `getASRJob` until `COMPLETED` (transcript segments). The web app runs this automatically for any recording without a job **when the note is opened**, and also kicks off `generateStudyMaterialsBatch` / `syncQuizSession`.
* Live transcription (quota): mirrors the web `ChunkUploadBuffer`. Audio is fragmented MP4 (AAC); chunk 0 carries `ftyp+moov` plus the first `moof+mdat`, later chunks one `moof+mdat` each.
  * `PUT /live-transcription/session/<uuid>`: body = concatenation of **all un-acked chunks**; headers `Content-Type: audio/mp4`, `Accept-Language`, `nb-collab-note-id`, `nb-last-audio-chunk-index` (cumulative chunks appended), `nb-request-id`.
  * `200 {transcriptionText, smartNotesText, lastAudioChunkIndexConsumed}` acks (drops) consumed chunks; `204` = everything consumed; `403` = quota/entitlement (QuotaError); 408/429/5xx retried (the web app retries 10x).
  * `POST .../finalize` -> `{id, status:"COMPLETED", transcriptionText, smartNotesText, lastAudioChunkIndexConsumed}`.
  * Not implemented in this CLI.
* CLI: `audio record start|stop` records locally with ffmpeg, then uploads and appends `CreateRecording`; `--transcribe` runs one ASR job (reusing an existing job for the same hash). `audio upload` attaches an existing audio file (video files have their audio track extracted).

### YouTube import

Client side only: oEmbed title -> `query getYoutubeTranscript{videoId, preferredLanguages}` -> body text ops at the RGA end of the note body: `InsertString(url + "\n\n")`, `ModifyStyle link`, `InsertString(label)`, `InsertString(title)`, `ModifyStyle size 12 + grey`, `ModifyStyle bold`, `InsertString(transcript)`, `InsertString("\n\n")`. No server job.

### Export ("Export as")

| Format | Mechanism | CLI |
|---|---|---|
| Rendered PDF (with ink/annotations) | client-side only in the web app (Apryse WebViewer); no server endpoint/job | `files export --to pdf`: **local renderer** (`nb-render`) - PDF backgrounds, paper, ink strokes (decoded path format below), shapes, images and typed text. Labelled approximation: rich text styling and pen textures are simplified. |
| Markdown / plain text | client-side | `--to md`, `--to txt` (typed text, block text and transcripts) |
| Notability (.ntb) | zip: `version` ("1"), `manifest.json {"appVersion"}`, `noteBundle`, `assets/<sha512>`, `thumbnail.png` | `--to ntb` |
| Source PDF / assets | `GET /media/{sha}` for each referenced asset; source PDFs joined | `--to source-pdf`, `--to assets` |
| Transcript | `getASRJob` per recording | `--to transcript` |

### Other UI commands as ops

Rename: `SetMetadata{title}` · comments: `CreateComment` · page reorder: `ModifyPage{moveTo}` · delete page/block/recording: `DeleteEntities` · template/paper: `ModifyPage.background` / `SetMetadata.pageBackground`.

### Ink path encoding (`CreateInk.encodedCenterPath`)

The same encoding is used for `encodedCustomPath`, `encodedFillPath` and
`AddPathElements.encodedCenterPathElements`. Older notes called this `CreateStroke.path`; the schema
name is `CreateInk.encodedCenterPath`.

All little-endian. `byte0 = version<<3 | precision`, then `u16` element count, then elements, each a
type byte followed by its points (`float16` coords normally, `float32` when any |coord| > 256):

| type | element | payload |
|---|---|---|
| 0 / 4 | (attributed) cubic | `c1, c2, end` |
| 1 / 5 | (attributed) quadratic | `c, end` |
| 2 / 6 | (attributed) line | `end` |
| 3 / 7 | (attributed) moveto | `end` |

Types 0-3 additionally carry per-point `width`, `force` (f16) and `altitude`, `azimuth` (u8).
`nb_protocol::inkpath` decodes and encodes this (round-trip tested).

### Comment authors

`Comment.authorSite()` is the op id's `site`. The web app names it from live peers, else from
`collabPresence.collaboratorsByNote[note].userIdBySite` (GraphQL `getSiteUserMappings`) -> user
display name, else `Anonymous <site>`. `ensureCollaboratorsLoaded` fetches the mapping once per note
per session, so a comment written by a *new* site (e.g. the CLI's device) shows "Anonymous N" in an
already-open tab until reload; the server mapping does include CLI sites. `content comments list` resolves
authors the same way.

### Organizers: subjects and dividers

Legacy organizer kinds next to folders (library rows with `organizerType` `Subject`/`Divider`;
plain folders are `Folder`). Metadata `change` kinds:

| kind | payload |
|---|---|
| `createSubject` | `{id, hexColor, isLocked, title, siblingOrder, workspaceId, parentId}` |
| `createDivider` | `{id, title, siblingOrder, parentId}` |
| `modifyOrganizer` | `{organizerId}` + any of `title`, `subjectHexColor`, `subjectIsLocked`, `subjectEmoji`, `siblingOrder`, `moveToParent{parentId, workspaceId}` |
| `deleteOrganizer` | `{organizerId, childrenHash?}` |

Verified live (create, rename, recolour, delete on disposable test items). CLI: `folders organizers`.

### TOC-based quiz

`createTableOfContentsBasedQuiz` selects the first 3 top-level outline items of the note's PDF
(`setSelectedTableOfContentsItem` also selects their children), then `ensureLearnContent` ->
`generateStudyMaterialsBatch` with `studyMaterials:[{content:<text of the selected sections' page
ranges>, type:NOTE_PDF}]`. Not implemented in this CLI; `files toc` reads the outline only.

### Chat retry and recent-notes backfill

* Opening the chat panel fires `POST /collab-api/rag/recent-notes-backfill` (no body) so recently
  edited notes are indexed for retrieval. CLI `chat backfill` (verified, HTTP 200).
* `retryLastTurn` -> `ChatManager._classifyAndReplay`: after a broken stream the thread is re-read;
  if `nextExchangeIndex` > the sent index the turn was committed and its reply is adopted without a
  re-POST, else it is replayed once at the corrected index (index 0 is simply restarted). `chat ask`
  does the same (unit-tested).

### Calendar switches (device-local, not an API)

`setCalendarSwitch {connectedAccountId, calendarId, isOn}`, `setCalendarGroupFolder`,
`setCalendarGroupDismissed`, `setCalendarSeriesNotify` and `setCalendarManualGroup` are messages to
the web app's own persistence worker; they write IndexedDB (`switchChoices`, ...) and broadcast to
tabs. No network request is made, so there is nothing server-side for the CLI to toggle.

## Edit-log op reference

The schema the codec is built from is
[`crates/nb-protocol/schema/notability-ops.fbs`](../crates/nb-protocol/schema/notability-ops.fbs).
[`notability-ops.annotated.fbs`](notability-ops.annotated.fbs) is the same schema with named enums
and field notes, reconstructed from the web app's FlatBuffers code.

The codec round-trips byte for byte:
- the 50 probe vectors in `crates/nb-protocol/tests/fixtures/wire/ops/`;
- `crates/nb-protocol/tests/fixtures/wire/web_ui/web_editor_ops.json` (89 ops written by the web editor);
- the reference encodings in `crates/nb-protocol/goldens/` (every table, struct and union variant).

Per-command status is in [STATUS.md](STATUS.md).

### Wire conventions

* Root of an append body: `AppendOpsEvent{ops:[Op]}`. Root of a bundle: `NoteBundle`.
* `Op = {id: Id!, clientTime: ulong, serverTime: ulong, audioTime: double?, payload: OpPayload!, transientInteraction: bool}`.
  * The server fills in `serverTime`.
  * `audioTime` links an op to the position in a recording that was playing when it was written. The iPad sets it while recording.
* `Id{site:u16, timestamp:u32}` is an 8-byte struct. `SeqId{site, timestamp, index:u32}` is a 12-byte struct.
* Other structs:
  * `Color{bitsR,G,B,A: u8}`;
  * `Point{x,y: f32}`;
  * `Size{width,height: f32}`;
  * `Rect{origin, size}`;
  * `Boundary{location: SeqId, type: BoundaryType}` (16 bytes).
* `Set*` wrapper tables (`SetBool`, `SetFloat`, `SetColor`, `SetString`, `SetRect`, `SetSeqId`, …) mean "modify this field". The field itself is one-element `value`.
  * The table missing = leave the field unchanged.
  * The table present with `value` missing = reset to the default.
  * The web uses the reset form when it creates a link: `foregroundColor:{}` and `underline:{}`.
* Optional scalars (`= null` in the `.fbs`) are FlatBuffers optional scalars. Absent is not the same as zero.
* **Struct union members.** These union members are *structs*, not tables:
  * `Anchor.CanvasAnchor` and `Anchor.ReplyAnchor`;
  * all three `Endpoint` variants (`CardinalAttachedEndpoint`, `FreeEndpoint`, `FractionalAttachedEndpoint`).

  The union value offset points straight at the inline struct bytes. `flatc` generates C++ and TypeScript for this, but refuses some other targets ("only tables can be union elements"). `nb_protocol::codec` reads these members by hand: it follows the `uoffset`, then reads the fixed struct layout.
* Union discriminants follow the order of `union OpPayload` in the `.fbs`: `SetMetadata = 1`, …, `ModifyConnector = 33`. `NONE` is 0.
* The site id is per device, per note: it is the bundle's `editorSite`. Lamport `timestamp` is the max seen + 1.
  * The server writes some ops itself as site **65535**. For example, `AssetCloudPersisted{assetHash}` arrives right after an image `CreateBlock`.
* An ack is not semantic validation. All 50 probes were acked, including some with field combinations no client writes. Check that an op renders before relying on it.

### Text model

* Text is an RGA of characters. The id of each character is a `SeqId`: the inserting op's `(site, timestamp)` plus `index` = the character's offset within that op's string.
* `InsertString.location` = the character to insert after. Absent means the start of the text. `textField` = the id of the text-box block. Absent means the note body.
* `RemoveChar{location}` and `RemoveChars{locations}` tombstone characters, and `ReviveChars{locations}` undoes that. Undo uses revive.
* Ranges use `Boundary{location, type}`. The usual shapes are:
  * `Before(0)` / `After(1)` the given character;
  * `StartOfDoc(2)` / `EndOfDoc(3)`, where `location` is ignored.
* **Paragraph ops are keyed by the newline that starts the paragraph.** For the first paragraph, that is the first character. Two forms:
  * `ModifyParagraphStyle{start}` with no `end` applies from that paragraph to the end of the document.
  * `{start, end}` applies to an inclusive range of paragraph keys. The iPad writes this range form.
* `UpdateCheckbox{textField?, location: the paragraph's key SeqId, isChecked}` is what the web writes when a checkbox is clicked. Setting `ModifyParagraphStyle.isChecked` also works.

### Field reference (! = required)

| Op | Fields | Semantics / observed values |
|---|---|---|
| `SetMetadata` | `title`, `pageBackground`, `handwritingLanguage`, `alignTextToLines`, `defaultFontFamily`, `defaultFontSize`, `layoutMode`, `blockWrapSupport`, `coverPageId: SetSeqId`, `measurementScale{scaleFactor, unit}`, `measurementsShown` | last writer wins per field. Order: Lamport ts, then site |
| `InsertString` / `InsertChar` | `location?`, `string` or `unicodeScalar`, `textField?` | newline = `\n` |
| `ModifyStyle` | `start!`, `end!: Boundary`, `textField?`, `bold`, `italic`, `underline`, `strikethrough`, `superscript`, `subscript`, `code: SetBool`; `highlight`, `foregroundColor: SetColor`; `familyName`, `link: SetString`; `size: SetFloat` | Web writes one attribute per op. Highlight FFF600/0x59. Heading = size 30/24/18 + bold. A link is two ops: reset `underline` + `foregroundColor`, then `link` |
| `ClearStyle` | `start!`, `end!`, `paragraph`, `textField?` | `paragraph:true` also clears paragraph styles |
| `ModifyParagraphStyle` | `start!`, `end?`, `textField?`, `indentLevel: u8`, `alignment: ParagraphAlignment`, `lineSpacing: f32`, `decoratorStyle: DecoratorStyle`, `isChecked`, `programmingLanguage: string`, `writingDirection` | All fields are `Set*`. Decorators: Bullet 1, Number 2, CheckBox 3, BlockQuote 4, CodeBlock 5. Code block default language `plaintext`; the web renders `<pre data-language=…>` |
| `UpdateCheckbox` | `textField?`, `location`, `isChecked` | see Text model above |
| `CreateBlock` | `type: BlockType (Text 0, Image 1, Math 2)`, `corner`, `page!`, `origin!`, `rotation`, `scale`, `size!`, `textWrap`, `enableCaption`, `zIndex`, `image: ImageAsset`, `cropRect: Rect`, `webUrl`, `mathLatex`, `mathColor`, `paper`, `imageFlippedHorizontally/Vertically`, `resizesWidthToFitText`, `margins`, `positionLocked` | Web math paste = `{type: 2, mathLatex, mathColor}`. The image block written by the CLI is `textWrap 1` with `image{metadata{assetHash, fileName, mimeType, fileSize}, size: pixel size}` |
| `ModifyBlock` | `blocks!: [Id]`, plus optional overrides. `Set*` tables: `rotation`, `scale`, `mathLatex`, `mathColor`, `cropRect: SetRect`, `paper: SetPaper`. Optional scalars: `corner`, `textWrap`, `enableCaption`, `zIndex`, `imageFlippedHorizontally/Vertically`, `resizesWidthToFitText`, `positionLocked`. Plain structs (absent = unchanged): `page`, `origin`, `size` | crop rect and flip were accepted. The crop rect is presumably in display points; render not verified (no WebGL) |
| `CreateInk` | `page!`, `origin!`, `rotation`, `scale`, `tool: Tool`, `style: InkStyle`, `tapePattern`, `color!`, `width`, `encodedCenterPath`, `encodedCustomPath`, `encodedFillPath`, `fillColor`, `styleMap: [StyleMap]`, `zIndex`, `audioDuration`, `nibAngle`, `nibFlatness`, `inkEffects`, `inkEffectsTinted = true` | Web pen: `tool 0, style 1 (FixedWidth), #171a20, width 2`. Web highlighter: `tool 2, style 1, FFF600/0x59, width 12, zIndex 999999`. The iPad writes `style 0` with variable widths (for example 0.398) and `audioDuration` |
| `AddPathElements` | `ink!`, `encodedCenterPathElements`, `encodedCenterPathEstimatedElements` | extends a stroke that is still being drawn |
| `ModifyInk` | `inks!` + same fields as `CreateInk`, all optional | |
| `CreateShape` | `page!`, `origin!`, `rotation`, `scale`, `definition!: ShapeDefinition`, `tool`, `style = FixedWidth`, `tapePattern`, `color!`, `borderWidth = 4`, `fillColor`, `zIndex`, `smartHighlight`, `force`, `positionLocked`, `inkEffects`, `inkEffectsTinted`, `measurementUnit` | `ShapeDefinition` is one of `Line{start!, controlPoint1?, controlPoint2?, end!, arrowHead}`, `Polygon{points!}` or `NormalShape{type: Ellipse, size!}`. Web: rect/triangle are `Polygon`, ellipse is `NormalShape`, arrow is `Line{arrowHead: 1}` |
| `ModifyShape` | `shapes!` + optional geometry/style fields | the web writes it when a line endpoint is dragged |
| `ModifyPositions` | `modifications!: [ModifyPosition{target!, page?, origin?, rotation?, scale?, zIndex?}]` | moving a lasso selection. Origins are absolute |
| `DeleteEntities` | `entityDeletes`, `entityUndeletes: [Id]`, `pageDeletes`, `pageUndeletes: [SeqId]` | eraser, delete, undo |
| `CreateGroup` / `ModifyGroup` | `members!` / `group!, members` | custom stickers start from a group |
| `CreateConnector` | `start!`, `end!: Endpoint`, `pathStyle = Orthogonal`, `bends`, `startCap`, `endCap = FilledTriangle`, `tool`, `style`, `color!`, `borderWidth = 4`, `zIndex`, `positionLocked`, `labelPosition` | the meanings of `CardinalAttachedEndpoint.side` and `Bend.pin` are unresolved |
| `CreateComment` | `anchor!: Anchor`, `text` | `CanvasAnchor{page, origin}`, `ReplyAnchor{root}`, `TextAnchor{textField?, selection{anchor, focus}}`, `EntityAnchor{entities}` |
| `ModifyComment` | `comment!`, `anchor?: CanvasAnchor`, `text?: SetString`, `resolved?: SetBool` | web: moving a pin writes `anchor`, editing writes `text`. The web has no resolve control |
| `CreateRecording` / `ModifyRecording` | `recording!`, `startTime`, `endTime`, `name`, `segmentation`, `zIndex` | see Recording above |
| `ModifyPDFField` | `assetHash!`, `key`, `valueString`, `valueBoolean` | PDF form fields; not probed |
| `PeerInteraction` | `cursorPosition`, slots 1-2 deprecated, `selectedEntities`, `tool: PeerTool`, `textSelection`, `recordingInProgress` | collaborator presence. Sent with `transientInteraction` and not persisted |

### Web UI ground truth

These are the ops the web editor writes as site 1, recorded on a throwaway test note and stored (ids replaced) in `crates/nb-protocol/tests/fixtures/wire/web_ui/web_editor_ops.json`.

| UI action | Ops |
|---|---|
| Ctrl+B / I / U | one `ModifyStyle` each |
| Highlight | `ModifyStyle{highlight: FFF600/0x59}` |
| Add link | `ModifyStyle{underline:{}, foregroundColor:{}}`, then `ModifyStyle{link}` |
| `# ` | `ModifyStyle{size 30}` + `{bold}` |
| `- ` / `1. ` / `> ` / ```` ``` ```` | `ModifyParagraphStyle{decoratorStyle 1 / 2 / 4 / 5 (+ programmingLanguage "plaintext")}` |
| Click a checkbox | `UpdateCheckbox{location, isChecked: true}` |
| Paste `$$x^2$$` | `CreateBlock{type 2, mathLatex, mathColor}` |
| Pen stroke | `CreateInk{style 1, #171a20, width 2}` |
| Highlighter stroke | `CreateInk{tool 2, FFF600/0x59, width 12, zIndex 999999}` |
| Rect / triangle / ellipse / arrow / line | `CreateShape{Polygon / Polygon / NormalShape / Line{arrowHead 1} / Line}` |
| Drag a selection | `ModifyPositions` |
| Drag a line endpoint | `ModifyShape{definition}` |
| Eraser | `DeleteEntities` |
| Comment: add / move pin / edit | `CreateComment{CanvasAnchor}` / `ModifyComment{anchor}` / `ModifyComment{text}` |

The web app's Insert menu offers only Image, Document, YouTube and Google Drive. There are no stickers, tables or web clips. Changing a code block's language in the web UI wrote no op when observed, so `programmingLanguage` was verified by writing it through the API.
