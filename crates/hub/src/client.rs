//! Talking to a Hyperview server.
//!
//! Deliberately small and synchronous. The desktop already has a worker thread
//! for anything slow, and a viewer that has to keep a drawing on screen at
//! sixty frames a second is not helped by an async runtime underneath its file
//! transfers.
//!
//! Everything here assumes the server may be gone. A shop's server being down
//! is a Tuesday, not an emergency: the drawing on disk still opens, the
//! markups still save into it, and the sync catches up later.

use std::time::Duration;

use crate::model::*;
use crate::update::{Channel, UpdateOffer};
use crate::{API_ROOT, API_VERSION};

#[derive(Clone)]
pub struct Client {
    /// `https://drawings.thecompany.com`, no trailing slash.
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

/// A request on its way out, and whether it may be sent a second time.
///
/// Every call in this file ends in `.call()`, `.send_json()` or
/// `.send_bytes()`, and those are the three that go through here, so nothing
/// above has to know retrying exists.
pub struct Attempt {
    request: ureq::Request,
    twice: bool,
}

impl Attempt {
    fn again(request: ureq::Request) -> Attempt {
        Attempt { request, twice: true }
    }

    fn once(request: ureq::Request) -> Attempt {
        Attempt { request, twice: false }
    }

    /// The one check every request to an office server passes.
    ///
    /// An office server is not the outside world and is never refused -- a
    /// sealed seat is a seat with the doors to the internet shut, not a seat
    /// that cannot work. But "the office server" has to mean a machine on the
    /// office network, and since remote access it might not be: a server with
    /// a tunnel answers on a public hostname, and a seat pointed at that one
    /// is reaching across the internet like anything else.
    ///
    /// It matters for exactly one shop shape and that shape is the customer:
    /// a machine sealed by Group Policy inside an office that is otherwise
    /// open. The licence cannot seal only one seat, so this is the only way
    /// it happens -- and it is what a defence contractor does.
    fn allowed(&self) -> Result<(), ureq::Error> {
        let url = self.request.url();
        if crate::sealed::is_sealed() && !crate::sealed::is_inside(url) {
            return Err(ureq::Error::from(std::io::Error::other(
                crate::sealed::refusal("Reaching an office server across the internet"),
            )));
        }
        Ok(())
    }

    fn call(self) -> Result<ureq::Response, ureq::Error> {
        self.allowed()?;
        let spare = self.twice.then(|| self.request.clone());
        match (self.request.call(), spare) {
            (Err(e), Some(spare)) if never_arrived(&e) => spare.call(),
            (result, _) => result,
        }
    }

    fn send_json(self, body: impl serde::Serialize) -> Result<ureq::Response, ureq::Error> {
        self.allowed()?;
        // Turned into a value first, so the same bytes can go twice without
        // the caller having to hand over something cloneable.
        let body = match serde_json::to_value(body) {
            Ok(body) => body,
            Err(e) => return Err(ureq::Error::from(std::io::Error::other(e))),
        };
        let spare = self.twice.then(|| self.request.clone());
        match (self.request.send_json(&body), spare) {
            (Err(e), Some(spare)) if never_arrived(&e) => spare.send_json(&body),
            (result, _) => result,
        }
    }

    /// The two builder methods callers use, passed straight through so a
    /// request can still have a header or a query added before it goes.
    fn set(mut self, header: &str, value: &str) -> Attempt {
        self.request = self.request.set(header, value);
        self
    }

    fn query(mut self, key: &str, value: &str) -> Attempt {
        self.request = self.request.query(key, value);
        self
    }

