//! The published description of this server.
//!
//! Written out rather than generated from annotations, because the point of it
//! is that somebody integrating against this server can read it, and a
//! generated document reads like a generated document. A test checks that it
//! describes every route the server actually serves and no route it does not,
//! so honest and readable do not have to be a trade.

use serde_json::{json, Value};

pub fn document(name: &str) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": format!("{name} — Excalibur View API"),
            "version": "1.0.0",
            "summary": "Drawings, markups and takeoffs from a self-hosted Excalibur View server.",
            "description": DESCRIPTION,
        },
        "servers": [{ "url": "/api/v1" }],
        "security": [{ "bearer": [] }],
        "components": {
            "securitySchemes": {
                "bearer": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "A token from POST /auth/token."
                }
            },
            "schemas": schemas(),
        },
        "paths": paths(),
    })
}

const DESCRIPTION: &str = "\
Everything an Excalibur View desktop can do, any other program can do too.

Two things worth knowing before you integrate against this.

**The PDF is the file of record.** A markup on this API is the PDF annotation \
dictionary itself, base64'd — not a summary of one. Anything you read here can \
be written back into the drawing byte for byte, and anything you write here \
lands in the drawing as a real annotation that Bluebeam Revu, Acrobat and any \
other reader will show.

**Nothing is estimated, and you must check for that.** Measuring needs a scale, \
and a sheet does not always have one. A measurement from a sheet with no scale \
comes back with `scaled: false` and every measured field absent — not zero — \
and it is counted in `left_out` and excluded from every total. If `left_out` is \
above zero the totals are short, and `sheets_without_a_scale` names the sheets \
somebody needs to look at. A caller that ignores this will quietly under-report \
a bid.

Weights come from the unit weight carried by the tool that drew the markup, out \
of the tool chest the estimator was using. They are that company's numbers. \
This server does not look anything up in a section table.";

