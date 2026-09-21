# Napstr playlist event: kind `30425`

> **Status:** draft proposal for upstream inclusion in
> [lnbits/napstr `PROTOCOL.md`](https://github.com/lnbits/napstr/blob/main/PROTOCOL.md).
> First implemented by napstr.net (2026-09-01), and implemented for publication by
> Napstr (`src-tauri/src/playlist.rs`). Normative keywords follow the main
> document: **MUST**, **MUST NOT**, **SHOULD**, **MAY**.
>
> **Public only:** this kind carries public playlists and nothing else. There is no
> private variant of it, and no relay-backed private tier either: a private
> playlist stays on its owner's own devices. See "Privacy".
>
> **Kind selection (probed 2026-09-20):** `30425` is where this format is already
> deployed, so it stays. It is also the most heavily co-occupied coordinate in
> the block. An unrelated encrypted game protocol holds 70 events in `30425`
> (`story:*` and `character:*` markers, spanning 2026-04-28 to 2026-09-18),
> alongside 22 in `30424` and 5 in `30426`. Cohabitation is workable **only**
> because the marker tag is mandatory: a client that queries kind `30425`
> without a `#t` filter receives roughly seventy foreign events per playlist.
> `30427` carries album covers.
>
> **Provisional:** the optional `artist` and `mbid` album-enrichment fields are
> newly added and untested in the wild. They are additive — a client that ignores
> them is unaffected — and may be withdrawn without breaking the kind.

A playlist is a named, ordered, mutable list of file IDs. It is a curation
object, not a file: it owns no bytes, publishes no availability, and asserts
nothing about who holds the files it names. Every member MUST still be
independently discoverable and transferable as an ordinary kind `30421`
catalogue entry, so a client that ignores this kind entirely remains fully
interoperable.

Playlists are **additive assertions**. A playlist MUST NOT remove, suppress, or
reclassify any kind `30421` entry, and MUST NOT be treated as evidence of
availability.

Kind `30425` is a parameterized-replaceable (addressable) event. Its `d` tag is
a playlist ID, so the newest valid event from one author for that ID replaces
the author's previous revision of that playlist. Because the ID is stable across
revisions, edits — adding a track, renaming, reordering — replace in place.

## Event shape

Required tags:

```json
[
  ["d", "<playlistId>"],
  ["t", "napstr-playlist"],
  ["title", "<playlist title>"],
  ["alt", "Napstr public playlist"]
]
```

Optional tags:

```json
[
  ["x", "<memberFileId>"],
  ["t", "<search word>"],
  ["client", "Napstr"]
]
```

- `d`: the **playlist ID** — a lowercase canonical UUID, see below.
- `t`: the literal marker `napstr-playlist`, followed by search words. The marker
  MUST be present and is the only supported discovery path. The words after it are
  the author's own tags, and see "Author tags and suggested words" for the one
  case in which a client may add words of its own. A playlist MAY carry no words
  after the marker at all, which is a valid choice, not a gap to be filled.
- `title`: the playlist's display name. MUST be present, non-empty, and bounded
  by the catalogue metadata rules (at most 256 characters, no unsafe control or
  bidirectional formatting characters).
- `x`: one tag per member file, **in member order**. See "Membership tags".
- `alt`: the literal `Napstr public playlist`, see "Changes from the deployed
  event" below for why the word `public` is kept.
- `client`: optional provenance hint, informational only.

There is deliberately **no** `x` tag carrying the playlist's own ID. This is the
opposite of kind `30423`, where `x` repeats `d`; here `x` carries member file
IDs, which is the content-hash meaning `x` has in NIP-94 (the SHA-256 of a file)
and NIP-56 (the SHA-256 of reported content). Kind `30423` is the outlier in this
block, not this kind. Consumers MUST NOT assume `x == d` on a playlist.

Example:

```json
{
  "kind": 30425,
  "tags": [
    ["d", "77abf082-7075-4d36-afe2-e9710ac6b33c"],
    ["t", "napstr-playlist"],
    ["title", "rock"],
    ["alt", "Napstr public playlist"],
    ["client", "Napstr"],
    ["x", "48e5979efa6a56dc3cab293b954ae84e36f464b936bd8c534838189abcc93c68"],
    ["x", "cdf1741591bf1e580b1e7a2712ce781ef7ade8b12ebf309731d264498accf5ce"],
    ["t", "rock"],
    ["t", "metallica"],
    ["t", "alice"],
    ["t", "chains"]
  ],
  "content": "{\"protocol\":\"napstr/1\",\"playlistId\":\"77abf082-7075-4d36-afe2-e9710ac6b33c\",\"title\":\"rock\",\"tracks\":[{\"position\":1,\"fileId\":\"48e5979efa6a56dc3cab293b954ae84e36f464b936bd8c534838189abcc93c68\",\"title\":\"Enter Sandman\",\"artist\":\"Metallica\",\"album\":\"Metallica\"},{\"position\":2,\"fileId\":\"cdf1741591bf1e580b1e7a2712ce781ef7ade8b12ebf309731d264498accf5ce\",\"title\":\"Rooster\",\"artist\":\"Alice In Chains\"}]}"
}
```

## Playlist identity

The `d` tag and `content.playlistId` are the same value: a lowercase canonical
UUID in `8-4-4-4-12` hexadecimal form. Publishers SHOULD generate a version 4
(random) or version 7 (time-ordered) UUID. Consumers MUST accept any canonical
UUID rather than validating a specific version nibble.

This diverges deliberately from kinds `30421` and `30423`, where `d` is a
SHA-256 of the content. A playlist's identity is its **name and role for its
author**, not its contents. Content-hashing the member list would be wrong here
in a way it is not for an audiobook edition: two playlists by one author
containing identical tracks under different titles — "rock" and "driving" —
would collide on a single coordinate, and the second publish would silently
overwrite the first.

The cost of a UUID identity is that a coordinate cannot be recomputed from local
state, so an author cannot derive "which playlists did I publish?" the way the
reference client derives audiobook edition IDs from its shared folders. Clients
MUST therefore persist the playlist IDs they have published, and see
"Withdrawal and stale playlists" for the reconciliation path when that state is
lost.

## Content

The content is JSON with camel-case property names, at most 128 KiB — the same
budget as a kind `30423` manifest:

```json
{
  "protocol": "napstr/1",
  "playlistId": "77abf082-7075-4d36-afe2-e9710ac6b33c",
  "title": "rock",
  "tracks": [
    {
      "position": 1,
      "fileId": "48e5979efa6a56dc3cab293b954ae84e36f464b936bd8c534838189abcc93c68",
      "title": "Enter Sandman",
      "artist": "Metallica",
      "album": "Metallica"
    }
  ]
}
```

- `protocol`: the literal `napstr/1`.
- `playlistId`: MUST equal the `d` tag value. A withdrawal body is the one body
  that does not carry it, because it describes no playlist — see "Withdrawal and
  stale playlists".
- `title`: the playlist display name. MUST equal the `title` tag value.
- `tags`: optional, the author's own comma-separated search words, in the same
  shape and under the same rules as a catalogue entry's `tags` (at most 12 words,
  at most 32 characters each, no control or bidirectional formatting characters,
  duplicates removed case-insensitively). These are the author's own words. The
  `t` tags carry them tokenised, and a client MUST NOT add words of its own to an
  author's list — see "Author tags and suggested words".