    fn send_bytes(self, body: &[u8]) -> Result<ureq::Response, ureq::Error> {
        self.allowed()?;
        let spare = self.twice.then(|| self.request.clone());
        match (self.request.send_bytes(body), spare) {
            (Err(e), Some(spare)) if never_arrived(&e) => spare.send_bytes(body),
            (result, _) => result,
        }
    }
}

/// True when the request died on the way out rather than being answered.
///
/// A pooled connection the server closed while it sat idle fails like this,
/// and the giveaway is that there is no status: the server never saw it. Worth
/// one more go on a request where asking twice is the same as asking once.
fn never_arrived(e: &ureq::Error) -> bool {
    match e {
        // The server answered. Whatever it said, it said it on purpose.
        ureq::Error::Status(_, _) => false,
        ureq::Error::Transport(t) => matches!(t.kind(), ureq::ErrorKind::Io),
    }
}

/// What to tell somebody, when the machine's own words are no use.
///
/// "Invalid argument (os error 22)" is what a closed connection produces and
/// it is not a sentence anybody can do anything with.
fn in_words(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Transport(t) if matches!(t.kind(), ureq::ErrorKind::Io) => {
            "the connection to the server dropped. Try that again.".into()
        }
        ureq::Error::Transport(t) if matches!(t.kind(), ureq::ErrorKind::Dns) => {
            "that address could not be looked up. Check the server address.".into()
        }
        ureq::Error::Transport(t) if matches!(t.kind(), ureq::ErrorKind::ConnectionFailed) => {
            "nothing answered at that address. The server may be off, or the address wrong.".into()
        }
        other => other.to_string(),
    }
}

/// Anything that can go wrong on the way to a server.
#[derive(Debug)]
pub enum Trouble {
    /// The server answered, and said no.
    Refused(Problem),
    /// We could not reach it at all.
    Unreachable(String),
    /// It answered with something we could not read.
    Unreadable(String),
    /// This build is too old for that server, or the other way round.
    Mismatch { ours: u32, theirs: u32 },
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Refused(p) => write!(f, "{}", p.message),
            Trouble::Unreachable(why) => write!(f, "Could not reach the server: {why}"),
            Trouble::Unreadable(why) => write!(f, "The server said something unexpected: {why}"),
            Trouble::Mismatch { ours, theirs } => write!(
                f,
                "This copy of Excalibur View speaks version {ours} and that server speaks {theirs}. \
                 One of them needs updating."
            ),
        }
    }
}

impl std::error::Error for Trouble {}

type Answer<T> = Result<T, Trouble>;

impl Client {
    pub fn new(base: &str) -> Client {
        Client {
            base: base.trim_end_matches('/').to_string(),
            token: None,
            agent: crate::web::agent()
                .timeout_connect(Duration::from_secs(8))
                .timeout_read(Duration::from_secs(120))
                .build(),
        }
    }