fn paths() -> Value {
    json!({
        "/health": {
            "get": {
                "summary": "Is this an Excalibur View server, and which version does it speak?",
                "security": [],
                "responses": ok("Health")
            }
        },
        "/openapi.json": {
            "get": {
                "summary": "This document.",
                "security": [],
                "responses": { "200": { "description": "The API description." } }
            }
        },
        "/auth/token": {
            "post": {
                "summary": "Sign in.",
                "security": [],
                "requestBody": body("SignIn"),
                "responses": {
                    "200": schema_response("A session.", "Session"),
                    "401": schema_response("That email address and password do not match.", "Problem")
                }
            }
        },
        "/setup": {
            "post": {
                "summary": "Claim a server nobody has claimed yet.",
                "description": "Works exactly once, on a server with no accounts on it. \
The person who calls it becomes its administrator and is signed in straight away, and the \
reply carries the join code to hand to everybody else. On a server that already belongs to \
somebody this refuses with 409 and changes nothing. GET /health says which kind you are \
looking at before you ask.",
                "security": [],
                "requestBody": body("Setup"),
                "responses": {
                    "200": schema_response("Set up, signed in, and the code for everybody else.", "Claimed"),
                    "400": schema_response("Something in the form was not usable.", "Problem"),
                    "409": schema_response("This server already belongs to somebody.", "Problem")
                }
            }
        },
        "/join": {
            "post": {
                "summary": "Make yourself an account.",
                "description": "How the rest of an office gets on without an administrator \
typing their details for them. Needs the server's join code unless the administrator has set \
joining to 'open'. Signs the new account in straight away.",
                "security": [],
                "requestBody": body("Join"),
                "responses": {
                    "200": schema_response("A session for the new account.", "Session"),
                    "400": schema_response("Something in the form was not usable.", "Problem"),
                    "403": schema_response("Wrong code, or this server is not taking accounts.", "Problem"),
                    "409": schema_response("That email address already has an account.", "Problem")
                }
            }
        },
        "/admin/joining": {
            "get": {
                "summary": "How people are getting accounts, and the current code.",
                "responses": ok("Joining")
            },
            "post": {
                "summary": "Change how people get accounts, or issue a new code.",
                "description": "Issuing a new code stops the old one working, which is what \
to do when somebody leaves.",
                "requestBody": body("ChangeJoining"),
                "responses": ok("Joining")
            }
        },
        "/me": {
            "get": {
                "summary": "Who this token belongs to.",
                "description": "With a key, also which key it is and what it was made for, so a \
program can refuse to run on a key with more power than it needs. `may_administer` is true \
only for an administrator signed in with their password: no key ever administers.",
                "responses": ok("Me")
            }
        },
        "/me/password": {
            "post": {
                "summary": "Change your own password.",
                "description": "Needs the current one, and a person signed in: a key is \
refused (403 key_not_allowed). Wrong tries count against the same limit as signing in (429 \
too_many). Every other place you are signed in is signed out.",
                "requestBody": body("ChangePassword"),
                "responses": ok("Changed")
            }
        },
        "/signout": {
            "post": {
                "summary": "End this session on the server.",
                "description": "Not just on this computer: the token stops working the moment \
                                this returns. A key is not a session -- take one back with \
                                /keys/{id}/revoke.",
                "responses": ok("SignedOut")
            }
        },
        "/me/keys": {
            "get": {
                "summary": "Keys for programs: your own, or every one for an administrator.",
                "description": "Never the keys themselves: those are shown once, when made.",
                "responses": ok_list("ApiKey")
            },
            "post": {
                "summary": "Make a key for a program.",
                "description": "`assistant` reads the takeoffs over MCP at `/mcp` and nothing \
else. `integration` uses this API as you — for another system — and only an \
administrator can make one. `sync` is narrower still, for a program that keeps its \
own projects in step: `GET /me`, `POST /projects` (under a reference), \
`POST /projects/{id}/archive`, `POST /sets`, `GET /projects/{id}/stamp` and \
`GET /projects/{id}/markuplist`, on projects filed under a reference, and nothing \
else; only an administrator can make one. No key of any purpose can administer the \
server, whoever made it. `expires_in_days` gives a key an end (1 to 3650); left out, \
it never runs out. Send it as `Authorization: Bearer <key>`. \
The key is in this answer and never again. Asking again with the same name and \
purpose replaces the old key. A key cannot make a key.",
                "requestBody": body("NewKey"),
                "responses": ok("ApiKey")
            }
        },
        "/keys/{id}/revoke": {
            "post": {
                "summary": "Take a key back. Whatever was using it stops working at once.",
                "parameters": [path_param("id", "The key's id, from the list.")],
                "responses": ok("Changed")
            }
        },
        "/sets/{id}/markuplist": {
            "get": {
                "summary": "A set's markups, shaped like Bluebeam's markup list.",
                "description": "`Markups`, each with `Subject`, `Label`, `Comment` (the \
measurement as Revu writes it, e.g. 24'-6\"), `PageLabel`, `Length`, `Area`, `Count`, \
`Pounds` and the tool chest's own columns by name under `ExtendedProperties` \
(\"LBS Per FT\" and so on, named from the office's shared chest). Then `Groups` \
and `Totals`, and `Warning` whenever measurements were left out for want of a \
scale — in which case every total is short.",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": { "200": { "description": "The markup list." } }
            }
        },
        "/projects/{id}/markuplist": {
            "get": {
                "summary": "Every set in a project, the same way.",
                "description": "`{ \"results\": [ { \"file\", \"set\", \"markups\": { \"Markups\": [...] } } ] }`: \
a whole bid's takeoff in one answer. Each markup also carries `Hyperview`: its id, kind, \
`Each` (the exact count, quantity included), `LengthFt`, `AreaSqFt`, and whether it was \
carried forward from an earlier issue and not yet checked. A drawing issued again is read \
from its newest issue; the ones it replaced are listed under `superseded` with how many \
markups they still hold. `stamp` is the same as `GET /projects/{id}/stamp`.",
                "parameters": [path_param("id", "The project."),
                               query_param("all", "1 also reads the issues that were replaced.")],
                "responses": { "200": { "description": "One markup list per drawing set." } }
            }
        },
        "/projects/{id}": {
            "get": {
                "summary": "One project.",
                "parameters": [path_param("id", "The project.")],
                "responses": ok("Project")
            }
        },
        "/projects/{id}/archive": {
            "post": {
                "summary": "Put a project away, or bring it back.",
                "description": "Archived projects are left off `GET /projects` and change in no \
other way: every drawing, markup and quantity stays. `{\"archived\": false}` brings one back.",
                "parameters": [path_param("id", "The project.")],
                "requestBody": body("Archiving"),
                "responses": ok("Project")
            }
        },
        "/projects/{id}/place": {
            "post": {
                "summary": "Move a project in the Studio's list.",
                "description": "A place is folders written with `/`: `Bids/2026`, `Active Jobs`, \
`Past Jobs/2025`. The Studio shows projects on those shelves, as an office keeps its files; a \
program moves one there as a bid is won or lost and a job finishes. Empty or null puts it back \
with the other projects. Nothing else about it changes.",
                "parameters": [path_param("id", "The project.")],
                "requestBody": body("Placing"),
                "responses": ok("Project")
            }
        },
        "/projects/{id}/stamp": {
            "get": {
                "summary": "Whether anything has changed, without reading it.",
                "description": "`stamp` changes whenever the takeoff could have: a markup drawn, \
moved or deleted, a drawing added or issued again. Compare it with the one from the last pull.",
                "parameters": [path_param("id", "The project.")],
                "responses": ok("Stamp")
            }
        },
        "/admin/fleet": {
            "get": {
                "summary": "Whether Excalibur Fleet may watch this server's health.",
                "responses": ok("FleetAccess")
            },
            "post": {
                "summary": "Let Excalibur Fleet watch this server, or stop it.",
                "description": "Turning it on makes a new key and returns it once; only its \
digest is kept. The Fleet can read health and the log and deliver releases signed by the \
publisher's key. It cannot read drawings.",
                "requestBody": body("ChangeFleet"),
                "responses": ok("FleetAccess")
            }
        },
        "/admin/updates": {
            "get": {
                "summary": "How this server keeps itself and its seats up to date.",
                "responses": ok("UpdateSettings")
            },
            "post": {
                "summary": "Take new versions as they are released, or early.",
                "description": "`early` gets versions the publisher is still trying, before \
anybody else does.",
                "requestBody": body("ChangeUpdates"),
                "responses": ok("UpdateSettings")
            }
        },
        "/projects": {
            "get": {
                "summary": "Every project that is not archived.",
                "parameters": [query_param("all", "1 includes archived projects.")],
                "responses": ok_list("Project")
            },
            "post": {
                "summary": "Start a project.",
                "description": "With a `reference` that another project already has, answers \
with that project instead of making a second — brought back if it was archived. A program \
that files its jobs here by its own id can push as often as it likes. Filing under a \
`reference` takes an administrator (signed in, or a key they made) or a sync key; anybody \
else is refused (403) whether or not the reference exists.",
                "requestBody": body("NewProject"),
                "responses": ok("Project")
            }
        },
        "/projects/{id}/sets": {
            "get": {
                "summary": "The drawing sets in a project.",
                "description": "Sets replaced by a newer issue are left off unless `all=1`.",
                "parameters": [path_param("id", "The project."),
                               query_param("all", "1 includes sets replaced by a newer issue.")],
                "responses": ok_list("DrawingSet")
            }
        },
        "/sets": {
            "post": {
                "summary": "Upload a drawing set.",
                "description": "Multipart: `project`, `name`, optionally `folder`, and the PDF as \
`file`. The sheets are read out of it on the way in. Bytes the project already holds answer \
with the set that holds them (`already_here`). A drawing with the same name as a current set \
in the same folder supersedes it; with `carry_forward=1` that set's markups are brought \
across, sheet for sheet, marked `Carried forward — check`. The same name in another folder is \
another drawing.",
                "requestBody": {
                    "required": true,
                    "content": {
                        "multipart/form-data": {
                            "schema": {
                                "type": "object",
                                "required": ["project", "file"],
                                "properties": {
                                    "project": { "type": "string" },
                                    "name": { "type": "string" },
                                    "carry_forward": { "type": "string", "description": "1 brings the markups on the issue this replaces across." },
                                    "folder": { "type": "string", "description": "The folder it came from in the job's own files, as folders written with `/`: 10 SHOP DRAWINGS/10.4 FOR FABRICATION." },
                                    "file": { "type": "string", "format": "binary" }
                                }
                            }
                        }
                    }
                },
                "responses": ok("DrawingSet")
            }
        },
        "/sets/{id}": {
            "get": {
                "summary": "One drawing set.",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": ok("DrawingSet")
            }
        },
        "/sets/{id}/file": {
            "get": {
                "summary": "The PDF itself.",
                "description": "Send `If-None-Match` with the digest you already have and \
                                you will get a 304 instead of sixty megabytes.",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": {
                    "200": { "description": "The drawing.",
                             "content": { "application/pdf": { "schema": { "type": "string", "format": "binary" } } } },
                    "304": { "description": "You already have this exact file." }
                }
            }
        },
        "/sets/{id}/sheets": {
            "get": {
                "summary": "The sheets, their sizes and their scales.",
                "description": "A sheet whose `scale` is null has no scale set. \
                                Measurements on it are reported but never totalled.",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": ok_list("Sheet")
            }
        },
        "/sets/{id}/markups": {
            "get": {
                "summary": "Markups, since a revision you have already seen.",
                "description": "Removals travel as records with `removed: true`, so a client \
                                syncing from an old revision learns what went away.",
                "parameters": [
                    path_param("id", "The drawing set."),
                    query_param("since", "The revision you last saw. Omit for everything.")
                ],
                "responses": ok("MarkupPage")
            },
            "post": {
                "summary": "Add markups.",
                "description": "Each one is a PDF annotation dictionary, base64. They are all \
                                checked before any are written, so a bad one in a batch leaves \
                                nothing half-saved.",
                "parameters": [path_param("id", "The drawing set.")],
                "requestBody": {
                    "required": true,
                    "content": { "application/json": {
                        "schema": { "type": "array", "items": { "$ref": "#/components/schemas/NewMarkup" } }
                    } }
                },
                "responses": ok("MarkupPage")
            }
        },
        "/sets/{id}/takeoff": {
            "get": {
                "summary": "The quantities, measured and totalled.",
                "description": "The same engine the viewer uses, on the same annotations, so \
                                this and somebody's screen cannot disagree. **Read `left_out` \
                                before you trust `tons`.**",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": ok("Takeoff")
            }
        },
        "/sets/{id}/takeoff.csv": {
            "get": {
                "summary": "The same, as a spreadsheet.",
                "parameters": [path_param("id", "The drawing set.")],
                "responses": { "200": { "description": "One line per markup, then the totals, \
                                                        then a note of anything left out.",
                    "content": { "text/csv": { "schema": { "type": "string" } } } } }
            }
        },
        "/people": {
            "get": {
                "summary": "Everyone with a seat. Administrators only.",
                "responses": ok_list("User")
            },
            "post": {
                "summary": "Give somebody a seat. Administrators only.",
                "description": "A password shorter than twelve characters is refused. Four \
                                unrelated words beats a short one with symbols in it.",
                "requestBody": body("NewPerson"),
                "responses": ok("User")
            }
        },
        "/people/{id}/role": {
            "post": {
                "summary": "Change what somebody may do. Administrators only.",
                "description": "Takes effect on their very next request: the role is read out \
                                of the account on every call rather than kept in the session. \
                                Taking the badge off the only administrator is refused, because \
                                there is no way back from a server nobody can administer.",
                "parameters": [path_param("id", "The person.")],
                "requestBody": body("NewRole"),
                "responses": ok("User")
            }
        },
        "/people/{id}/remove": {
            "post": {
                "summary": "Take somebody's account away. Administrators only.",
                "description": "Their sessions and their keys go with them, at once. What they \
                                made stays: a markup carries its author's name and a drawing set \
                                carries who uploaded it, as text, so a takeoff does not lose its \
                                history because somebody left. You cannot remove your own \
                                account, and the last administrator cannot be removed.",
                "parameters": [path_param("id", "The person.")],
                "responses": ok("Removed")
            }
        },
        "/chests": {
            "get": { "summary": "The shared tool chests.", "responses": ok_list("Chest") },
            "post": {
                "summary": "Upload a tool chest for the whole office. Administrators only.",
                "description": "Multipart: `name` and a Revu profile or tool set as `file`. \
                                The tools are counted on the way in, and a file no tools can \
                                be read out of is refused.",
                "requestBody": upload("A .bpx or .btx."),
                "responses": ok("Chest")
            }
        },
        "/plugins": {
            "get": { "summary": "The plugins every seat in the office is handed.", "responses": ok_list("Plugin") },
            "post": {
                "summary": "Hand a plugin to the whole office. Administrators only.",
                "description": "The body is a signed `.hvplugin` file. The server checks the \
                                signature against the keys a plugin may be signed by and refuses \
                                anything else, and every seat checks it again before loading it. \
                                A plugin with the same id replaces the one the office had.",
                "requestBody": { "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } },
                "responses": ok("Plugin")
            }
        },
        "/plugins/{id}/file": {
            "get": {
                "summary": "A plugin file, as it was signed.",
                "parameters": [path_param("id", "The plugin's id.")],
                "responses": { "200": { "description": "The .hvplugin file." } }
            }
        },
        "/plugins/{id}/run": {
            "post": {
                "summary": "Run a plugin on the server, for a seat that can't run one itself.",
                "description": "For the Mac App Store copy, which isn't allowed to run code it \
                                downloads. The body is the plugin's input as a seat would hand it \
                                over: `plugin_api::pack`, or the same thing as plain JSON. The \
                                signature is checked again, the plugin runs in the same sandbox \
                                and limits it would have on a seat, and the answer is the \
                                plugin's own: `done` with its findings, or `failed` with its \
                                reason. Nothing is kept.",
                "parameters": [path_param("id", "The plugin's id.")],
                "requestBody": { "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } },
                "responses": { "200": { "description": "The plugin's answer." } }
            }
        },
        "/plugins/{id}/remove": {
            "post": {
                "summary": "Stop handing a plugin out. Administrators only.",
                "description": "Seats drop it the next time they ask what the office has.",
                "parameters": [path_param("id", "The plugin's id.")],
                "responses": { "200": { "description": "Removed." } }
            }
        },
        "/notices": {
            "get": {
                "summary": "Where the office tells other programs a takeoff changed. Administrators only.",
                "responses": ok_list("ChangeNotice")
            },
            "post": {
                "summary": "Tell another program whenever a drawing set's markups change. Administrators only.",
                "description": "The body is `{\"url\": \"https://…\"}`. From then on, a couple of \
                                seconds after any drawing set's markups change, the server POSTs a \
                                small JSON body there: `event` (`markups.changed`), `set`, `file`, \
                                `revision`, `project`, `number`, `reference` (the other program's \
                                own reference for the job, when it filed one), `server` and `at`. \
                                No drawing or markup travels with it; ask the API for what you \
                                want. Several changes to one set close together are one notice. \
                                Each is signed: `X-Excalibur-Signature: sha256=<hex HMAC-SHA256 of \
                                the body, keyed with the secret>`, and `X-Excalibur-Event` names \
                                the event. The secret is in this answer and never shown again. A \
                                notice that isn't taken is tried three times, then dropped. A \
                                sealed server sends notices only inside the company's network.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object", "properties": { "url": { "type": "string" } } } } } },
                "responses": ok("ChangeNotice")
            }
        },
        "/notices/{id}/test": {
            "post": {
                "summary": "Send a `ping` notice now, and say how it went. Administrators only.",
                "parameters": [path_param("id", "The notice address's id.")],
                "responses": { "200": { "description": "`{\"status\": \"delivered\"}`, or what went wrong." } }
            }
        },
        "/notices/{id}/remove": {
            "post": {
                "summary": "Stop telling that address. Administrators only.",
                "parameters": [path_param("id", "The notice address's id.")],
                "responses": { "200": { "description": "Removed." } }
            }
        },
        "/license": {
            "get": {
                "summary": "Where this office stands with Excalibur View Office.",
                "description": "founding, licensed, trial or unlicensed, with users, the date \
                                updates run through and one sentence to show. Anybody signed in \
                                may ask; an administrator signed in (never a key) is also told the license id. With no \
                                license after the trial, reading and exporting carry on and \
                                writes answer 403 license_needed.",
                "responses": ok("Standing")
            },
            "post": {
                "summary": "Add a license file. Administrators only.",
                "description": "The whole text of the .evlicense file. It is checked on this \
                                server against the keys built into it; nothing is sent anywhere.",
                "requestBody": body("NewLicense"),
                "responses": ok("Standing")
            }
        },
        "/sets/{id}/copy": {
            "post": {
                "summary": "A copy of this drawing set to take off on.",
                "description": "An estimator marks up a copy, not the set everybody else is \
                                reading. Costs no bytes: the file is kept by its SHA-256, so the \
                                copy points at the same one. Sheet numbers, titles and the scales \
                                somebody already calibrated come across; markups do not. The copy \
                                supersedes nothing and is superseded by nothing — it is a working \
                                copy, not another issue of the drawing. Name it, or leave the name \
                                out for \"<the set> — Takeoff\".",
                "parameters": [path_param("id", "The drawing set to copy.")],
                "requestBody": body("CopyAsked"),
                "responses": ok("DrawingSet")
            }
        },
        "/audit": {
            "get": {
                "summary": "Who did what on this server, newest first. Administrators only.",
                "description": "Signing in and failing to, reaching and uploading drawing sets, \
                                exporting takeoffs, adding and removing people, licenses and keys. \
                                Append only: nothing in the program edits or deletes a line. The \
                                log stays on this server and is never sent anywhere. `since` is an \
                                RFC 3339 time; `limit` defaults to 500 and is capped at 50,000.",
                "parameters": [
                    query_param("since", "Only what happened at or after this RFC 3339 time."),
                    query_param("limit", "At most this many lines. Default 500, most 50,000.")
                ],
                "responses": ok_list("AuditEntry")
            }
        },
        "/audit.csv": {
            "get": {
                "summary": "The same log as a CSV, which is what an auditor asks to be sent.",
                "description": "Administrators only. Same `since` and `limit`; `limit` here \
                                defaults to everything.",
                "parameters": [
                    query_param("since", "Only what happened at or after this RFC 3339 time."),
                    query_param("limit", "At most this many lines.")
                ],
                "responses": { "200": { "description": "text/csv" } }
            }
        },
        "/admin/pin": {
            "post": {
                "summary": "Hold every seat on one version, or let them move again.",
                "description": "An empty version unpins. A fabricator in the middle of a bid \
                                package should not have the tool change under them.",
                "requestBody": body("Pin"),
                "responses": ok("UpdateOffer")
            }
        },
        "/admin/releases": {
            "post": {
                "summary": "Publish an installer. Administrators only.",
                "description": "Multipart: the signed `manifest` and the installer as `file`. \
                                The server holds no signing key and signs nothing. It checks \
                                that the file matches its manifest and refuses an unsigned \
                                release; every seat then checks the signature for itself \
                                against a key compiled into its own binary. A release arrives \
                                unoffered — mark it ready when one seat has tried it.",
                "requestBody": upload("The installer, with its signed manifest."),
                "responses": ok("Release")
            }
        },
        "/admin/releases/{version}/ready": {
            "post": {
                "summary": "Let the office have a release, or take it back.",
                "parameters": [path_param("version", "The version.")],
                "requestBody": body("Ready"),
                "responses": { "200": { "description": "Done." } }
            }
        },
        "/chests/{id}/file": {
            "get": {
                "summary": "A tool chest, as its original .bpx.",
                "parameters": [path_param("id", "The tool chest.")],
                "responses": { "200": { "description": "The tool chest file." } }
            }
        },
        "/update/latest": {
            "get": {
                "summary": "Is there a newer version for this seat?",
                "description": "An empty `release` means there is nothing newer, or an \
                                administrator has pinned this office to what it is already \
                                running. Nothing installs that is not signed by a key the \
                                running program already trusts.",
                "parameters": [
                    query_param("fresh", "true to have the server look at the publisher's feed \
                                          first, waiting up to 75 seconds. `looking` in the \
                                          answer means it was still looking."),
                    query_param("version", "What this seat is running."),
                    query_param("channel", "stable or preview."),
                    query_param("platform", "windows-x64, linux-x64.")
                ],
                "responses": ok("UpdateOffer")
            }
        },
        "/update/{version}/download": {
            "get": {
                "summary": "An installer.",
                "description": "Check its digest and its signature before running it. \
                                The server serving it is not what makes it safe.",
                "parameters": [
                    path_param("version", "The version."),
                    query_param("platform", "windows-x64, linux-x64.")
                ],
                "responses": { "200": { "description": "The installer." } }
            }
        }
    })
}