- `artist`, `mbid`: optional album enrichment, see "Album enrichment and cover
  resolution" below.
- `tracks`: between 1 and 500 members. `position` values MUST be contiguous and
  MUST start at 1. `fileId` values MUST be valid lowercase 64-character SHA-256
  IDs and MUST be **unique within the playlist**; a playlist MUST NOT repeat a
  file ID. Ordering is defined by `position` alone.

Unknown properties MUST be ignored.

A playlist is intentionally slim. It carries no `filename`, `format`, `mime`,
`size`, or `totalSize`. Filename and size reach the downloader over the transfer
protocol itself — the `WELCOME` frame carries `file_id`, `filename`, and `size` —
and format, MIME, and extension agreement are properties of the catalogue entry,
which is where they are validated. The only per-member fields are the file ID and
optional display hints.

## Album enrichment and cover resolution

A playlist MAY represent a single release group — an album — rather than a
personal mix. The member list is unchanged; two optional content fields are
added:

```json
{
  "protocol": "napstr/1",
  "playlistId": "0f9de6a1-5c2e-47a8-9a3c-2f6f8f1b7d40",
  "title": "Blood on the Dance Floor: HIStory in the Mix",
  "artist": "Michael Jackson",
  "mbid": "60691bed-fdd7-32f9-92dc-b151aac9e271",
  "tracks": [
    {
      "position": 1,
      "fileId": "48e5979efa6a56dc3cab293b954ae84e36f464b936bd8c534838189abcc93c68",
      "title": "Blood on the Dance Floor"
    }
  ]
}
```