    pub fn with_token(mut self, token: &str) -> Client {
        self.token = Some(token.to_string());
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base, API_ROOT, path)
    }

    /// A GET, which may be sent twice.
    ///
    /// Connections are pooled, and a pooled connection the server has already
    /// closed fails on the way out rather than on the way in -- the request
    /// never arrives. Sending it again is the whole fix, and it is safe here
    /// because asking twice is the same as asking once.
    fn get(&self, path: &str) -> Attempt {
        Attempt::again(self.sign(self.agent.get(&self.url(path))))
    }

    /// A POST, which is sent once and once only.
    ///
    /// The same dead-connection failure happens, but a POST that did reach the
    /// server and died on the way back would be done twice -- two projects,
    /// two people, two of whatever it made. Better a sentence somebody can act
    /// on than a duplicate they have to find.
    fn post(&self, path: &str) -> Attempt {
        Attempt::once(self.sign(self.agent.post(&self.url(path))))
    }

    fn sign(&self, request: ureq::Request) -> ureq::Request {
        match &self.token {
            Some(token) => request.set("Authorization", &format!("Bearer {token}")),
            None => request,
        }
    }

    // ---- the bottom of the stack -----------------------------------------

    fn read<T: serde::de::DeserializeOwned>(result: Result<ureq::Response, ureq::Error>) -> Answer<T> {
        match result {
            Ok(response) => response.into_json::<T>().map_err(|e| {
                // Reading the body can fail because the connection died
                // halfway, which is a different thing from the server sending
                // nonsense, and the person reading the message can act on one
                // and not the other.
                if e.kind() == std::io::ErrorKind::UnexpectedEof || e.raw_os_error().is_some() {
                    Trouble::Unreachable(
                        "the connection to the server dropped partway through the answer. \
                         Try that again."
                            .into(),
                    )
                } else {
                    Trouble::Unreadable(e.to_string())
                }
            }),
            Err(ureq::Error::Status(code, response)) => {
                // A server of ours explains itself. Anything else gets a
                // sentence built from what little it gave us.
                let problem = response.into_json::<Problem>().unwrap_or_else(|_| match code {
                    401 => Problem::new("unauthorised", "Sign in again: that session has expired."),
                    403 => Problem::new(
                        "forbidden",
                        "Your account is not allowed to do that. An administrator can change it.",
                    ),
                    404 => Problem::new("not_found", "That is not on this server."),
                    413 => Problem::new(
                        "too_big",
                        "That drawing set is larger than this server accepts.",
                    ),
                    500..=599 => Problem::new(
                        "server_error",
                        format!("The server had a problem ({code}). Nothing was changed."),
                    ),
                    _ => Problem::new("http_error", format!("The server answered {code}.")),
                });
                Err(Trouble::Refused(problem))
            }
            Err(e) => Err(Trouble::Unreachable(in_words(&e))),
        }
    }

    // ---- reaching this server from a jobsite -----------------------------

    pub fn remote(&self) -> Answer<crate::Remote> {
        Self::read(self.get("/admin/remote").call())
    }

    pub fn remote_on(&self, token: &str, hostname: &str) -> Answer<crate::Remote> {
        Self::read(self.post("/admin/remote").send_json(serde_json::json!({
            "token": token,
            "hostname": hostname,
        })))
    }

    pub fn remote_off(&self) -> Answer<crate::Remote> {
        Self::read(
            Attempt::once(self.sign(self.agent.delete(&self.url("/admin/remote")))).call(),
        )
    }

    // ---- is it there, and who am I ---------------------------------------

    /// Asks a server whether it is a Hyperview server at all, and whether this
    /// build can talk to it. Done before anything else, so a wrong address
    /// fails at the address rather than halfway through an upload.
    pub fn health(&self) -> Answer<Health> {
        let health: Health = Self::read(self.get("/health").call())?;
        if health.api_version != API_VERSION {
            return Err(Trouble::Mismatch {
                ours: API_VERSION,
                theirs: health.api_version,
            });
        }
        Ok(health)
    }

    pub fn sign_in(&mut self, email: &str, password: &str) -> Answer<Session> {
        let session: Session = Self::read(
            self.post("/auth/token")
                .send_json(SignIn {
                    email: email.to_string(),
                    password: password.to_string(),
                }),
        )?;
        self.token = Some(session.token.clone());
        Ok(session)
    }

    pub fn me(&self) -> Answer<User> {
        Self::read(self.get("/me").call())
    }

    // ---- getting an account ----------------------------------------------

    /// Claims a server nobody has claimed yet, and signs in as the
    /// administrator it just made. Refused on a server that already belongs to
    /// somebody, which is what `Health::claimed` says before this is called.
    pub fn claim(&mut self, setup: &Setup) -> Answer<Claimed> {
        let claimed: Claimed = Self::read(self.post("/setup").send_json(setup))?;
        self.token = Some(claimed.session.token.clone());
        Ok(claimed)
    }

    /// Makes an account on somebody else's server with their join code, and
    /// signs in as it.
    pub fn join(&mut self, join: &Join) -> Answer<Session> {
        let session: Session = Self::read(self.post("/join").send_json(join))?;
        self.token = Some(session.token.clone());
        Ok(session)
    }

    /// Where the office stands with Excalibur View Office. A server from
    /// before licensing has never heard of it and answers `None`.
    pub fn license(&self) -> Answer<Option<crate::license::Standing>> {
        match Self::read(self.get("/license").call()) {
            Ok(standing) => Ok(Some(standing)),
            Err(Trouble::Refused(problem)) if problem.error == "not_found" => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Adds a license file: its whole text. Administrators only. Checked by
    /// the server against the keys built into it.
    pub fn add_license(&self, text: &str) -> Answer<crate::license::Standing> {
        Self::read(self.post("/license").send_json(serde_json::json!({ "license": text })))
    }

    /// Everybody with an account here. Administrators only.
    pub fn people(&self) -> Answer<Vec<User>> {
        Self::read(self.get("/people").call())
    }

    /// Gives somebody an account. Administrators only.
    ///
    /// The other way in is the join code, which names nobody and lets
    /// everybody in as the same role. This one is for the shop that would
    /// rather hand out accounts than a shared secret.
    pub fn add_person(&self, name: &str, email: &str, password: &str, role: Role) -> Answer<User> {
        Self::read(self.post("/people").send_json(serde_json::json!({
            "name": name,
            "email": email,
            "password": password,
            "role": role.word(),
        })))
    }

    /// Changes what somebody is allowed to do. Administrators only.
    ///
    /// It takes effect on the next thing that seat asks the server, because
    /// the role is read out of `people` on every call and never carried in
    /// the session.
    pub fn change_role(&self, id: &str, role: Role) -> Answer<User> {
        Self::read(
            self.post(&format!("/people/{id}/role"))
                .send_json(serde_json::json!({ "role": role.word() })),
        )
    }

    /// Takes somebody's account away, and their keys and sessions with it.
    /// What they made stays. Administrators only.
    pub fn remove_person(&self, id: &str) -> Answer<serde_json::Value> {
        Self::read(
            self.post(&format!("/people/{id}/remove")).send_json(serde_json::json!({})),
        )
    }

    /// How people are getting accounts. Administrators only.
    pub fn joining(&self) -> Answer<Joining> {
        Self::read(self.get("/admin/joining").call())
    }

    /// Changes that, or issues a new code. Anything left `None` stays as it is.
    pub fn change_joining(
        &self,
        how: Option<&str>,
        role: Option<&str>,
        new_code: bool,
    ) -> Answer<Joining> {
        self.change_joining_fully(how, role, new_code, None)
    }

    /// The same, and how many days the code should last. `Some(0)` means
    /// forever, which has to be asked for rather than being what happens by
    /// not choosing.
    pub fn change_joining_fully(
        &self,
        how: Option<&str>,
        role: Option<&str>,
        new_code: bool,
        days: Option<u32>,
    ) -> Answer<Joining> {
        Self::read(self.post("/admin/joining").send_json(serde_json::json!({
            "how": how,
            "role": role,
            "new_code": new_code,
            "days": days,
        })))
    }

    // ---- inviting one person ---------------------------------------------

    /// Makes one invitation. Administrators only. Nothing is emailed: what
    /// comes back is the code, to send however you already talk to them.
    pub fn invite(
        &self,
        email: &str,
        name: &str,
        role: &str,
        days: Option<u32>,
    ) -> Answer<crate::Invitation> {
        Self::read(self.post("/admin/invites").send_json(serde_json::json!({
            "email": email,
            "name": name,
            "role": role,
            "days": days,
        })))
    }

    /// The invitations nobody has used yet.
    pub fn invites(&self) -> Answer<Vec<crate::Invitation>> {
        Self::read(self.get("/admin/invites").call())
    }

    /// Throws one away before it is used.
    pub fn drop_invite(&self, code: &str) -> Answer<Vec<crate::Invitation>> {
        Self::read(
            Attempt::once(self.sign(self.agent.delete(&self.url(&format!("/admin/invites/{code}")))))
                .call(),
        )
    }

    /// How this server keeps up to date. Administrators only.
    pub fn update_settings(&self) -> Answer<crate::UpdateSettings> {
        Self::read(self.get("/admin/updates").call())
    }

    /// Stable, or early. Administrators only.
    pub fn change_update_channel(&self, channel: &str) -> Answer<crate::UpdateSettings> {
        Self::read(
            self.post("/admin/updates")
                .send_json(serde_json::json!({ "channel": channel })),
        )
    }

    /// Whether Excalibur Fleet may watch this server. Administrators only.
    pub fn fleet_access(&self) -> Answer<crate::FleetAccess> {
        Self::read(self.get("/admin/fleet").call())
    }

    /// Turns the Fleet's access on (with a fresh key, returned once) or off.
    pub fn change_fleet_access(&self, on: bool) -> Answer<crate::FleetAccess> {
        Self::read(self.post("/admin/fleet").send_json(serde_json::json!({ "on": on })))
    }

    /// Makes a key for a program. `purpose` is `assistant` (reads the
    /// takeoffs over MCP, nothing else), `integration` (the API as this
    /// person, never their administrator's power; administrators only) or
    /// `sync` (only the routes a program keeping projects in step needs;
    /// administrators only). The key is in the answer, once.
    pub fn make_key(&self, purpose: &str, name: &str) -> Answer<crate::ApiKey> {
        Self::read(self.post("/me/keys").send_json(crate::NewKey {
            purpose: purpose.to_string(),
            name: name.to_string(),
            expires_in_days: None, // WHAT: no end asked for. WHY: this call keeps making the keys it always made.
        }))
    }

    /// The keys this person has made; every key, for an administrator.
    pub fn keys(&self) -> Answer<Vec<crate::ApiKey>> {
        Self::read(self.get("/me/keys").call())
    }

    /// Takes a key back. Whatever was using it stops working at once.
    pub fn revoke_key(&self, id: &str) -> Answer<serde_json::Value> {
        Self::read(self.post(&format!("/keys/{id}/revoke")).send_json(serde_json::json!({})))
    }

    /// A set's markups the way Bluebeam's markup list hands them over.
    pub fn markup_list(&self, set: &str) -> Answer<serde_json::Value> {
        Self::read(self.get(&format!("/sets/{set}/markuplist")).call())
    }

    /// Changes the signed-in person's own password. Every other place they
    /// are signed in is signed out.
    pub fn change_password(&self, current: &str, new: &str) -> Answer<serde_json::Value> {
        Self::read(self.post("/me/password").send_json(crate::ChangePassword {
            current: current.to_string(),
            new: new.to_string(),
        }))
    }

    // ---- projects and drawing sets ---------------------------------------

    pub fn projects(&self) -> Answer<Vec<Project>> {
        Self::read(self.get("/projects").call())
    }

    /// One project by its id. Refused as "not found" when the caller is not
    /// on it, which is the same answer they would get for one that does not
    /// exist -- so nobody learns a job exists by being refused it.
    pub fn project(&self, id: &str) -> Answer<Project> {
        Self::read(self.get(&format!("/projects/{id}")).call())
    }

    /// Who is on a project. Empty means nobody has been named, which means
    /// it is everybody's.
    pub fn who_is_on(&self, project: &str) -> Answer<Vec<String>> {
        Self::read(self.get(&format!("/projects/{project}/people")).call())
    }

    /// Puts somebody on a project, or takes them off. Administrators only.
    pub fn change_who_is_on(&self, project: &str, person: &str, on: bool) -> Answer<Vec<String>> {
        Self::read(
            self.post(&format!("/projects/{project}/people"))
                .send_json(serde_json::json!({ "person": person, "on": on })),
        )
    }

    pub fn create_project(&self, number: &str, name: &str) -> Answer<Project> {
        Self::read(self.post("/projects").send_json(NewProject {
            number: number.to_string(),
            name: name.to_string(),
            reference: None,
            place: None,
        }))
    }

    /// Puts a drawing set on the server, in a project. The PDF goes up as the
    /// bytes it is; the server reads the sheets out of it on the way in.
    pub fn upload_set(
        &self,
        project: &str,
        name: &str,
        filename: &str,
        pdf: &[u8],
    ) -> Answer<DrawingSet> {
        let (kind, body) = multipart(&[("project", project), ("name", name)], filename, pdf);
        Self::read(self.post("/sets").set("Content-Type", &kind).send_bytes(&body))
    }

    /// Shares a tool chest with everybody on the server. Administrators only.
    pub fn upload_chest(&self, name: &str, filename: &str, bytes: &[u8]) -> Answer<Chest> {
        let (kind, body) = multipart(&[("name", name)], filename, bytes);
        Self::read(self.post("/chests").set("Content-Type", &kind).send_bytes(&body))
    }

    /// The plugins the office hands every seat.
    pub fn plugins(&self) -> Answer<Vec<PluginInfo>> {
        Self::read(self.get("/plugins").call())
    }

    /// One plugin file, to be checked by the seat before it is loaded.
    pub fn plugin_file(&self, id: &str) -> Answer<Vec<u8>> {
        match self.get(&format!("/plugins/{id}/file")).call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| Trouble::Unreadable(e.to_string()))?;
                Ok(bytes)
            }
            other => Self::read::<serde_json::Value>(other).map(|_| Vec::new()),
        }
    }

    /// Hands a signed plugin to every seat in the office. Administrators only;
    /// the server checks the signature before it keeps it.
    pub fn upload_plugin(&self, file: &[u8]) -> Answer<PluginInfo> {
        Self::read(
            self.post("/plugins")
                .set("Content-Type", "application/octet-stream")
                .send_bytes(file),
        )
    }

    /// Runs one of the office's plugins on the server rather than here, for a
    /// copy that isn't allowed to run code it downloads. The input goes over
    /// packed, the way a seat would hand it to the plugin itself, and what
    /// comes back is the plugin's own answer.
    pub fn run_plugin(&self, id: &str, input: &plugin_api::Input) -> Answer<plugin_api::Answer> {
        Self::read(
            self.post(&format!("/plugins/{id}/run"))
                .set("Content-Type", "application/octet-stream")
                .send_bytes(&plugin_api::pack(input)),
        )
    }

    /// Where the office tells other programs a takeoff changed. Administrators only.
    pub fn notices(&self) -> Answer<Vec<ChangeNotice>> {
        Self::read(self.get("/notices").call())
    }

    /// A new address to tell. The secret comes back this once.
    pub fn add_notice(&self, url: &str) -> Answer<ChangeNotice> {
        Self::read(self.post("/notices").send_json(serde_json::json!({ "url": url })))
    }

    /// Sends a test notice now. What came of it: `delivered`, or why not.
    pub fn test_notice(&self, id: &str) -> Answer<String> {
        let said: serde_json::Value =
            Self::read(self.post(&format!("/notices/{id}/test")).send_json(serde_json::json!({})))?;
        Ok(said["status"].as_str().unwrap_or_default().to_string())
    }

    pub fn remove_notice(&self, id: &str) -> Answer<serde_json::Value> {
        Self::read(self.post(&format!("/notices/{id}/remove")).send_json(serde_json::json!({})))
    }

    /// Stops handing a plugin out. Seats drop it the next time they look.
    pub fn remove_plugin(&self, id: &str) -> Answer<serde_json::Value> {
        Self::read(self.post(&format!("/plugins/{id}/remove")).send_json(serde_json::json!({})))
    }

    pub fn sets(&self, project: &str) -> Answer<Vec<DrawingSet>> {
        Self::read(self.get(&format!("/projects/{project}/sets")).call())
    }

    pub fn set(&self, id: &str) -> Answer<DrawingSet> {
        Self::read(self.get(&format!("/sets/{id}")).call())
    }

    /// A copy of a drawing set to mark up, so the issued set stays clean.
    /// Costs no upload: the copy points at the same file.
    pub fn copy_set(&self, id: &str, name: &str) -> Answer<DrawingSet> {
        Self::read(
            self.post(&format!("/sets/{id}/copy"))
                .send_json(serde_json::json!({ "name": name })),
        )
    }

    pub fn sheets(&self, id: &str) -> Answer<Vec<Sheet>> {
        Self::read(self.get(&format!("/sets/{id}/sheets")).call())
    }

    // ---- folders, who sees them, and files that aren't drawings -----------

    /// A project's folders this person may see, the empty ones too, each with
    /// who sees it.
    pub fn folders(&self, project: &str) -> Answer<Vec<FolderInfo>> {
        Self::read(self.get(&format!("/projects/{project}/folders")).call()) // WHAT: GET the folders. WHY: the Studio shows every folder.
    }

    /// Says who sees one folder of a project. Administrators only.
    pub fn set_folder_rule(&self, project: &str, folder: &str, see: &See) -> Answer<serde_json::Value> {
        Self::read(
            self.post(&format!("/projects/{project}/folders"))
                .send_json(serde_json::json!({ "folders": [{ "folder": folder, "see": see }] })), // WHAT: one folder and its rule. WHY: the route takes a list; the Studio changes one at a time.
        )
    }

    /// A project's files that aren't drawing sets, in folders this person may see.
    pub fn files(&self, project: &str) -> Answer<Vec<ProjectFile>> {
        Self::read(self.get(&format!("/projects/{project}/files")).call()) // WHAT: GET the files. WHY: listed in their folders after the drawings.
    }

    /// One file's bytes.
    pub fn file_bytes(&self, id: &str) -> Answer<Vec<u8>> {
        match self.get(&format!("/files/{id}/file")).call() {
            Ok(response) => {
                let mut bytes = Vec::new(); // WHAT: the file. WHY: saved as a copy to open.
                response.into_reader().read_to_end(&mut bytes).map_err(|e| Trouble::Unreadable(e.to_string()))?; // WHAT: read whole. WHY: a file is opened whole.
                Ok(bytes)
            }
            other => Self::read::<serde_json::Value>(other).map(|_| Vec::new()), // WHAT: a refusal, as a problem. WHY: the same errors as every other call.
        }
    }

    /// Every group of people and who is in each. Administrators only.
    pub fn groups(&self) -> Answer<Vec<Group>> {
        Self::read(self.get("/groups").call()) // WHAT: GET the groups. WHY: Studio, Office, People.
    }

    /// Makes a group, or finds the one of that name. Administrators only.
    pub fn make_group(&self, name: &str) -> Answer<Group> {
        Self::read(self.post("/groups").send_json(serde_json::json!({ "name": name }))) // WHAT: POST a name. WHY: a new group.
    }

    /// Puts somebody in a group, or takes them out. Administrators only.
    pub fn change_group(&self, group: &str, person: &str, on: bool) -> Answer<Group> {
        Self::read(self.post(&format!("/groups/{group}/people")).send_json(serde_json::json!({ "person": person, "on": on }))) // WHAT: in or out. WHY: decides the folders they see.
    }

    /// Downloads the drawing itself.
    ///
    /// `have` is the digest of the copy already on this machine. When it
    /// matches, the server says so and nothing comes down the wire — which
    /// matters when a drawing set is sixty megabytes and six people open it
    /// every morning.
    pub fn download(&self, id: &str, have: Option<&str>) -> Answer<Option<Vec<u8>>> {
        let mut request = self.get(&format!("/sets/{id}/file"));
        if let Some(digest) = have {
            request = request.set("If-None-Match", &format!("\"{digest}\""));
        }
        match request.call() {
            // Not modified: we already have these exact bytes. It arrives as a
            // perfectly ordinary response rather than an error, so it is
            // checked for by its status and not by catching something.
            Ok(response) if response.status() == 304 => Ok(None),
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| Trouble::Unreadable(e.to_string()))?;
                Ok(Some(bytes))
            }
            other => Self::read::<serde_json::Value>(other).map(|_| None),
        }
    }

    // ---- markups ---------------------------------------------------------

    /// Everything that has happened to this set's markups since `since`.
    pub fn markups(&self, id: &str, since: u64) -> Answer<MarkupPage> {
        Self::read(
            self.get(&format!("/sets/{id}/markups"))
                .query("since", &since.to_string())
                .call(),
        )
    }

    pub fn push_markups(&self, id: &str, markups: &[NewMarkup]) -> Answer<MarkupPage> {
        Self::read(self.post(&format!("/sets/{id}/markups")).send_json(markups))
    }

    // ---- the number everybody actually wants ------------------------------

    /// The takeoff, measured by the same engine the viewer uses.
    ///
    /// This is the endpoint an estimating system, an ERP or a spreadsheet macro
    /// calls. Read `left_out` before you trust `tons`.
    pub fn takeoff(&self, id: &str) -> Answer<Takeoff> {
        Self::read(self.get(&format!("/sets/{id}/takeoff")).call())
    }

    // ---- shared tool chests ----------------------------------------------

    pub fn chests(&self) -> Answer<Vec<Chest>> {
        Self::read(self.get("/chests").call())
    }

    pub fn chest_file(&self, id: &str) -> Answer<Vec<u8>> {
        match self.get(&format!("/chests/{id}/file")).call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| Trouble::Unreadable(e.to_string()))?;
                Ok(bytes)
            }
            other => Self::read::<serde_json::Value>(other).map(|_| Vec::new()),
        }
    }

    // ---- updates ---------------------------------------------------------

    pub fn update_offer(&self, running: &str, channel: Channel, platform: &str) -> Answer<UpdateOffer> {
        self.asking_for_update(running, channel, platform, false)
    }

    /// The same question, with the server asked to look at the publisher's
    /// feed first rather than answer from its last look. What "Check for
    /// Updates" sends: somebody is waiting for the answer. A server older
    /// than this question ignores the asking and answers from its last look.
    pub fn update_offer_now(&self, running: &str, channel: Channel, platform: &str) -> Answer<UpdateOffer> {
        self.asking_for_update(running, channel, platform, true)
    }

    fn asking_for_update(
        &self,
        running: &str,
        channel: Channel,
        platform: &str,
        fresh: bool,
    ) -> Answer<UpdateOffer> {
        Self::read(
            self.get("/update/latest")
                .query("fresh", if fresh { "true" } else { "false" })
                .query("version", running)
                .query(
                    "channel",
                    match channel {
                        Channel::Stable => "stable",
                        Channel::Preview => "preview",
                    },
                )
                .query("platform", platform)
                .call(),
        )
    }

    /// Fetches an installer. Nothing is done with these bytes until
    /// [`crate::update::Trusted::check`] has passed on them.
    pub fn download_update(&self, download: &str) -> Answer<Vec<u8>> {
        let url = if download.starts_with("http://") || download.starts_with("https://") {
            download.to_string()
        } else {
            format!("{}{}", self.base, download)
        };
        match self.sign(self.agent.get(&url)).call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut bytes)
                    .map_err(|e| Trouble::Unreadable(e.to_string()))?;
                Ok(bytes)
            }
            Err(ureq::Error::Status(code, _)) => Err(Trouble::Refused(Problem::new(
                "download_failed",
                format!("The update could not be downloaded ({code})."),
            ))),
            Err(e) => Err(Trouble::Unreachable(e.to_string())),
        }
    }
}