fn ok(schema: &str) -> Value {
    json!({ "200": schema_response("Done.", schema) })
}

fn ok_list(schema: &str) -> Value {
    json!({ "200": { "description": "Done.", "content": { "application/json": {
        "schema": { "type": "array", "items": { "$ref": format!("#/components/schemas/{schema}") } }
    } } } })
}

fn schema_response(description: &str, schema: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": {
            "schema": { "$ref": format!("#/components/schemas/{schema}") }
        } }
    })
}

fn upload(description: &str) -> Value {
    json!({
        "required": true,
        "description": description,
        "content": { "multipart/form-data": { "schema": { "type": "object" } } }
    })
}

fn body(schema: &str) -> Value {
    json!({
        "required": true,
        "content": { "application/json": {
            "schema": { "$ref": format!("#/components/schemas/{schema}") }
        } }
    })
}

fn path_param(name: &str, description: &str) -> Value {
    json!({ "name": name, "in": "path", "required": true,
            "description": description, "schema": { "type": "string" } })
}

fn query_param(name: &str, description: &str) -> Value {
    json!({ "name": name, "in": "query", "required": false,
            "description": description, "schema": { "type": "string" } })
}

fn schemas() -> Value {
    json!({
        "Problem": object(&[
            ("error", "string", "A stable short name: not_found, unauthorised."),
            ("message", "string", "A sentence somebody can act on."),
        ]),
        "Health": object(&[
            ("ok", "boolean", ""),
            ("api_version", "integer", "The wire version this server speaks."),
            ("version", "string", "The server's own build."),
            ("name", "string", "What the company calls this installation."),
            ("claimed", "boolean", "False only on a server nobody has set up yet. Absent on an older server, which means claimed."),
            ("joining", "string", "How somebody without an account gets one: code, open or closed."),
        ]),
        "SignIn": object(&[("email", "string", ""), ("password", "string", "")]),
        "Setup": object(&[
            ("company", "string", "What to call this installation. It shows in every seat's title bar."),
            ("name", "string", "The first administrator's own name."),
            ("email", "string", ""),
            ("password", "string", "Twelve characters at the very least."),
        ]),
        "Claimed": object(&[
            ("company", "string", "What this installation is now called."),
            ("join_code", "string", "Give this to everybody else in the office."),
        ]),
        "Join": object(&[
            ("code", "string", "The server's join code. Ignored when joining is open."),
            ("name", "string", "Your own name. It goes on every markup you make."),
            ("email", "string", ""),
            ("password", "string", "Twelve characters at the very least."),
        ]),
        "Joining": object(&[
            ("how", "string", "code, open or closed."),
            ("code", "string", "Only filled in for an administrator."),
            ("role", "string", "What a new account gets."),
        ]),
        "UpdateSettings": object(&[
            ("running", "string", "This server's own version."),
            ("channel", "string", "stable, or early."),
            ("checked", "string", "When the publisher's feed was last asked."),
            ("note", "string", "What came of it."),
            ("offering", "string", "The newest version offered to the seats."),
        ]),
        "ChangeUpdates": object(&[("channel", "string", "stable or early.")]),
        "ChangePassword": object(&[
            ("current", "string", "The password now."),
            ("new", "string", "Twelve characters at the very least."),
        ]),
        "Changed": object(&[("changed", "boolean", "")]),
        "NewRole": object(&[("role", "string", "viewer, estimator or admin.")]),
        "Removed": object(&[("removed", "string", "The person who is no longer on this server.")]),
        "SignedOut": object(&[("signed_out", "boolean", "")]),
        "ApiKey": object(&[
            ("id", "string", ""),
            ("name", "string", "What it was called when it was made."),
            ("purpose", "string", "assistant, integration or sync."),
            ("person", "string", "Whose it is. It can do what they can, no more, and never administer."),
            ("made", "string", ""),
            ("last_used", "string", ""),
            ("key", "string", "Only in the answer that made it."),
            ("expires", "string", "When it stops working by itself. Absent: never."),
        ]),
        "NewKey": object(&[
            ("purpose", "string", "assistant, integration or sync."),
            ("name", "string", "e.g. the program's name, or the assistant on the estimator's PC."),
            ("expires_in_days", "integer", "Optional: stop working this many days from now (1 to 3650). Left out, never."),
        ]),
        "Me": object(&[
            ("id", "string", ""),
            ("name", "string", "What goes in a markup's Author column."),
            ("email", "string", ""),
            ("role", "string", "viewer, estimator or admin. A sync key made by an administrator says estimator: that is all it acts as."),
            ("may_administer", "boolean", "True only for an administrator signed in with their password. Never for a key."),
            ("key", "object", "The key this request came with: id, name, purpose (assistant, integration or sync) and expires. Absent for a signed-in person."),
        ]),
        "FleetAccess": object(&[
            ("on", "boolean", ""),
            ("key", "string", "Only in the answer that turned it on."),
        ]),
        "ChangeFleet": object(&[("on", "boolean", "")]),
        "ChangeJoining": object(&[
            ("how", "string", "code, open or closed. Leave it out to keep what it is."),
            ("role", "string", "What a new account should get. Never admin."),
            ("new_code", "boolean", "Throw the old code away and issue a new one."),
        ]),
        "User": object(&[
            ("id", "string", ""),
            ("name", "string", "What goes in a markup's Author column."),
            ("email", "string", ""),
            ("role", "string", "viewer, estimator or admin."),
        ]),
        "Session": object(&[
            ("token", "string", "Send as `Authorization: Bearer`."),
            ("expires_in", "integer", "Seconds."),
            ("api_version", "integer", ""),
        ]),
        "Project": object(&[
            ("id", "string", ""),
            ("number", "string", "The job number, as the shop says it."),
            ("name", "string", ""),
            ("created", "string", ""),
            ("sets", "integer", "How many drawing sets, not counting ones replaced by a newer issue."),
            ("reference", "string", "Who this project is in another program: fabwire:bid:2630."),
            ("archived", "string", "When it was put away. Absent while it is in use."),
            ("place", "string", "Where it sits in the Studio's list: Bids/2026, Active Jobs. Absent: with the other projects."),
        ]),
        "NewProject": object(&[
            ("number", "string", ""),
            ("name", "string", ""),
            ("reference", "string", "Optional. The same reference always answers with the same project."),
            ("place", "string", "Optional. Where it goes in the Studio's list; with a reference already filed, it moves there. Left out, it stays where it is."),
        ]),
        "Placing": object(&[("place", "string", "Folders written with /. Empty or null: with the other projects.")]),
        "Archiving": object(&[("archived", "boolean", "False brings it back. True if left out.")]),
        "Stamp": object(&[
            ("project", "string", ""),
            ("stamp", "string", "Changes whenever the takeoff could have."),
            ("sets", "integer", "Sets that count."),
            ("superseded", "integer", "Sets replaced by a newer issue."),
            ("markups", "integer", "Markups on the sets that count."),
            ("carried_forward", "integer", "Of those, brought forward from an earlier issue."),
            ("archived", "string", ""),
        ]),
        "NewPerson": object(&[
            ("name", "string", ""),
            ("email", "string", ""),
            ("password", "string", "At least twelve characters."),
            ("role", "string", "viewer, estimator or admin. Anything else is an ordinary seat."),
        ]),
        "Pin": object(&[("version", "string", "Empty unpins.")]),
        "NewLicense": object(&[("license", "string", "The whole text of the .evlicense file.")]),
        "Standing": object(&[
            ("state", "string", "founding, licensed, trial or unlicensed."),
            ("edition", "string", "office or sealed."),
            ("company", "string", ""),
            ("license_id", "string", "Administrators only."),
            ("users_allowed", "integer", "Absent is no limit."),
            ("users", "integer", "People with accounts on this server."),
            ("updates_through", "string", "YYYY-MM-DD. Absent is forever."),
            ("trial_days_left", "integer", ""),
            ("sharing", "boolean", "Whether new work can be shared through the server now."),
            ("message", "string", "One sentence to show."),
            ("buy_url", "string", "Where to buy or renew."),
        ]),
        "CopyAsked": object(&[
            ("name", "string", "What to call the copy. Leave it out for \"<the set> — Takeoff\"."),
        ]),
        "AuditEntry": object(&[
            ("at", "string", "RFC 3339, in UTC."),
            ("person", "string", "The person's id. Absent when nobody was identified — a sign-in that failed."),
            ("email", "string", "Their email as it read at the time, so the line still says who after the account is gone."),
            ("action", "string", "signed in, sign-in refused, opened a drawing set, downloaded a drawing set, exported a takeoff, uploaded a drawing set, added a person, removed a person, changed what a person may do, changed a password, added a license, made a key for another program, took back a key, changed a server setting."),
            ("subject", "string", "What it was done to: a set's name, a person's email, a license id."),
            ("detail", "string", ""),
            ("address", "string", "Where from, when the server can tell."),
        ]),
        "Ready": object(&[
            ("platform", "string", ""),
            ("ready", "boolean", "True offers it to the office."),
        ]),
        "DrawingSet": object(&[
            ("id", "string", ""),
            ("project", "string", ""),
            ("name", "string", ""),
            ("filename", "string", ""),
            ("digest", "string", "SHA-256 of the PDF. Use it with If-None-Match."),
            ("bytes", "integer", ""),
            ("sheets", "integer", ""),
            ("uploaded", "string", ""),
            ("uploaded_by", "string", ""),
            ("revision", "integer", "Goes up whenever markups change."),
            ("supersedes", "string", "The set this one replaced."),
            ("superseded_by", "string", "The set that replaced this one."),
            ("already_here", "boolean", "The upload was bytes this project already had; this is that set."),
            ("carried_forward", "integer", "Markups brought across from the set this replaced."),
            ("carry_note", "string", "Why some were not."),
            ("folder", "string", "The folder it came from in the job's own files. Absent: the project's own."),
        ]),
        "Sheet": object(&[
            ("page", "integer", "Zero-based, the way the file counts."),
            ("number", "string", "The number in the title block: S-200."),
            ("title", "string", ""),
            ("width", "number", "Points."),
            ("height", "number", "Points."),
            ("rotation", "integer", ""),
            ("scale", "string", "How the sheet reads. Null means no scale is set, which is never the same as a scale of zero."),
        ]),
        "Markup": object(&[
            ("id", "string", ""),
            ("page", "integer", ""),
            ("subject", "string", "The Tool Chest tool that drew it: W12x26."),
            ("kind", "string", "Length, Area, Count, Volume, Markup."),
            ("caption", "string", "What it says on the sheet. Empty when the sheet has no scale."),
            ("author", "string", ""),
            ("created", "string", ""),
            ("colour", "string", ""),
            ("dictionary", "string", "The PDF annotation dictionary, base64. This is the markup."),
            ("revision", "integer", ""),
            ("removed", "boolean", "It has been taken off the drawing."),
        ]),
        "NewMarkup": object(&[
            ("page", "integer", ""),
            ("dictionary", "string", "The PDF annotation dictionary, base64."),
            ("replaces", "string", "Replaces this markup rather than adding one."),
        ]),
        "MarkupPage": json!({
            "type": "object",
            "properties": {
                "markups": { "type": "array", "items": { "$ref": "#/components/schemas/Markup" } },
                "revision": { "type": "integer", "description": "You are now up to date with this." }
            }
        }),
        "TakeoffRow": object(&[
            ("markup", "string", ""),
            ("page", "integer", ""),
            ("sheet", "string", "S-200."),
            ("subject", "string", ""),
            ("kind", "string", ""),
            ("caption", "string", ""),
            ("author", "string", ""),
            ("scaled", "boolean", "False means its sheet has no scale. Every measured field below is then absent, and this row is left out of the totals."),
            ("count", "number", ""),
            ("length_feet", "number", "Absent when unscaled. Never zero to mean unknown."),
            ("area_square_feet", "number", "Absent when unscaled."),
            ("volume_cubic_feet", "number", "Absent when unscaled."),
            ("pounds", "number", "From the unit weight on the tool that drew it. Absent when there is none."),
        ]),
        "TakeoffGroup": object(&[
            ("subject", "string", ""),
            ("kind", "string", ""),
            ("picks", "integer", "How many markups."),
            ("count", "number", ""),
            ("length_feet", "number", ""),
            ("area_square_feet", "number", ""),
            ("volume_cubic_feet", "number", ""),
            ("pounds", "number", "Absent when nothing in the group carried a unit weight."),
            ("left_out", "integer", "Markups in this group with no scale."),
        ]),
        "Takeoff": json!({
            "type": "object",
            "description": "Read left_out before you trust tons.",
            "properties": {
                "set": { "type": "string" },
                "revision": { "type": "integer" },
                "rows": { "type": "array", "items": { "$ref": "#/components/schemas/TakeoffRow" } },
                "groups": { "type": "array", "items": { "$ref": "#/components/schemas/TakeoffGroup" } },
                "picks": { "type": "integer" },
                "pounds": { "type": "number", "description": "Absent when nothing carried a weight." },
                "tons": { "type": "number" },
                "left_out": { "type": "integer", "description": "Measurements excluded from every total above because their sheet has no scale. Above zero means these totals are short." },
                "sheets_without_a_scale": { "type": "array", "items": { "type": "string" } }
            }
        }),
        "Chest": object(&[
            ("id", "string", ""),
            ("name", "string", ""),
            ("filename", "string", ""),
            ("digest", "string", ""),
            ("bytes", "integer", ""),
            ("tools", "integer", ""),
            ("sets", "integer", ""),
            ("uploaded", "string", ""),
            ("shared", "boolean", "Every seat gets this one."),
        ]),
        "Plugin": object(&[
            ("id", "string", "The plugin's own id."),
            ("name", "string", ""),
            ("version", "string", ""),
            ("digest", "string", "SHA-256 of the plugin file."),
            ("bytes", "integer", ""),
            ("uploaded", "string", ""),
            ("uploaded_by", "string", ""),
            ("key", "string", "Which trusted key signed it."),
            ("manifest", "object", "Its commands and settings, as the plugin describes them."),
        ]),
        "ChangeNotice": object(&[
            ("id", "string", ""),
            ("url", "string", "Where the notices go."),
            ("created", "string", ""),
            ("created_by", "string", ""),
            ("last_status", "string", "How the last one went: delivered, or what went wrong."),
            ("last_at", "string", ""),
            ("secret", "string", "What every notice is signed with. Only in the answer that made it."),
        ]),
        "Release": object(&[
            ("version", "string", ""),
            ("channel", "string", "stable or preview."),
            ("platform", "string", ""),
            ("published", "string", ""),
            ("notes", "string", ""),
            ("download", "string", "Where the installer is."),
            ("bytes", "integer", ""),
            ("digest", "string", "SHA-256 of the installer."),
            ("signature", "string", "Ed25519, checked against a key compiled into the running program."),
            ("key", "string", "Which key signed it."),
            ("minimum_api_version", "integer", ""),
        ]),
        "UpdateOffer": json!({
            "type": "object",
            "properties": {
                "release": { "$ref": "#/components/schemas/Release" },
                "pinned_to": { "type": "string", "description": "An administrator is holding this installation on one version." },
                "required": { "type": "boolean", "description": "This seat is too old to talk to this server." },
                "looking": { "type": "boolean", "description": "The server was still looking at the publisher's feed when it answered, so nothing newer means nothing newer yet." }
            }
        })
    })
}