- `artist`: the album artist. Optional display text, bounded by the catalogue
  metadata rules and treated as untrusted.
- `mbid`: the MusicBrainz **release-group** MBID, matching the meaning and
  spelling of `mbid` in kind `30427`.
- The two accompany each other: a publisher SHOULD include `artist` whenever
  `mbid` is present, because the artist half is required to form a cover key.
- Both are self-asserted organisation metadata. Any identity may claim any MBID,
  so clients MUST NOT use either field for trust decisions, availability, or
  membership.

Cover resolution needs no pointer between the two kinds, because the cover key is
derivable from fields a playlist already carries:

1. Normalize `artist` and `title` exactly as kind `30427` specifies to form the
   cover key.
2. Query covers by `#d` with both the verbatim key and its canonical alias, using
   the existing cover consumption flow and its bounded chunking.
3. When `mbid` is present it MAY reconcile keys that fail to match, which is the
   canonicalization role kind `30427` reserves for it. It MUST NOT override a
   successful key match, and its absence MUST NOT prevent one.

A client that ignores `artist` and `mbid` remains fully interoperable: they change
nothing about membership, ordering, or availability.

## Member hints and precedence

`title`, `artist`, and `album` on a member are **display hints only**. They exist
so that a member whose file can no longer be found still renders as something a
human recognises.

Clients MUST apply this precedence:

1. When the member's kind `30421` catalogue entry has been fetched, its
   `filename`, `title`, `artist`, and `album` are authoritative and MUST
   override the hint.
2. When no catalogue entry is available — the file was never shared, was
   withdrawn, or the entry is missing from the author's current relay set —
   clients MAY render the hint, and SHOULD mark it unverified.
3. When neither is available, clients MUST render the file ID, truncated if
   space requires, rather than omitting the row.

Hints MAY be empty or absent. They MUST NOT be used for identity, availability,
validation, transfer, or as the destination filename. They MUST NOT be used to
reject a playlist: an event whose hints disagree with the catalogue entry is
still valid, and the catalogue entry wins.

## Membership tags

Each member contributes one `x` tag holding its file ID, in `position` order.
These exist so relays can index membership and answer "which playlists include
this file" without fetching and parsing every playlist's content:

```json
{"kinds":[30425],"#t":["napstr-playlist"],"#x":["<fileId>"],"limit":500}
```

The `x` tags and `content.tracks` carry the same set of IDs. Consumers MUST treat
the content as authoritative for order and MUST NOT infer playback order from tag
order, because a consumer that merges several playlists sees a tag list, not a
sequence.

## Author tags and suggested words