use std::io::Read;

/// A multipart form: some text fields and one file.
///
/// The boundary is made from the file's own digest, so the file cannot
/// happen to contain it.
fn multipart(fields: &[(&str, &str)], filename: &str, file: &[u8]) -> (String, Vec<u8>) {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(file);
    let boundary: String = std::iter::once("----hyperview".to_string())
        .chain(digest.iter().take(12).map(|b| format!("{b:02x}")))
        .collect();
    let mut body = Vec::with_capacity(file.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    let filename: String = filename
        .chars()
        .map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_address_with_a_trailing_slash_does_not_produce_a_double_one() {
        let client = Client::new("https://drawings.example.com/");
        assert_eq!(
            client.url("/projects"),
            "https://drawings.example.com/api/v1/projects"
        );
    }

    #[test]
    fn an_update_download_may_point_somewhere_else_entirely() {
        let client = Client::new("https://drawings.example.com");
        // Relative: their own server.
        assert_eq!(client.base(), "https://drawings.example.com");
        // The client hands absolute addresses straight through, which is how a
        // shop can serve the installer off a file share or a CDN. The
        // signature check is what makes that safe, not where it came from.
        assert!("https://releases.example.com/x.msi".starts_with("https://"));
    }

    #[test]
    fn a_server_that_speaks_another_version_is_named_rather_than_guessed_at() {
        let trouble = Trouble::Mismatch { ours: 1, theirs: 3 };
        let said = trouble.to_string();
        assert!(said.contains("version 1"), "{said}");
        assert!(said.contains("speaks 3"), "{said}");
    }
}
