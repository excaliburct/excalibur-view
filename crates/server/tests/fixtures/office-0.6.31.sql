-- The Office Server's database as Excalibur View 0.6.31 made it: the SCHEMA
-- in crates/server/src/store.rs at commit aba11b3 (0.6.31), copied as it was,
-- and the columns its upgrade() added. Read by upgraded_from_before_devices.rs,
-- which opens it with this version, as an office's server does when it updates.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS people (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT NOT NULL UNIQUE,
    password    TEXT NOT NULL,
    role        TEXT NOT NULL,
    created     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token       TEXT PRIMARY KEY,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    issued      TEXT NOT NULL,
    expires     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    number      TEXT NOT NULL,
    name        TEXT NOT NULL,
    created     TEXT NOT NULL
);

-- Who is on a project, when a project has been given a list at all.
--
-- The rule this table exists to make possible: **a project with nobody on it
-- is everybody's**. One shop is one room until somebody says otherwise, which
-- is how every server has worked until now and how most shops want it. The
-- moment one name goes on a project, that project is for the people named on
-- it, and it stops being listed for anybody else.
--
-- Administrators see everything regardless. Somebody has to be able to find a
-- job that the only person on it left the company over.
CREATE TABLE IF NOT EXISTS project_people (
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    added       TEXT NOT NULL,
    added_by    TEXT NOT NULL,
    PRIMARY KEY (project, person)
);