A playlist is found by the words tagged on it, and those words belong to its
author. `content.tags` holds exactly what the author chose; the `t` tags carry
the same words tokenised per the catalogue's search rules. A client MUST NOT add
search words of its own to an author's list: a word the author did not choose is
a word that can misrepresent what the playlist is, and an author who wants their
playlist to be found only by the marker — or by nothing at all beyond its
members — is entitled to that.

A client MAY offer to suggest words, and if the author accepts, the suggestions
are additional `t` tags. Three rules make that safe:

- Suggestions apply **only** while the author has written no tags of their own.
  One word of their own turns the suggestions off for that revision; the author's
  list is then the whole answer, not the beginning of one.
- Suggestions MUST NOT be written into `content.tags`. That field is the author's
  own choice, and a suggestion read back as theirs would quietly become theirs on
  the next edit of the same playlist.
- Suggestions MUST be words the playlist itself carries: its title, or the titles,
  artists and albums of its members. A client MUST NOT invent topical words, and
  MUST NOT copy words from anywhere the playlist does not name.

**No words at all is a valid answer, and clients MUST support it.** An author may
want a playlist that is not findable by search: reachable by its coordinate, by
its members, or by a link someone shares, but by no word. A playlist whose
`content.tags` is empty and whose only `t` tag is the marker is a valid event and
MUST NOT be rejected, and its emptiness MUST NOT be read as "this author has not
answered yet".

Neither form of the answer is visible in the event, because declining
suggestions and never having been asked both leave `content.tags` empty. The
distinction therefore lives in the client, and a client MUST keep it:

- It SHOULD ask once per playlist rather than on every publish, and MUST
  remember the answer.
- It MUST NOT offer words again for a playlist whose author has declined, and
  MUST NOT add words to a later revision of it on the strength of a fresh
  question.
- It MUST pass the declined answer to the publisher. An author who has said no
  must be able to keep saying no without retyping it, so the negative answer is
  client state, not an absent field in the event.

The published `t` tag order after the marker is therefore the author's words
first, in the order they gave them, then any accepted suggestions. Consumers
MUST NOT read meaning into that order; it is not part of the playlist's ordering
semantics, which live in `position`.

## Curation and availability

Playlists are **curation**, not seeding assertions. A publisher of a playlist
need not hold, seed, or even have ever held any of its members. Members are
expected to be seeded by third parties, which is the point: a playlist can
circulate files its author does not have.

Consequences that consumers MUST observe:

- A playlist MUST NOT be treated as a seeder claim. The complete-active-seeder
  rule that applies to kind `30423` audiobook manifests does **not** apply here.
- Member availability MUST be resolved exclusively from unexpired kind `30422`
  heartbeats, from **any** author — not only the playlist author.
- A playlist whose author is offline, or has deleted their catalogue, is still
  fully usable while its members are seeded elsewhere.
- An unavailable member MUST NOT invalidate the playlist. Clients SHOULD render
  per-member availability and MAY disable playback of unavailable members, but
  MUST NOT silently drop them, because `position` ordering is part of the
  playlist's meaning.
- Downloading MUST use the normal NIP-17 negotiation and transfer protocol, with
  the consumer choosing seeders itself. Playlist ordering grants no transfer
  priority and conveys no capability.

## Consumption

Typical flow for a catalogue browser:

1. Discover playlists by marker, and by member when reconstructing a library
   view:

   ```json
   {"kinds":[30425],"#t":["napstr-playlist"],"limit":500}
   ```

2. Validate each event, then resolve members against current availability:
   collect the distinct member file IDs, and intersect them with the active
   `(author, fileId)` pairs already built from kind `30422`. Hydrate missing
   catalogue entries with bounded `#d` pages exactly as for ordinary files:

   ```json
   {"kinds":[30421],"#t":["napstr"],"#d":["<fileId1>","<fileId2>"],"limit":500}
   ```