fn object(fields: &[(&str, &str, &str)]) -> Value {
    let mut properties = serde_json::Map::new();
    for (name, kind, description) in fields {
        let mut field = serde_json::Map::new();
        field.insert("type".into(), json!(kind));
        if !description.is_empty() {
            field.insert("description".into(), json!(description));
        }
        properties.insert(name.to_string(), Value::Object(field));
    }
    json!({ "type": "object", "properties": Value::Object(properties) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_description_covers_every_route_the_server_serves() {
        let document = document("Example Steel");
        let paths = document["paths"].as_object().expect("paths");

        let described: BTreeSet<(String, String)> = paths
            .iter()
            .flat_map(|(path, methods)| {
                methods
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.keys())
                    .map(move |method| (method.to_uppercase(), path.clone()))
            })
            .collect();
        let served: BTreeSet<(String, String)> = crate::api::ROUTES
            .iter()
            .map(|(m, p)| (m.to_string(), p.to_string()))
            .collect();

        let undocumented: Vec<_> = served.difference(&described).collect();
        assert!(
            undocumented.is_empty(),
            "these routes exist but are not described: {undocumented:?}"
        );
        let imaginary: Vec<_> = described.difference(&served).collect();
        assert!(
            imaginary.is_empty(),
            "these are described but not served: {imaginary:?}"
        );
    }

    #[test]
    fn every_schema_a_path_refers_to_actually_exists() {
        let document = document("Example Steel");
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let text = serde_json::to_string(&document).unwrap();
        let mut missing = Vec::new();
        for piece in text.split("#/components/schemas/").skip(1) {
            let name: String = piece
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !schemas.contains_key(&name) {
                missing.push(name);
            }
        }
        assert!(missing.is_empty(), "referred to but not defined: {missing:?}");
    }

    #[test]
    fn the_description_warns_about_unscaled_sheets_where_somebody_will_see_it() {
        let document = document("Example Steel");
        let overview = document["info"]["description"].as_str().unwrap();
        assert!(overview.contains("left_out"), "the warning must be up front");
        assert!(overview.contains("Nothing is estimated"));

        let takeoff = &document["components"]["schemas"]["Takeoff"];
        let note = takeoff["properties"]["left_out"]["description"].as_str().unwrap();
        assert!(note.contains("short"), "{note}");
    }

    #[test]
    fn signing_in_needs_no_token_and_everything_else_does() {
        let document = document("Example Steel");
        assert!(document["security"][0]["bearer"].is_array());
        // The three that must work before anybody has a token.
        for open in ["/health", "/openapi.json", "/auth/token"] {
            let methods = document["paths"][open].as_object().unwrap();
            for (_, operation) in methods {
                assert_eq!(
                    operation["security"],
                    json!([]),
                    "{open} should be reachable without a token"
                );
            }
        }
        // And one that must not.
        assert!(document["paths"]["/sets/{id}/takeoff"]["get"]["security"].is_null());
    }
}