CREATE TABLE IF NOT EXISTS sets (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    filename    TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    sheets      INTEGER NOT NULL,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL,
    revision    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sheets (
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    page        INTEGER NOT NULL,
    number      TEXT NOT NULL,
    title       TEXT NOT NULL,
    width       REAL NOT NULL,
    height      REAL NOT NULL,
    rotation    INTEGER NOT NULL,
    scale       TEXT,
    PRIMARY KEY (set_id, page)
);

-- A markup is the annotation dictionary. Everything beside it is derived, and
-- is here so an integration can filter and total without parsing PDF.
CREATE TABLE IF NOT EXISTS markups (
    id          TEXT PRIMARY KEY,
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    page        INTEGER NOT NULL,
    subject     TEXT NOT NULL,
    kind        TEXT NOT NULL,
    caption     TEXT NOT NULL,
    author      TEXT NOT NULL,
    colour      TEXT NOT NULL,
    created     TEXT NOT NULL,
    dictionary  BLOB NOT NULL,
    revision    INTEGER NOT NULL,
    removed     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS markups_by_revision ON markups(set_id, revision);

CREATE TABLE IF NOT EXISTS chests (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    filename    TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    tools       INTEGER NOT NULL,
    sets        INTEGER NOT NULL,
    uploaded    TEXT NOT NULL,
    shared      INTEGER NOT NULL DEFAULT 1
);

-- Plugins the office hands every seat, one row per plugin id: adding a newer
-- version of one replaces the older. The file itself is a blob.
CREATE TABLE IF NOT EXISTS plugins (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    key         TEXT NOT NULL,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL
);

-- Where to tell another program that a drawing set's takeoff changed, and
-- the secret its notices are signed with. See notices.rs.
CREATE TABLE IF NOT EXISTS notices (
    id          TEXT PRIMARY KEY,
    url         TEXT NOT NULL,
    secret      TEXT NOT NULL,
    created     TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    last_status TEXT,
    last_at     TEXT
);

CREATE TABLE IF NOT EXISTS releases (
    version     TEXT NOT NULL,
    channel     TEXT NOT NULL,
    platform    TEXT NOT NULL,
    published   TEXT NOT NULL,
    notes       TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    digest      TEXT NOT NULL,
    signature   TEXT NOT NULL,
    signing_key TEXT NOT NULL,
    minimum_api INTEGER NOT NULL DEFAULT 1,
    -- A release nobody has approved is not offered to anybody. One seat tries
    -- it first; the other five carry on working.
    ready       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (version, platform)
);

CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
);

-- What happened on this server, and who did it. Kept because an office
-- handling Controlled Unclassified Information is asked, by an auditor, to
-- show exactly this: who reached which drawing, and when.
--
-- Append only. There is no route in the program that edits or deletes a row,
-- which is the property that makes it worth anything. Whoever administers the
-- machine can of course reach the database - that is true of every log there
-- has ever been, and is why an auditor asks who administers the machine.
--
-- `person` is kept as the id and the email as it read at the time, so a row
-- still says who did it after somebody leaves and their account is removed.
-- An invitation: one code, for one person, used once.
--
-- The join code is one server-wide secret that names nobody, never expires
-- and is not used up. An invitation is the opposite of all four: it names who
-- it is for, it carries the role they will get, it stops working on a date,
-- and using it kills it.
--
-- No email is sent, and none ever will be. Sending mail means an SMTP server,
-- a sending reputation, a bounce nobody sees and a support call when a shop's
-- spam filter eats it. An administrator already has a way to reach the person
-- they are inviting -- they work together. What they need is something to
-- send, not something to send it with.
CREATE TABLE IF NOT EXISTS invites (
    code        TEXT PRIMARY KEY,
    email       TEXT NOT NULL,
    name        TEXT NOT NULL,
    role        TEXT NOT NULL,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL,
    expires     TEXT NOT NULL,
    used        TEXT,
    used_by     TEXT
);

CREATE TABLE IF NOT EXISTS audit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          TEXT NOT NULL,
    person      TEXT,
    email       TEXT NOT NULL,
    action      TEXT NOT NULL,
    subject     TEXT NOT NULL DEFAULT '',
    detail      TEXT NOT NULL DEFAULT '',
    address     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS audit_at ON audit (at);
CREATE INDEX IF NOT EXISTS audit_person ON audit (person, at);

-- Groups of people an administrator makes ("Shop", "Field"), and who is in
-- each. A person can be in several. A folder given to groups is seen by the
-- people in them and by the office. See folders.rs.
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS group_people (
    grp         TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    added       TEXT NOT NULL,
    added_by    TEXT NOT NULL,
    PRIMARY KEY (grp, person)
);

-- A project's folders, written down so an empty one shows, and so each can
-- say who sees it. `key` is the folder as compared (case and spacing aside),
-- `path` as it was first written. `see` is its own rule, NULL when it takes
-- its parent's; a folder with no rule anywhere above it is everybody's, as
-- every folder was before folders had rules. Never removed.
CREATE TABLE IF NOT EXISTS folders (
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    path        TEXT NOT NULL,
    see         TEXT,
    set_in      TEXT,
    set_by      TEXT,
    set_at      TEXT,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL,
    PRIMARY KEY (project, key)
);

-- Files in a project's folders that are not drawing sets: Word, Excel,
-- pictures, models. The bytes are kept once, by their digest, as a drawing's
-- are. One saved again under the same name in the same folder replaces the
-- one before, which is kept as its history.
CREATE TABLE IF NOT EXISTS files (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    folder      TEXT,
    name        TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    modified    TEXT,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL,
    replaces    TEXT,
    replaced_by TEXT
);
CREATE INDEX IF NOT EXISTS files_by_project ON files(project);

-- Keys for programs rather than people: an assistant that reads the takeoffs
-- (MCP only, read-only), or another system such as FabWire that uses the API
-- as the person who made the key. Stored as a digest, shown once, never
-- expiring on their own, taken back one at a time.
CREATE TABLE IF NOT EXISTS keys (
    id          TEXT PRIMARY KEY,
    digest      TEXT NOT NULL UNIQUE,
    person      TEXT NOT NULL REFERENCES people(id),
    purpose     TEXT NOT NULL,
    name        TEXT NOT NULL,
    made        TEXT NOT NULL,
    last_used   TEXT
);

-- Sign-offs: signatures a program asks for on a drawing set (a traveler's
-- stations, a shipping ticket's receiver), signed by a person on View. A
-- signature is final: the row is written once more, when it is signed, and
-- never again. `seq` orders every change, so the asker reads back only what
-- changed since it last looked.
CREATE TABLE IF NOT EXISTS signoffs (
    id          TEXT PRIMARY KEY,
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    label       TEXT NOT NULL,
    grp         TEXT NOT NULL DEFAULT '',
    ref         TEXT NOT NULL,
    page        INTEGER,
    asked_by    TEXT NOT NULL,
    asked       TEXT NOT NULL,
    signer      TEXT,
    signed_by   TEXT,
    signed      TEXT,
    strokes     TEXT,
    withdrawn   INTEGER NOT NULL DEFAULT 0,
    seq         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS signoffs_by_set ON signoffs(set_id);
CREATE INDEX IF NOT EXISTS signoffs_by_seq ON signoffs(seq);

-- What 0.6.31's upgrade() added to every database it opened.
ALTER TABLE projects ADD COLUMN reference TEXT;
ALTER TABLE projects ADD COLUMN archived TEXT;
ALTER TABLE sets ADD COLUMN supersedes TEXT;
ALTER TABLE sets ADD COLUMN superseded_by TEXT;
ALTER TABLE markups ADD COLUMN carried_from TEXT;
ALTER TABLE projects ADD COLUMN place TEXT;
ALTER TABLE sets ADD COLUMN folder TEXT;
ALTER TABLE keys ADD COLUMN expires TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS projects_by_reference ON projects(reference) WHERE reference IS NOT NULL;
CREATE INDEX IF NOT EXISTS sets_by_project ON sets(project);