3. Render in `position` order using catalogue values where hydrated and hints
   otherwise, showing per-member availability and the seeder count. Seeders
   SHOULD be ranked by distinct active authors.

4. Enqueue or stream members in `position` order. Playback order MUST follow
   `position`; fetching order MAY be parallel.

Playlists MUST NOT be used to enumerate an unbounded public catalogue. A UI that
lists playlists SHOULD bound the result set the same way catalogue browse is
bounded in the main document.

A companion that never talks to relays — Napstrfy, which holds no Nostr keys —
does not consume this kind at all. It asks its own desktop instead, over the
companion protocol in `PROTOCOL.md` (`playlists` for the list, `playlist` for one
playlist's members, `libraryByIds` to turn member file IDs into playable
catalogue records), and the desktop is the side that discovers, validates and
hydrates. A private playlist reaches a companion the same way, which is the only
way it reaches anything at all.

## Relationship to kind 30423

Playlists and audiobook manifests are the same envelope with different member
semantics. The shared parts are intentional and clients SHOULD share one
validator:

| aspect | `30423` audiobook | `30425` playlist |
| --- | --- | --- |
| identity | SHA-256 of `napstr-audiobook-v1\0` + ordered IDs | UUID, stable across edits |
| `x` tags | the collection's own ID (`x == d`) | one per member, in member order |
| member fields | `position, fileId, filename, title, format, mime, size` | `position, fileId` + hints |
| `totalSize` | required, MUST equal sum | absent, no size claim |
| duplicate members | forbidden | forbidden |
| content budget | 128 KiB | 128 KiB |
| member limit | 500 | 500 |
| seeder rule | publisher must seed every member | no seeding requirement |
| duplicates/modifier | `author`, `narrator` | `artist` hint |

`totalSize` is deliberately absent. Without per-member `size` it could only be
restated from hints, which are not authoritative, so a size claim would be
unverifiable. Clients that want a total size SHOULD sum hydrated catalogue sizes
and present it as a computed value, not as a playlist claim.

## Relay limits

NIP-01 imposes no limit on the number of tags or on filter value counts, so a
500-member playlist is valid protocol-wise: roughly 500 `x` tags plus marker,
title, `alt`, `client`, and up to 20 search words, around 530 tags.

Relays vary. Many advertise limits in their NIP-11 `limitation` object, commonly
including a maximum event tag count, a maximum event size, and a maximum number
of values per filter. Publishers MUST tolerate a relay rejecting an oversized
playlist event and MUST surface the failure rather than silently publishing
nothing. Because the reference client already caps audiobook manifests at 500
members and 128 KiB, playlists SHOULD share those bounds so one set of constants
and one validator serve both kinds.

Consumers MUST bound `#x` filter value counts. The reference cover consumer
chunks `#d` filters at 75 values and at most 4 concurrent requests; a playlist
consumer SHOULD chunk `#x` the same way. Note that "which playlists contain any
of my N files" degrades immediately at library scale and SHOULD NOT be attempted
as a single filter.

## Withdrawal and stale playlists

An author retracts a playlist by replacing the same coordinate with the
catalogue withdrawal body:

```json
{
  "kind": 30425,
  "tags": [
    ["d", "<playlistId>"],
    ["t", "napstr-playlist"]
  ],
  "content": "{\"protocol\":\"napstr/1\",\"deleted\":true}"
}
```

Consumers MUST treat the latest event at that coordinate from that author as
withdrawn and MUST NOT offer the author's older revision.

Because a playlist ID cannot be recomputed from local state, a publisher that
loses its record of published playlist IDs would otherwise leave orphaned
playlists on relays that no client can distinguish from live ones. Clients MUST
persist published playlist IDs. A client MAY offer a **user-confirmed**
reconciliation: query its own events with `{"kinds":[30425],"authors":[<self>],
"#t":["napstr-playlist"]}` and present relay-known IDs missing from local state
for the user to restore or withdraw.

Such reconciliation MUST NOT be automatic. A second installation of the same
identity with a different local state — a laptop with a small library, a fresh
install whose share folder has not been indexed yet — would otherwise withdraw
the other installation's playlists. The main document's audiobook sweep, which
withdraws manifests whose local folder has disappeared, has the same shape and
the same hazard.

## Validation requirements

An interoperable consumer MUST:

- validate the event ID and signature per NIP-01;
- require `protocol` = `napstr/1` and the literal `napstr-playlist` marker tag;
- require a canonical lowercase UUID `d` tag, which is the playlist's coordinate
  and therefore its identity together with its author. In a revision it MUST
  equal `content.playlistId`; a withdrawal body MAY omit that property entirely,
  because the `d` tag is the id;
- treat `{"protocol":"napstr/1","deleted":true}` at a valid coordinate as a
  withdrawal rather than as a malformed playlist, and MUST NOT offer the author's
  older revision of it;
- require a non-empty bounded `title` equal to the `title` tag;
- treat `content.tags`, when present, as the author's own search words and bound
  them by the catalogue tag rules rather than rejecting the playlist over one: a
  word that cannot be carried is dropped, and a list longer than the rules allow
  is cut short. The words themselves are never authoritative for anything but
  finding the playlist;
- accept a playlist with no search words beyond the marker, and MUST NOT invent,
  require, or restore words for it. Discovery by word is one path to a playlist,
  not a property it has to have;
- require 1 to 500 members with contiguous positions starting at 1, valid
  lowercase 64-character hexadecimal file IDs, and no duplicate file IDs;
- require content at most 128 KiB;
- treat `title`, member `artist`/`album` hints, top-level `artist`, and `mbid` as
  untrusted display and organisation text, bounded by the catalogue metadata
  rules, and never as authoritative;
- require `mbid`, when present, to be a canonical lowercase UUID, and ignore it
  when it is not;
- ignore `x` tags that do not correspond to a member, and ignore any `x` tag
  count that disagrees with the member list, because the content is
  authoritative;
- resolve availability only from unexpired kind `30422` heartbeats.

A publisher MUST NOT include local filesystem paths, private keys, or transfer
capabilities anywhere in the event.

A publisher SHOULD refuse to publish a playlist that names file IDs it cannot
resolve to any known catalogue entry or availability record, unless the user is
explicitly curating from memory. A playlist of unresolvable IDs renders as a list
of unverified hints, which is worse than no playlist.

## Privacy

Playlists are public by design and reveal organisation that individual kind
`30421` entries do not: a playlist groups a subset of a library into a named,
ordered unit, which is a stronger statement about its author's taste and
collection structure than any single track entry. Clients SHOULD make clear that
publishing a playlist publishes its title and membership.

There is no private playlist variant of this kind, and a `napstr-private-playlist`
marker MUST NOT be added to it. A public marker on private data is a
contradiction: `t` tags are indexed and queryable by anyone, so the marker would
publish the existence, the count, and the edit cadence of every author's private
playlists — precisely the information the feature exists to conceal.

Private state also needs a different trust model: it must be readable only by its
owner. No relay can express "half of kind `30425` is owner-only", and the
per-kind answer does not survive contact with the threat either. Kind `30078`
(NIP-78 application data) would have to be gated by relay policy — NIP-78 says
relays SHOULD require a NIP-42 `AUTH` flow before accepting or serving its kinds
— but that is a SHOULD on infrastructure its author does not control, and what a
relay always sees is the coordinate, the event size and the edit timestamp, no
matter how the body is encrypted. An app-scoped `d` value such as
`napstr/playlists/v1` would therefore publish the existence, and the rough scale,
of a private playlist; every edit to any private playlist would rewrite that one
event, publishing the edit cadence too; and a store of several playlists could
grow past a relay's event-size limit and become unpublishable. That is precisely
the information this feature exists to conceal.

Clients MUST therefore keep private playlists off Nostr entirely: not as kind
`30425`, and not as NIP-78 application data either. A private playlist is local
state. The reference implementation stores it beside the public ones and serves
it to its owner's own paired companions over the companion channel described in
`PROTOCOL.md`, which is already end-to-end encrypted to that owner and to nobody
else — so a companion loses nothing by there being no relay copy. The one thing
this gives up is durability across installs of the same identity, and the honest
place for that is an explicit export the user performs, not a public relay.

## Changes from the deployed event (non-normative)

A napstr.net client published one playlist on 2026-09-01, and it remains the only
`napstr-playlist` event seen on the network. This draft differs from it in a few
places. Each difference is listed here so a reviewer can trace the reasoning, and
none of them invalidate that event, because consumers MUST ignore unknown
properties and MUST NOT reject an event for carrying extras.

| aspect | deployed 2026-09-01 | this draft | reason |
| --- | --- | --- | --- |
| member fields | `filename`, `format`, `mime`, `size`, plus `totalSize` | `position`, `fileId`, optional hints | filename and size arrive in the transfer `WELCOME` frame; format, MIME, and extension agreement are validated where the file is claimed |
| display fields | carried as plain values | explicitly hints, catalogue entry wins | one authority for a file's metadata |
| `totalSize` | present | absent | without per-member `size` it could only restate hints, and would be an unverifiable claim |
| marker | `napstr-playlist` | unchanged | already deployed and unambiguous |
| `d` | UUID version 4 | any canonical UUID | no reason to forbid the time-ordered v7 |
| `alt` | `Napstr public playlist` | unchanged | see below |
| album enrichment | absent | optional `artist` and `mbid` | provisional and additive |
| `client` | absent | optional | provenance hint |

The `alt` string deliberately **keeps the word `public`** from the deployed event.
It is redundant from Napstr's own perspective — this kind is public by
construction, and a private tier exists but is local state that never reaches a
relay — but `alt` is exactly the field that a client which does not implement this
kind will display to a human, and in this kind most neighbouring traffic is NIP-44
ciphertext. `Napstr public playlist` tells such a reader both what the event is
and that its content is readable text, which `Napstr playlist` does not.

## Rationale notes (non-normative)

- **Why a stable UUID `d` and not a content hash:** the identity of a playlist is
  what its author calls it, and it must survive edits. A content hash makes every
  edit a new object, which is correct for a fixed audiobook edition and wrong for
  a working mix.
- **Why no content hash beside the UUID:** the signature already commits to the
  member list, so a hash field would add no integrity. It would help only with
  collapsing two playlists that contain identical tracks, which is a display
  nicety that does not justify a second identity on the event.
- **Why members are file IDs and not catalogue event IDs:** a file ID names the
  bytes and therefore all of their seeders. A catalogue event ID names one
  author's copy, which would tie a playlist to a specific seeder and break the
  curation model where any holder can serve the file.
- **Why the playlist does not carry size or format:** those are properties of the
  file, validated where the file is claimed and transferred, and duplicating them
  creates a second copy that goes stale without any mechanism to update it.
- **Why hints are worth keeping anyway:** a playlist is the one object that
  outlives the files it names. Hints are the only thing that lets a user
  recognise a broken entry instead of seeing a bare hash.
- **Why curation semantics matter:** requiring a playlist author to seed every
  member would make playlists useless as recommendations, which is their main
  value on a network where any holder can serve any file.
- **Why `mbid` is optional and provisional:** the string cover key is derived
  from display metadata and always present, so it is the reliable join; an MBID
  is sparse in practice and self-asserted, so it can only enrich a match that the
  key already found. Keeping it optional means a playlist is never invalidated by
  a missing or wrong MBID, and the field can be dropped later at no cost if it
  turns out not to carry its weight.
