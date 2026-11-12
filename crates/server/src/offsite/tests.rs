//! The off-site backup's claims, each attacked rather than only shown working:
//! 1. only the public half of the key is on the server; a private key is refused before anything runs
//! 2. nothing leaves unencrypted, and the backup can't be read without the private key (real age, where installed)
//! 3. nothing is marked unless it is confirmed where it went
//! 4. the keep rule is 7 / 4 / 12, never tonight's, never a stranger's
//! 5. the archive reader can't be made to write outside its folder
//! 6. the drill restores into a scratch folder, checks counts, fingerprints and a record, and fails when it should
//! 7. the ops report says null for unknown, never a guess
//! 8. a restored file is owner-only from the moment it exists
//! 9. no object name can reach outside the target, and no remote can be read by rclone as an option
//! 10. the settings are checked as the server starts, and the night comes once a day
//! 11. a target belongs to one office: another office's target is refused before anything is sent or pruned (audit M2)

use super::*; // WHAT: the module under test. WHY: its private parts are tested too.

struct Tmp(PathBuf); // WHAT: a temp folder removed when dropped. WHY: no clutter, no shared state between tests.
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/offsite-tests");
        create_private_dir(&root).unwrap();
        let p = root.join(format!("{tag}-{}", hex(&rand::random::<[u8; 8]>())));
        create_private_dir(&p).unwrap();
        Tmp(p)
    } // WHAT: a fresh owner-only folder in the build's own (git-ignored) target folder. WHY: each test its own, never in a temp folder other accounts share; it holds test keys and plaintext.
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
} // WHAT: remove it. WHY: tidy.

/// Writes a file owner-only (Unix 0600), as a person setting up the work folder would. The tests' umask may be 0002.
fn put(p: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    std::fs::write(p.as_ref(), bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p.as_ref(), std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
} // WHAT: write, then owner-only. WHY: the work folder trusts nothing others can change.

const CH: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"; // WHAT: the letters of an age key. WHY: well-formed fake keys.
fn fake_pub() -> String {
    let r: [u8; 58] = std::array::from_fn(|_| rand::random::<u8>());
    format!(
        "age1{}",
        r.iter()
            .map(|b| CH.as_bytes()[(*b % 32) as usize] as char)
            .collect::<String>()
    )
} // WHAT: a fake public key line. WHY: right shape, so the real check accepts it.
fn secret_line() -> String {
    ["AGE", "SECRET", "KEY", "1QQQQ"].join("-")
} // WHAT: a private-key-shaped line, built in pieces. WHY: the secret scanners must not flag a test value.

struct World {
    _t: Tmp,
    data: PathBuf,
    work: PathBuf,
    bucket: PathBuf,
    id: PathBuf,
    wrong: PathBuf,
    root: PathBuf,
} // WHAT: a live server's data, the job's folder, a folder target and two keys. WHY: the same layout as the server.

fn world() -> World {
    let t = Tmp::new("w");
    let root = t.0.clone(); // WHAT: the test's folder. WHY: everything inside it.
    let (data, work, bucket) = (root.join("data"), root.join("work"), root.join("bucket")); // WHAT: the three places. WHY: apart, as on the server.
    let store = crate::Store::open(&data.join("hyperview.sqlite"), &data.join("files")).unwrap(); // WHAT: a real store. WHY: the real schema and file layout.
    let pdf = b"%PDF-1.7\nSECRET-DRAWING-CONTENT the real job\n%%EOF\n".to_vec();
    let d1 = store.put_blob(&pdf).unwrap(); // WHAT: a drawing. WHY: a file to back up.
    let d2 = store.put_blob(b"a chest file").unwrap(); // WHAT: a second file. WHY: more than one.
    store.with(|db| { db.execute_batch(&format!("INSERT INTO projects(id,number,name,created) VALUES('p1','2630','Real Job Name','2026-11-01'); INSERT INTO sets(id,project,name,filename,digest,bytes,sheets,uploaded,uploaded_by) VALUES('s1','p1','Structural','s.pdf','{d1}',{},1,'2026-11-01T10:00:00Z','x'); INSERT INTO files(id,project,name,digest,bytes,uploaded,uploaded_by) VALUES('f1','p1','note.txt','{d2}',12,'2026-11-01','x');", pdf.len()))?; Ok(()) }).unwrap(); // WHAT: a project, a set and a file. WHY: real records pointing at real files.
    store.set_setting(crate::api::settings::INSTALLATION, &crate::api::fresh_id("inst")).unwrap(); // WHAT: the office's own id, as the server makes it when it first starts. WHY: the backup names its office by it
    store.set_setting(crate::api::settings::COMPANY, "Test Fab Co").unwrap(); // WHAT: the company it was set up with. WHY: rides only inside the encrypted catalog
    drop(store); // WHAT: close it. WHY: as a running server would leave it, open or not.
    create_private_dir(&work).unwrap();
    lock_work(&work); // WHAT: lock the work folder with the server's own code, as it does each time it starts. WHY: a new Windows folder inherits its parent's permissions (on a build machine, often write for every account); the server locks it before any night runs, so the tests take the same path.
    #[cfg(windows)]
    assert_eq!(others_can_change(&work), None, "the server's lock leaves the work folder its own"); // WHAT: the lock worked. WHY: lock_work only logs a failure, and the backup's refusal would hide where it went wrong.
    let pubk = fake_pub();
    put(
        work.join("age-recipients.txt"),
        format!("# public key: {pubk}\n{pubk}\n"),
    )
    .unwrap(); // WHAT: the public key file. WHY: what Creede places.
    let id = root.join("id.txt");
    std::fs::write(&id, format!("# public key: {pubk}\n{}\n", secret_line())).unwrap(); // WHAT: the right private key (kept off the server). WHY: the drill's key.
    let wrong = root.join("wrong.txt");
    std::fs::write(
        &wrong,
        format!("# public key: {}\n{}\n", fake_pub(), secret_line()),
    )
    .unwrap(); // WHAT: someone else's key. WHY: must open nothing.
    World {
        _t: t,
        data,
        work,
        bucket,
        id,
        wrong,
        root,
    }
}

fn job(w: &World) -> Job<'static> {
    Job {
        product: &VIEW,
        data: w.data.clone(),
        work: w.work.clone(),
        target: Target::Folder(w.bucket.clone()),
        cipher: Cipher::Fake,
        version: "9.9.9".into(),
        mark: true,
    }
} // WHAT: a job against the folder target. WHY: the stand-in cipher runs anywhere.
fn drill(w: &World, id: &Path, scratch: &str) -> Drill<'static> {
    Drill {
        job: job(w),
        identity: id.to_path_buf(),
        scratch: w.root.join(scratch),
        object: None,
        full: true,
        sample: 20,
        keep: false,
    }
} // WHAT: a full drill. WHY: the strongest check.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_in(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
} // WHAT: every file under a folder. WHY: to search what left the box.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
} // WHAT: a byte search. WHY: plaintext must not appear.

#[test]
fn names_are_one_per_second_in_utc_and_a_stranger_or_impossible_date_is_never_ours() {
    let t = time::OffsetDateTime::from_unix_timestamp(1_793_000_000).unwrap(); // WHAT: a fixed moment. WHY: repeatable.
    let n = object_name("view", t);
    assert_eq!(parse_name("view", &n), Some(t)); // WHAT: round trip. WHY: the keep rule dates names.
    for bad in [
        "view-20261332-010101.tar.gz.age",
        "view-20261102-250000.tar.gz.age",
        "archetype-20261102-010101.tar.gz.age",
        "view-20261102-010101.tar.gz.age.json",
        "view-2026110-0101010.tar.gz.age",
        "../view-20261102-010101.tar.gz.age",
    ] {
        assert_eq!(parse_name("view", bad), None, "{bad}");
    } // WHAT: each must be refused. WHY: never counted, never deleted.
}

#[test]
fn the_keep_rule_is_7_4_12_counting_tonight_and_touches_nothing_else() {
    let day = |d: u32| format!("view-202611{d:02}-013000.tar.gz.age"); // WHAT: a nightly name for a November day. WHY: readable cases.
    let mut l = BTreeMap::new();
    l.insert(
        "daily".to_string(),
        (1..=9)
            .map(day)
            .chain(["stranger.bin".to_string(), format!("{}.json", day(1))])
            .collect::<Vec<_>>(),
    );
    l.insert("weekly".into(), vec![day(2)]);
    l.insert("monthly".into(), vec![]); // WHAT: nine nights, a stranger and a sidecar. WHY: the rule must only count ours.
    let p = plan("view", &l, &day(10)).unwrap(); // WHAT: tonight is the 10th. WHY: a Tuesday in the week after the 2nd's.
    assert!(
        p.weekly && p.monthly,
        "first of its week (2nd was the week before) and the month had none"
    ); // WHAT: copies due. WHY: the first good one stands for the week and month.
    let deleted: Vec<_> = p
        .deletes
        .iter()
        .filter(|(f, _)| f == "daily")
        .map(|(_, n)| n.clone())
        .collect(); // WHAT: the daily deletes. WHY: 10 with tonight's, keep 7.
    assert_eq!(deleted, vec![day(3), day(2), day(1)]); // WHAT: the three oldest. WHY: newest 7 kept.
    assert!(
        !p.deletes
            .iter()
            .any(|(_, n)| n == "stranger.bin" || n.ends_with(".json") || *n == day(10)),
        "never a stranger, a sidecar or tonight's"
    ); // WHAT: nothing else. WHY: only our names are pruned.
    let later = |m: u32| format!("view-20261110-01{m:02}00.tar.gz.age"); // WHAT: names minutes after tonight's. WHY: a clock set back an hour, within the day the keep rule believes.
    let mut l2 = BTreeMap::new();
    l2.insert(
        "daily".to_string(),
        (31..=38).map(later).collect::<Vec<_>>(),
    );
    let back = plan("view", &l2, &day(10)).unwrap(); // WHAT: eight newer names already there, so tonight's sorts past the keep. WHY: a clock gone backwards.
    assert!(
        !back.deletes.iter().any(|(_, n)| *n == day(10)),
        "tonight's is never deleted, whatever the clock says"
    );
    assert_eq!(
        back.deletes,
        vec![("daily".to_string(), later(31))],
        "only the oldest of ours past the keep"
    ); // WHAT: tonight's survives. WHY: the copy just made is the one known good.
    let mut l3 = l.clone();
    l3.get_mut("daily")
        .unwrap()
        .extend((1..=7).map(|d| format!("view-209901{d:02}-013000.tar.gz.age")));
    let fut = plan("view", &l3, &day(10)).unwrap(); // WHAT: seven planted names dated 2099. WHY: they must not push the real catalogs out.
    assert_eq!(
        fut.deletes
            .iter()
            .filter(|(f, _)| f == "daily")
            .map(|(_, n)| n.clone())
            .collect::<Vec<_>>(),
        vec![day(3), day(2), day(1)],
        "the same three go as without them"
    );
    assert!(
        !fut.deletes.iter().any(|(_, n)| n.contains("2099")),
        "and the planted ones are neither counted nor deleted"
    ); // WHAT: as if they weren't there. WHY: a name from the future is not believed.
    assert!(plan("view", &l, "not-a-name").is_err()); // WHAT: refuse an undatable name. WHY: the plan never guesses.
}

#[test]
fn only_the_public_half_of_the_key_may_be_on_the_server() {
    let good = fake_pub();
    assert_eq!(
        check_recipients(&format!("# created\n{good}\n")).unwrap(),
        1
    ); // WHAT: a public key passes. WHY: the normal case.
    let e = check_recipients(&format!("{good}\n{}\n", secret_line()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("PRIVATE key"), "{e}"); // WHAT: a private key is refused. WHY: it would open every backup.
    assert!(
        check_recipients(&secret_line().to_lowercase()).is_err(),
        "lower case too"
    ); // WHAT: any case. WHY: no way around the check.
    assert!(
        check_recipients("").is_err() && check_recipients("# only a comment\n").is_err(),
        "no key: nobody could open it"
    ); // WHAT: empty refused. WHY: someone must be able to open it.
    assert!(
        check_recipients("age1short\n").is_err()
            && check_recipients(&format!("{}B\n", &good[..61])).is_err(),
        "not an age key"
    ); // WHAT: a typo refused. WHY: never encrypt to the wrong thing.
}

fn tar_of(entries: &[(&[u8; 100], u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new(); // WHAT: a hand-made archive. WHY: hostile ones the writer would never make.
    for (name, kind, body) in entries {
        let mut h = [0u8; 512];
        h[..100].copy_from_slice(&name[..]);
        h[100..108].copy_from_slice(b"0000600\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        h[156] = *kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].copy_from_slice(b"        ");
        let s: u32 = h.iter().map(|&b| u32::from(b)).sum(); // WHAT: the header's checksum, each byte widened. WHY: as ustar sums it
        h[148..156].copy_from_slice(format!("{s:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0, (512 - body.len() % 512) % 512));
    } // WHAT: each header and body. WHY: valid checksums, so only the name or kind is wrong.
    out.extend_from_slice(&[0u8; 1024]);
    out // WHAT: the end marker. WHY: a complete archive.
}
const EXPECT: &[(&str, u64)] = &[("manifest.json", 1 << 30), ("hyperview.sqlite", 1 << 30)]; // WHAT: what a catalog may hold. WHY: as the drill passes it.
const ANY: &[(&str, u64)] = &[
    ("manifest.json", 1 << 30),
    ("hyperview.sqlite", 1 << 30),
    ("a", 1 << 30),
    ("link", 1 << 30),
    ("../evil", 1 << 30),
    ("/etc/evil", 1 << 30),
    ("C:evil", 1 << 30),
    ("..\\evil", 1 << 30),
]; // WHAT: every hostile name let through this check. WHY: the name and kind checks are what is tested there.
fn name100(s: &str) -> [u8; 100] {
    let mut n = [0u8; 100];
    n[..s.len()].copy_from_slice(s.as_bytes());
    n
} // WHAT: a padded name field. WHY: the header's form.

#[test]
fn the_archive_round_trips_and_a_hostile_one_never_writes_outside_its_folder() {
    let t = Tmp::new("tar"); // WHAT: a folder. WHY: unpacked into.
    let mut buf = Vec::new();
    {
        let mut w = TarOut { w: &mut buf };
        let big = vec![7u8; 70_000];
        w.file("manifest.json", 2, &mut &b"{}"[..]).unwrap();
        w.file("hyperview.sqlite", big.len() as u64, &mut &big[..])
            .unwrap();
        w.end().unwrap();
    } // WHAT: a real archive. WHY: what the job writes.
    assert_eq!(
        untar(&mut &buf[..], &t.0.join("ok"), 1 << 20, EXPECT).unwrap(),
        vec!["manifest.json", "hyperview.sqlite"]
    );
    assert_eq!(
        std::fs::read(t.0.join("ok/hyperview.sqlite")).unwrap(),
        vec![7u8; 70_000]
    ); // WHAT: comes back byte for byte. WHY: a restore is exact.
    for (why, arch) in [
        ("climbs out", tar_of(&[(&name100("../evil"), b'0', b"x")])),
        ("absolute", tar_of(&[(&name100("/etc/evil"), b'0', b"x")])),
        ("drive letter", tar_of(&[(&name100("C:evil"), b'0', b"x")])),
        ("a link", tar_of(&[(&name100("link"), b'2', b"")])),
        (
            "twice",
            tar_of(&[(&name100("a"), b'0', b"1"), (&name100("a"), b'0', b"2")]),
        ),
        ("backslash", tar_of(&[(&name100("..\\evil"), b'0', b"x")])),
    ] {
        // WHAT: each hostile archive. WHY: a planted backup must not escape.
        let into = t.0.join(why.replace(' ', "-"));
        assert!(
            untar(&mut &arch[..], &into, 1 << 20, ANY).is_err(),
            "{why} must be refused"
        );
        assert!(
            !t.0.join("evil").exists() && !Path::new("/etc/evil").exists(),
            "{why}: nothing outside"
        ); // WHAT: refused, nothing outside. WHY: the claim.
    }
    assert!(
        untar(
            &mut &buf[..buf.len() - 1100],
            &t.0.join("short"),
            1 << 20,
            EXPECT
        )
        .is_err(),
        "cut short"
    ); // WHAT: a truncated archive. WHY: refused, not half-read as whole.
    assert!(
        untar(&mut &buf[..], &t.0.join("cap"), 1000, EXPECT).is_err(),
        "bigger than allowed"
    ); // WHAT: the size cap. WHY: a planted archive can't fill the disk.
    let stranger = tar_of(&[(&name100("evil.txt"), b'0', b"planted")]);
    assert!(
        untar(&mut &stranger[..], &t.0.join("stranger"), 1 << 20, EXPECT).is_err(),
        "a name this program never restores"
    );
    assert!(
        !t.0.join("stranger/evil.txt").exists(),
        "and nothing written"
    ); // WHAT: an unexpected plain name. WHY: only the manifest and the database come out of a catalog.
    let fat = tar_of(&[(&name100("manifest.json"), b'0', &vec![b' '; 2048])]);
    assert!(
        untar(
            &mut &fat[..],
            &t.0.join("fat"),
            1 << 20,
            &[("manifest.json", 1024)]
        )
        .is_err(),
        "over its own cap"
    );
    assert!(
        !t.0.join("fat/manifest.json").exists(),
        "refused before writing"
    ); // WHAT: a manifest over its cap. WHY: a giant never reaches the disk.
    let mut bad = buf.clone();
    bad[10] ^= 1;
    assert!(
        untar(&mut &bad[..], &t.0.join("sum"), 1 << 20, EXPECT).is_err(),
        "a damaged header"
    ); // WHAT: a flipped header byte. WHY: the checksum catches it.
}

#[test]
fn a_good_night_sends_only_encrypted_bytes_reads_them_back_and_marks_it() {
    let w = world(); // WHAT: a live server. WHY: real data.
    assert_eq!(
        report(&w.data),
        Report::default(),
        "before any backup: all unknown"
    );
    assert_eq!(backup_days(&w.data), None, "never is null, not 0"); // WHAT: unknown is null. WHY: Fleet's contract.
    let e = job(&w).backup();
    assert_eq!(e.code, 0, "{e:?}"); // WHAT: the run succeeds. WHY: the baseline.
    let daily: Vec<_> = std::fs::read_dir(w.bucket.join("daily"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect(); // WHAT: what landed. WHY: catalog and sidecar.
    let name = daily
        .iter()
        .find(|n| parse_name("view", n).is_some())
        .unwrap()
        .clone();
    assert!(daily.contains(&format!("{name}.json"))); // WHAT: both. WHY: the drill needs the sidecar.
    assert!(
        w.bucket.join("weekly").join(&name).exists()
            && w.bucket.join("monthly").join(&name).exists(),
        "first of its week and month"
    ); // WHAT: the copies. WHY: the keep rule.
    assert_eq!(
        std::fs::read_dir(w.bucket.join("blobs")).unwrap().count(),
        2,
        "both files sent"
    ); // WHAT: two objects. WHY: one per file.
    for f in files_in(&w.bucket) {
        let b = std::fs::read(&f).unwrap();
        let side = f.extension().map(|x| x == "json").unwrap_or(false); // WHAT: every object. WHY: nothing may leave in the clear.
        if !side {
            assert!(
                b.starts_with(b"FAKEAGE1"),
                "{} is the cipher's output",
                f.display()
            );
        } // WHAT: encrypted. WHY: the claim.
        for plain in [
            &b"SQLite format 3"[..],
            b"Real Job Name",
            b"SECRET-DRAWING-CONTENT",
            b"%PDF",
        ] {
            assert!(
                !contains(&b, plain),
                "{} leaks {:?}",
                f.display(),
                String::from_utf8_lossy(plain)
            );
        }
    } // WHAT: no plaintext anywhere, sidecars included. WHY: only fingerprints travel unencrypted.
    let r = report(&w.data);
    assert!(r.last_good.is_some() && r.restore_tested.is_none());
    assert_eq!(r.r#where.unwrap(), format!("folder {}", w.bucket.display()));
    assert!(backup_days(&w.data).unwrap() < 0.01); // WHAT: the ops report. WHY: Fleet's amber clears because it is true.
    assert!(
        !w.work.join("view/stage").exists() && !w.work.join("view/outgoing").exists(),
        "no plaintext stage left"
    ); // WHAT: tidy. WHY: the stage holds the database unencrypted.
    std::thread::sleep(std::time::Duration::from_millis(1100)); // WHAT: the next second. WHY: names are to the second.
    let store =
        crate::Store::open(&w.data.join("hyperview.sqlite"), &w.data.join("files")).unwrap();
    store.put_blob(b"a third file").unwrap();
    drop(store); // WHAT: one new file. WHY: only it should be sent.
    let e2 = job(&w).backup();
    assert_eq!(e2.code, 0, "{e2:?}");
    assert_eq!(
        std::fs::read_dir(w.bucket.join("blobs")).unwrap().count(),
        3,
        "only the new file sent again"
    ); // WHAT: incremental. WHY: files never change, so each is sent once.
    assert!(
        std::fs::read_to_string(w.work.join("view/offsite.log"))
            .unwrap()
            .contains("1 new sent"),
        "the log says one"
    ); // WHAT: the log. WHY: the morning read.
}

#[test]
fn the_drill_restores_into_scratch_checks_everything_and_fails_when_it_should() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0); // WHAT: a backup to drill. WHY: the setup.
    let mut d = drill(&w, &w.id, "scratch1");
    d.keep = true;
    let e = d.run();
    assert_eq!(e.code, 0, "{e:?}"); // WHAT: a full drill, kept. WHY: the restored copy is checked below.
    let restored = w.root.join("scratch1/restored");
    assert!(
        restored.join("hyperview.sqlite").exists()
            && list_blobs(&restored.join("files")).unwrap().len() == 2,
        "a full, usable data folder"
    ); // WHAT: the database and both files. WHY: a real restore copies this into place.
    let s =
        crate::Store::open(&restored.join("hyperview.sqlite"), &restored.join("files")).unwrap();
    let n: i64 = s
        .with(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM sets WHERE name='Structural'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(n, 1, "the record is there"); // WHAT: open it as the server does. WHY: the data is really back.
    assert!(
        report(&w.data).restore_tested.is_some(),
        "restoreTested is set"
    ); // WHAT: the ops report. WHY: Fleet shows the proof.
    owner_only(&w.root.join("scratch1")); // WHAT: every restored file and folder is the owner's alone. WHY: a restored database is as private as the original.
    let log = std::fs::read_to_string(w.work.join("view/offsite.log")).unwrap();
    assert!(
        log.contains("RESTORE DRILL PASSED")
            && log.contains("record s1 opened")
            && log.contains("a PDF"),
        "{log}"
    ); // WHAT: the record opened. WHY: the drill proves a real record reads.
    assert!(
        !log.contains("Real Job Name") && !log.contains("Structural"),
        "the log names no customer data"
    ); // WHAT: ids only. WHY: logs are not encrypted.
    std::fs::remove_file(w.data.join(DRILL_MARKER)).unwrap(); // WHAT: start the failures clean. WHY: none may set it.
    let wrong = drill(&w, &w.wrong, "s-wrong").run();
    assert_eq!(wrong.code, 2, "{wrong:?}");
    assert!(wrong.why.contains("could not decrypt"), "{}", wrong.why); // WHAT: someone else's key. WHY: opens nothing.
    assert!(
        !w.root.join("s-wrong").exists(),
        "a failed drill leaves no plaintext"
    ); // WHAT: cleaned up. WHY: the same.
    let cat = files_in(&w.bucket.join("daily"))
        .into_iter()
        .find(|p| parse_name("view", &p.file_name().unwrap().to_string_lossy()).is_some())
        .unwrap();
    let good = std::fs::read(&cat).unwrap();
    let mut bad = good.clone();
    let n = bad.len() - 3;
    bad[n] ^= 0xff; // WHAT: the catalog, one byte flipped. WHY: a damaged download.
    for f in ["daily", "weekly", "monthly"] {
        std::fs::write(w.bucket.join(f).join(cat.file_name().unwrap()), &bad).unwrap();
    } // WHAT: damage every copy. WHY: the drill takes the newest anywhere.
    let dmg = drill(&w, &w.id, "s-dmg").run();
    for f in ["daily", "weekly", "monthly"] {
        std::fs::write(w.bucket.join(f).join(cat.file_name().unwrap()), &good).unwrap();
    }
    assert_eq!(dmg.code, 2);
    assert!(
        dmg.why.contains("not the one that was uploaded"),
        "{}",
        dmg.why
    ); // WHAT: refused by fingerprint, then repaired. WHY: before decrypting.
    let blob = files_in(&w.bucket.join("blobs")).remove(0);
    let keep = std::fs::read(&blob).unwrap();
    std::fs::remove_file(&blob).unwrap(); // WHAT: lose one file from the target. WHY: a hole in the backup.
    let hole = drill(&w, &w.id, "s-hole").run();
    std::fs::write(&blob, keep).unwrap();
    assert_eq!(hole.code, 2, "{hole:?}");
    assert!(hole.why.contains("not in the bucket"), "{}", hole.why); // WHAT: found, then repaired. WHY: a hole is found now, not on the bad day.
    let inside = Drill {
        scratch: w.data.join("restore-here"),
        ..drill(&w, &w.id, "x")
    }
    .run();
    assert_eq!(inside.code, 4);
    assert!(
        !w.data.join("restore-here").exists(),
        "nothing made in the live data"
    ); // WHAT: scratch inside the live data. WHY: never restore over the real thing.
    std::fs::create_dir_all(w.root.join("used")).unwrap();
    std::fs::write(w.root.join("used/x"), "x").unwrap();
    assert_eq!(
        drill(&w, &w.id, "used").run().code,
        4,
        "a used scratch folder is refused"
    ); // WHAT: not empty. WHY: never mix a restore with anything.
    std::fs::create_dir_all(w.root.join("empty")).unwrap();
    assert_eq!(
        drill(&w, &w.id, "empty").run().code,
        4,
        "even an empty one made earlier"
    );
    assert!(w.root.join("empty").exists(), "and it is left alone"); // WHAT: a folder already there. WHY: it may be open to others; the drill makes its own.
    assert!(
        !w.data.join(DRILL_MARKER).exists(),
        "no failed drill marks a restore as tested"
    ); // WHAT: no marker. WHY: restoreTested only for a pass.
    let mut sample = drill(&w, &w.id, "s-sample");
    sample.full = false;
    sample.sample = 0;
    assert_eq!(
        sample.run().code,
        0,
        "a sample drill still opens the record's own file"
    ); // WHAT: the monthly form. WHY: the record is opened whole.
}

#[test]
fn nothing_is_marked_unless_it_is_confirmed_where_it_went() {
    let w = world();
    let rf = w.work.join("age-recipients.txt");
    let good = std::fs::read_to_string(&rf).unwrap(); // WHAT: the public key file. WHY: one case breaks it.
    put(&rf, format!("{good}{}\n", secret_line())).unwrap();
    let e = job(&w).backup();
    put(&rf, &good).unwrap(); // WHAT: a private key on the server. WHY: must refuse.
    assert_eq!(e.code, 2);
    assert!(e.why.contains("PRIVATE key"));
    assert!(!w.bucket.exists(), "nothing left the box"); // WHAT: refused before anything. WHY: the claim.
    std::fs::write(&w.bucket, "a file where the target folder should be").unwrap();
    let e = job(&w).backup();
    std::fs::remove_file(&w.bucket).unwrap();
    assert_eq!(e.code, 3, "{e:?}"); // WHAT: a target that can't be written. WHY: a failed upload.
    assert!(
        report(&w.data).last_good.is_none() && backup_days(&w.data).is_none(),
        "a failed upload is never marked"
    ); // WHAT: no marker. WHY: Fleet keeps it amber.
    assert!(
        !w.work.join("view/stage").exists(),
        "the stage is deleted on failure too"
    ); // WHAT: no plaintext left. WHY: it holds the database.
    std::fs::write(
        w.data.join("hyperview.sqlite"),
        b"not a database at all, just bytes",
    )
    .unwrap();
    let e = job(&w).backup();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(e.why.contains("database"), "{}", e.why);
    assert!(report(&w.data).last_good.is_none()); // WHAT: a database that won't copy or open. WHY: never sent, never marked.
    let inside = Job {
        work: w.data.join("work"),
        ..job(&w)
    }
    .backup();
    assert_eq!(inside.code, 4, "a work folder inside the data is refused"); // WHAT: overlap. WHY: the spool would back itself up.
    let held = hold_lock(&w.work.join("view/offsite.lock")).unwrap();
    let busy = job(&w).backup();
    assert_eq!(busy.code, 2, "{busy:?}");
    assert!(busy.why.contains("still running"));
    drop(held); // WHAT: a run already going, holding the system's lock. WHY: never two at once.
    put(
        w.work.join("view/offsite.lock"),
        "left by a run that crashed",
    )
    .unwrap();
    let after = job(&w).backup();
    assert!(!after.why.contains("still running"), "a lock file nobody holds is no lock (the run goes on to the damaged database above): {after:?}");
    // WHAT: a crashed run's leftover file. WHY: the system let go of its lock when it died, whatever its age.
}

#[test]
fn a_bucket_that_says_yes_and_keeps_nothing_is_never_marked() {
    for folder in ["daily/", "blobs/"] {
        // WHAT: the catalog lost, then the files lost. WHY: each read-back is its own guard.
        let w = world();
        LOSE_UPLOADS.with(|l| l.set(Some(folder)));
        let e = job(&w).backup();
        LOSE_UPLOADS.with(|l| l.set(None)); // WHAT: a run against a bucket that loses what it is sent. WHY: the upload said yes.
        assert_eq!(e.code, 3, "{folder}: {e:?}");
        assert!(e.why.contains("not marking"), "{folder}: {}", e.why); // WHAT: refused as not sent. WHY: only what is read back counts.
        assert!(
            report(&w.data).last_good.is_none() && backup_days(&w.data).is_none(),
            "{folder}: never marked"
        ); // WHAT: no marker. WHY: Fleet stays amber until a copy is really there.
    }
}

#[test]
fn a_damaged_file_on_disk_is_never_sent_as_good_and_is_said() {
    let w = world();
    let files = list_blobs(&w.data.join("files")).unwrap();
    let (d, (p, _)) = files.iter().next().unwrap(); // WHAT: one stored file. WHY: damage it.
    std::fs::write(p, b"rotted bytes").unwrap();
    let e = job(&w).backup();
    assert_eq!(e.code, 5, "good with a warning: {e:?}"); // WHAT: the run still backs up the rest, and says so. WHY: one bad file must not stop every night, nor pass unnoticed.
    assert_eq!(
        std::fs::read_dir(w.bucket.join("blobs")).unwrap().count(),
        1,
        "the damaged one was not sent"
    );
    assert!(std::fs::read_to_string(w.work.join("view/offsite.log"))
        .unwrap()
        .contains(&format!("{:.12} on disk doesn't match", d))); // WHAT: not sent, and said. WHY: never damaged bytes as good ones.
}

#[test]
fn the_ops_report_believes_only_a_real_past_time_and_a_passed_drill() {
    let t = Tmp::new("ops"); // WHAT: a data folder. WHY: markers written by hand.
    std::fs::write(
        t.0.join(BACKUP_MARKER),
        r#"{"lastGood":"2999-01-01T00:00:00Z","where":"x"}"#,
    )
    .unwrap();
    assert_eq!(report(&t.0), Report::default());
    assert_eq!(backup_days(&t.0), None); // WHAT: a future time. WHY: not believed.
    std::fs::write(t.0.join(BACKUP_MARKER), "{not json").unwrap();
    assert_eq!(backup_days(&t.0), None); // WHAT: a damaged file. WHY: unknown.
    std::fs::write(
        t.0.join(BACKUP_MARKER),
        r#"{"lastGood":"2026-10-30T00:00:00Z","where":"off-site bucket r2:x"}"#,
    )
    .unwrap();
    assert!(backup_days(&t.0).unwrap() > 1.0);
    assert_eq!(
        report(&t.0).r#where.as_deref(),
        Some("off-site bucket r2:x")
    ); // WHAT: a real one. WHY: read as days.
    std::fs::write(
        t.0.join(DRILL_MARKER),
        r#"{"ok":false,"at":"2026-10-30T00:00:00Z"}"#,
    )
    .unwrap();
    assert_eq!(
        report(&t.0).restore_tested,
        None,
        "a failed drill proves nothing"
    ); // WHAT: not a pass. WHY: never counted.
    std::fs::write(
        t.0.join(DRILL_MARKER),
        r#"{"ok":true,"at":"2026-10-30T00:00:00Z"}"#,
    )
    .unwrap();
    assert!(report(&t.0).restore_tested.is_some()); // WHAT: a pass. WHY: counted.
}

#[test]
fn a_locked_key_is_opened_once_beside_itself_and_never_left_behind() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0); // WHAT: a backup to drill. WHY: the setup.
    let stick = w.root.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    let locked = stick.join("key.age"); // WHAT: Creede's USB stick. WHY: where the locked key lives.
    let mut b = b"FAKEPASS1\n".to_vec();
    b.extend(std::fs::read(&w.id).unwrap());
    std::fs::write(&locked, b).unwrap(); // WHAT: the key, locked. WHY: as Creede keeps it.
    let e = drill(&w, &locked, "s-locked").run();
    assert_eq!(e.code, 0, "{e:?}"); // WHAT: a full drill with it. WHY: every file opens with one passphrase.
    assert_eq!(
        std::fs::read_dir(&stick).unwrap().count(),
        1,
        "the opened key is deleted from the stick"
    ); // WHAT: only the locked key left. WHY: the opened one opens every backup.
    std::fs::write(&locked, b"FAKEPASS1\nscrambled: the wrong passphrase").unwrap();
    let bad = drill(&w, &locked, "s-locked2").run();
    assert_eq!(bad.code, 2);
    assert!(bad.why.contains("wrong passphrase")); // WHAT: it won't open. WHY: stops at once.
    assert_eq!(
        std::fs::read_dir(&stick).unwrap().count(),
        1,
        "nothing left after a failure"
    );
    assert!(!w.root.join("s-locked2").exists()); // WHAT: tidy. WHY: the same.
}

/// Every file under `dir` is owner read and write only, and every folder owner only (Unix; Windows folders take their parent's rules).
fn owner_only(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "the scratch folder itself"
        );
        let mut n = 0;
        for f in files_in(dir) {
            let m = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600, "{} is {m:o}", f.display());
            n += 1;
            let d = f.parent().unwrap();
            let dm = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(dm, 0o700, "{} is {dm:o}", d.display());
        }
        assert!(n > 0, "something was restored");
    } // WHAT: check each file's mode and its folder's. WHY: never readable by others, even for a moment.
    #[cfg(not(unix))]
    let _ = dir; // WHAT: nothing to check. WHY: Windows modes come from the folder's rules.
}

#[test]
fn no_object_name_reaches_outside_the_target_and_no_remote_is_an_option() {
    let t = Tmp::new("names");
    let target = Target::Folder(t.0.join("bucket"));
    let src = t.0.join("src.txt");
    std::fs::write(&src, "x").unwrap(); // WHAT: a folder target and a file to send. WHY: names are attacked below.
    for bad in [
        "../evil",
        "/abs",
        "daily/../../evil",
        "daily//x",
        "./x",
        "daily/",
        "",
        "a\\b",
        "C:evil",
        "daily/x y",
    ] {
        assert!(target.put(&src, bad).is_err(), "{bad:?} must be refused");
        assert!(target.get(bad, &t.0.join("got")).is_err(), "{bad:?} get");
        assert!(target.delete(bad).is_err(), "{bad:?} delete");
    } // WHAT: each way out. WHY: the keep rule and the drill can never touch a path outside.
    assert!(
        !t.0.join("evil").exists() && !Path::new("/abs").exists(),
        "nothing landed outside"
    ); // WHAT: nothing escaped. WHY: the claim.
    assert!(
        target.put(&src, "daily/ok.txt").is_ok() && t.0.join("bucket/daily/ok.txt").exists(),
        "a plain name lands inside"
    ); // WHAT: the normal case. WHY: the check isn't refusing everything.
    assert!(
        target.put(&src, "daily/ok.txt").is_err(),
        "never written over"
    ); // WHAT: a second send of the same name. WHY: what was sent stays as it was sent.
    let r = Target::parse("rclone:r2:bucket/view", &t.0).unwrap();
    assert!(
        r.get_many("blobs", &["a/b".to_string()], &t.0.join("dl"))
            .is_err(),
        "a name with a folder in a download list"
    ); // WHAT: the list rclone reads. WHY: one name, one object.
    for bad in [
        "rclone:--config=/tmp/x:b",
        "rclone:-v:b",
        "rclone: r2:b",
        "rclone:r2",
        "rclone:r2:../b",
        "rclone:r2:b c",
        "rclone:r2,access_key_id=AKIA0:b",
        "rclone:s3,env_auth=true:b/v",
        "rclone:r2:b=1",
    ] {
        assert!(Target::parse(bad, &t.0).is_err(), "{bad:?} must be refused");
    } // WHAT: remotes rclone would read as an option, or malformed. WHY: rclone runs as the target says, nothing more.
}

#[test]
fn the_settings_are_checked_as_the_server_starts_and_the_night_comes_once_a_day() {
    for good in ["01:30", "1:30", "23:59", "00:00", " 02:15 "] {
        assert!(night_time(good).is_ok(), "{good:?}");
    } // WHAT: times that read. WHY: 24-hour, with or without a leading zero.
    for bad in [
        "1.30", "1:30am", "25:00", "01:61", "130", "", "01:3", "-1:30", "+1:30",
    ] {
        assert!(night_time(bad).is_err(), "{bad:?}");
    } // WHAT: times refused. WHY: never guessed.
    let t = Tmp::new("set");
    let data = t.0.join("data"); // WHAT: a data folder. WHY: the work folder must be apart from it.
    assert!(
        check_settings(&data, None, None, "01:30").is_ok(),
        "off is fine"
    ); // WHAT: nothing set. WHY: a server without a backup still starts.
    assert!(
        check_settings(&data, None, None, "1:30am").is_err(),
        "a bad time is refused even when off"
    ); // WHAT: a typo. WHY: found now.
    assert!(
        check_settings(&data, None, Some("rclone:r2:b/v"), "01:30").is_err(),
        "a target with no work folder"
    ); // WHAT: half set. WHY: can't run.
    assert!(
        check_settings(
            &data,
            Some(&data.join("offsite").to_string_lossy()),
            Some("rclone:r2:b/v"),
            "01:30"
        )
        .is_err(),
        "a work folder inside the data"
    ); // WHAT: overlap. WHY: it would back itself up.
    assert!(
        check_settings(
            &data,
            Some(&t.0.to_string_lossy()),
            Some("rclone:r2:b/v"),
            "01:30"
        )
        .is_err(),
        "the data folder inside the work folder"
    ); // WHAT: overlap the other way. WHY: the same.
    assert!(
        check_settings(
            &data,
            Some(&t.0.join("work").to_string_lossy()),
            Some("rclone:-v:b"),
            "01:30"
        )
        .is_err(),
        "a bad remote"
    ); // WHAT: refused at start. WHY: not at night.
    assert!(
        check_settings(
            &data,
            Some(&t.0.join("work").to_string_lossy()),
            Some("rclone:r2:b/v"),
            "01:30"
        )
        .is_ok(),
        "a good setting"
    ); // WHAT: the normal case. WHY: the check isn't refusing everything.
    let at = night_time("01:30").unwrap();
    let d = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
    let tm = |h, m| d.and_hms_opt(h, m, 0).unwrap(); // WHAT: a night. WHY: readable cases.
    assert!(!due(tm(1, 0), at, None, None), "not before the time");
    assert!(
        due(tm(1, 30), at, None, None),
        "at the time, never backed up"
    ); // WHAT: before and at. WHY: from the time on.
    assert!(
        due(
            tm(9, 0),
            at,
            Some(tm(1, 29) - chrono::Duration::days(1)),
            None
        ),
        "a server off at night catches up in the morning"
    ); // WHAT: yesterday's was the last. WHY: one a day, even late.
    assert!(!due(tm(3, 0), at, Some(tm(1, 31)), None), "done tonight");
    assert!(!due(tm(23, 59), at, Some(tm(1, 31)), None), "done today"); // WHAT: already done. WHY: once a day.
    assert!(
        !due(tm(2, 0), at, None, Some(tm(1, 30))),
        "a failure waits an hour"
    );
    assert!(
        due(tm(2, 31), at, None, Some(tm(1, 30))),
        "then tries again"
    ); // WHAT: the retry. WHY: gently, not every look.
}

#[test]
fn the_command_refuses_what_it_doesnt_know_and_says_when_it_isnt_set_up() {
    let w = world();
    let cfg = crate::Config {
        data: w.data.clone(),
        ..crate::Config::default()
    };
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>(); // WHAT: a server with no off-site settings. WHY: the command reads the server's settings.
    assert_eq!(
        main_from_args(&VIEW, &cfg, "9.9.9", &a(&["--offsite-backup"])),
        4,
        "not set up"
    ); // WHAT: nothing to do. WHY: says so, sends nothing.
    assert_eq!(
        main_from_args(
            &VIEW,
            &cfg,
            "9.9.9",
            &a(&[
                "--offsite-backup",
                &w.bucket.to_string_lossy(),
                "--age",
                "/usr/bin/age"
            ])
        ),
        4,
        "no --age: age is only ever the one in the work folder"
    ); // WHAT: an option that would pick another program. WHY: never whatever is elsewhere.
    assert_eq!(
        main_from_args(
            &VIEW,
            &cfg,
            "9.9.9",
            &a(&[
                "--offsite-backup",
                &w.bucket.to_string_lossy(),
                "--wrok",
                "x"
            ])
        ),
        4,
        "a typo"
    ); // WHAT: a mistyped option. WHY: never ignored.
    assert_eq!(
        main_from_args(
            &VIEW,
            &cfg,
            "9.9.9",
            &a(&[
                "--offsite-drill",
                &w.bucket.to_string_lossy(),
                "--work",
                &w.work.to_string_lossy(),
                "--identity",
                "k"
            ])
        ),
        4,
        "a drill needs a scratch folder"
    ); // WHAT: half a drill. WHY: refused before anything.
    assert_eq!(
        main_from_args(
            &VIEW,
            &cfg,
            "9.9.9",
            &a(&[
                "--offsite-backup",
                &w.bucket.to_string_lossy(),
                "--work",
                &w.work.to_string_lossy()
            ])
        ),
        2,
        "age isn't in the work folder's bin, so refused"
    ); // WHAT: the real command, with no age beside it. WHY: never age from the PATH, even where one is installed.
    assert!(
        !w.bucket.exists() && report(&w.data).last_good.is_none(),
        "nothing sent, nothing marked"
    ); // WHAT: refused before anything. WHY: the claim.
}

const WAIT: std::time::Duration = std::time::Duration::from_millis(1100); // WHAT: past the next second. WHY: catalog names are to the second.

/// Re-packs tonight's catalog under `name`, its manifest changed by `edit` and padded by `pad` bytes, with a sidecar to
/// match, as someone who could write to the bucket (but not read it without the key) might plant it.
fn plant(w: &World, name: &str, edit: impl FnOnce(&mut serde_json::Value), pad: usize) {
    let cat = files_in(&w.bucket.join("daily"))
        .into_iter()
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".tar.gz.age")
        })
        .unwrap(); // WHAT: a real catalog. WHY: its database is reused.
    let plain = fake::open(
        &std::fs::read_to_string(&w.id).unwrap(),
        &std::fs::read(&cat).unwrap(),
    )
    .unwrap();
    let t = Tmp::new("plant");
    untar(
        &mut flate2::read::GzDecoder::new(&plain[..]),
        &t.0.join("x"),
        1 << 30,
        EXPECT,
    )
    .unwrap(); // WHAT: opened. WHY: to change its manifest.
    let mut m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(t.0.join("x/manifest.json")).unwrap()).unwrap();
    m["name"] = name.into();
    edit(&mut m);
    let mut mj = serde_json::to_vec(&m).unwrap();
    mj.extend(std::iter::repeat_n(b' ', pad)); // WHAT: the changed manifest. WHY: each case below.
    let db = std::fs::read(t.0.join("x/hyperview.sqlite")).unwrap();
    let mut buf = Vec::new();
    {
        let mut gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::fast());
        {
            let mut tw = TarOut { w: &mut gz };
            tw.file("manifest.json", mj.len() as u64, &mut &mj[..])
                .unwrap();
            tw.file("hyperview.sqlite", db.len() as u64, &mut &db[..])
                .unwrap();
            tw.end().unwrap();
        }
        gz.finish().unwrap();
    } // WHAT: packed as the job packs. WHY: only the manifest differs.
    let sealed = fake::seal(
        &std::fs::read_to_string(w.work.join("age-recipients.txt")).unwrap(),
        &buf,
    );
    let out = w.bucket.join("daily").join(name);
    std::fs::write(&out, &sealed).unwrap(); // WHAT: locked to the same key. WHY: it opens like a real one.
    let side = Sidecar {
        format: "excalibur-offsite-1".into(),
        name: name.into(),
        created_at: "2026-11-08T00:00:00Z".into(),
        app_version: "9.9.9".into(),
        age_sha256: sha256_file(&out).unwrap(),
        age_bytes: sealed.len() as u64,
        blob_count: 0,
        blob_bytes: 0,
    };
    std::fs::write(
        w.bucket.join("daily").join(format!("{name}.json")),
        serde_json::to_vec(&side).unwrap(),
    )
    .unwrap(); // WHAT: a matching sidecar. WHY: it passes the fingerprint check.
}

#[test]
fn a_planted_manifest_or_an_oversized_sidecar_is_refused_before_it_steers_anything() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0); // WHAT: a real backup. WHY: plants are made from it.
    let soon = |m: i64| {
        object_name(
            "view",
            time::OffsetDateTime::now_utc() + time::Duration::minutes(m),
        )
    }; // WHAT: a name newer than the real ones. WHY: the drill picks the newest.
    type Edit = Box<dyn FnOnce(&mut serde_json::Value)>; // WHAT: one change to a manifest. WHY: named, so the list reads plainly.
    let cases: Vec<(&str, Edit, usize)> = vec![
        // WHAT: each planted manifest. WHY: each must be refused.
        (
            "a file named outside",
            Box::new(|m| m["blobs"][0][0] = "../../../evil".into()),
            0,
        ),
        (
            "a count missing",
            Box::new(|m| {
                m["counts"].as_object_mut().unwrap().remove("sets");
            }),
            0,
        ),
        (
            "a stranger's count",
            Box::new(|m| {
                m["counts"]["planted"] = 1.into();
            }),
            0,
        ),
        ("a short salt", Box::new(|m| m["salt"] = "abc".into()), 0),
        (
            "a database fingerprint in capitals",
            Box::new(|m| m["databaseSha256"] = "A".repeat(64).into()),
            0,
        ),
        (
            "a manifest over 64 MB",
            Box::new(|_| {}),
            64 * 1024 * 1024 + 1,
        ),
    ];
    for (i, (why, edit, pad)) in cases.into_iter().enumerate() {
        let n = soon(10 + i64::try_from(i).unwrap()); // WHAT: a minute apart for each case. WHY: each planted name is its own
        plant(&w, &n, edit, pad);
        let e = drill(&w, &w.id, &format!("s-plant{i}")).run();
        assert_eq!(e.code, 2, "{why}: {e:?}");
        assert!(e.why.contains("manifest"), "{why}: {}", e.why);
        assert!(
            !w.root.join(format!("s-plant{i}")).exists(),
            "{why}: nothing left"
        );
        std::fs::remove_file(w.bucket.join("daily").join(&n)).unwrap();
        std::fs::remove_file(w.bucket.join("daily").join(format!("{n}.json"))).unwrap();
    } // WHAT: refused, nothing left, then taken away. WHY: one plant at a time.
    let n = soon(30);
    plant(&w, &n, |_| {}, 0);
    let side = w.bucket.join("daily").join(format!("{n}.json"));
    let mut big = std::fs::read(&side).unwrap();
    big.extend(std::iter::repeat_n(b' ', 64 * 1024));
    std::fs::write(&side, big).unwrap(); // WHAT: a sidecar over 64 KB. WHY: refused unread.
    let e = drill(&w, &w.id, "s-bigside").run();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(
        e.why.contains("sidecar") && e.why.contains("not fetched"),
        "{}",
        e.why
    ); // WHAT: refused from the listing, before fetching. WHY: a giant sidecar never comes down.
    std::fs::remove_file(&side).unwrap();
    std::fs::remove_file(w.bucket.join("daily").join(&n)).unwrap(); // WHAT: that plant gone. WHY: the next one.
    let n = soon(40);
    plant(&w, &n, |_| {}, 0);
    let side = w.bucket.join("daily").join(format!("{n}.json"));
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&side).unwrap()).unwrap();
    v["ageBytes"] = (v["ageBytes"].as_u64().unwrap() + 1).into();
    std::fs::write(&side, serde_json::to_vec(&v).unwrap()).unwrap(); // WHAT: a catalog listed at another size than its sidecar says. WHY: a hostile catalog could be any size.
    let e = drill(&w, &w.id, "s-bigcat").run();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(
        e.why.contains("catalog is listed") && e.why.contains("not fetched"),
        "{}",
        e.why
    ); // WHAT: refused before fetching. WHY: the scratch drive is never filled.
    std::fs::remove_file(&side).unwrap();
    std::fs::remove_file(w.bucket.join("daily").join(&n)).unwrap();
    assert_eq!(
        drill(&w, &w.id, "s-after").run().code,
        0,
        "the real one still drills"
    ); // WHAT: the plants gone. WHY: the checks refuse only what is wrong.
}

#[test]
fn a_catalog_dated_in_the_future_is_never_the_one_the_drill_picks() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0); // WHAT: a real backup. WHY: drilled below.
    let real = files_in(&w.bucket.join("daily"))
        .into_iter()
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".tar.gz.age")
        })
        .unwrap(); // WHAT: tonight's catalog. WHY: copied under a far-future name.
    let fut = "view-20990101-013000.tar.gz.age";
    std::fs::copy(&real, w.bucket.join("daily").join(fut)).unwrap();
    std::fs::copy(
        real.with_extension("age.json"),
        w.bucket.join("daily").join(format!("{fut}.json")),
    )
    .unwrap(); // WHAT: a planted "newest". WHY: a drill must not be steered to it.
    let e = drill(&w, &w.id, "s-future").run();
    assert_eq!(e.code, 0, "the real newest is drilled: {e:?}");
    assert!(
        !std::fs::read_to_string(w.work.join("view/offsite.log"))
            .unwrap()
            .contains("downloaded daily/view-2099"),
        "the 2099 one was never fetched"
    ); // WHAT: the real one. WHY: a name from the future is not believed.
}

#[test]
fn a_run_by_hand_to_another_target_marks_nothing() {
    let t = Tmp::new("marks");
    let (wk, other) = (t.0.join("work"), t.0.join("other"));
    create_private_dir(&wk).unwrap();
    create_private_dir(&other).unwrap();
    let (wk, other) = (
        wk.to_string_lossy().into_owned(),
        other.to_string_lossy().into_owned(),
    ); // WHAT: the set work folder and another. WHY: compared as places.
    assert!(
        marks(
            Some("rclone:r2:b/v"),
            Some(&wk),
            Some("rclone:r2:b/v"),
            None
        ) && marks(
            Some(" rclone:r2:b/v/ "),
            Some(&wk),
            None,
            Some(&format!("{wk}/./"))
        ),
        "the server's own target and work folder mark"
    ); // WHAT: the same, however written. WHY: the nightly copy.
    assert!(
        !marks(Some("rclone:r2:b/v"), Some(&wk), Some("E:/usb"), None)
            && !marks(None, None, Some("E:/usb"), None)
            && !marks(Some(""), None, Some(""), None)
            && !marks(None, None, None, None),
        "another target, or none set"
    ); // WHAT: refused. WHY: a USB disk tried once is not the off-site copy.
    assert!(
        !marks(Some("rclone:r2:b/v"), Some(&wk), None, Some(&other))
            && !marks(Some("rclone:r2:b/v"), None, None, Some(&wk)),
        "another work folder doesn't"
    ); // WHAT: --work elsewhere. WHY: its own salt and sizes say nothing of the nightly copy.
    let folder = t.0.join("bucket");
    assert!(
        marks(
            Some(&folder.to_string_lossy()),
            None,
            Some(&format!("{}/../bucket", wk)),
            None
        ),
        "a folder target is the same place by any path"
    ); // WHAT: two spellings of one folder. WHY: compared as real places.
    let w = world();
    let e = Job {
        mark: false,
        ..job(&w)
    }
    .backup();
    assert_eq!(e.code, 0, "{e:?}");
    assert!(
        report(&w.data).last_good.is_none() && backup_days(&w.data).is_none(),
        "backupDays doesn't move"
    ); // WHAT: a good run elsewhere. WHY: sent, not marked.
    let d = Drill {
        job: Job {
            mark: false,
            ..job(&w)
        },
        ..drill(&w, &w.id, "s-other")
    }
    .run();
    assert_eq!(d.code, 0, "{d:?}");
    assert!(
        report(&w.data).restore_tested.is_none(),
        "restoreTested doesn't move"
    ); // WHAT: a drill of it. WHY: the same.
    std::thread::sleep(WAIT);
    assert_eq!(job(&w).backup().code, 0);
    assert!(report(&w.data).last_good.is_some(), "its own target marks"); // WHAT: the normal case. WHY: the rule isn't refusing everything.
}

#[test]
fn an_object_of_another_size_is_not_the_one_sent() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0);
    let first = report(&w.data).last_good; // WHAT: a good backup. WHY: its mark must not move below.
    let blob = files_in(&w.bucket.join("blobs")).remove(0);
    let good = std::fs::read(&blob).unwrap();
    let mut longer = good.clone();
    longer.push(0);
    std::fs::write(&blob, &longer).unwrap(); // WHAT: an object a byte longer than sent. WHY: "bigger than the plain file" is not proof.
    std::thread::sleep(WAIT);
    let e = job(&w).backup();
    assert_eq!(e.code, 3, "{e:?}");
    assert!(
        e.why.contains("size they were sent")
            && e.why.contains("blobs/")
            && e.why.contains("Delete each"),
        "{}",
        e.why
    );
    assert_eq!(report(&w.data).last_good, first, "not marked"); // WHAT: refused. WHY: only the exact size sent counts.
    std::fs::write(&blob, &good).unwrap();
    std::fs::remove_file(w.work.join("view/blob-sizes.json")).unwrap();
    std::thread::sleep(WAIT);
    let e = job(&w).backup();
    assert_eq!(e.code, 3, "{e:?}");
    assert!(e.why.contains("no record"), "{}", e.why); // WHAT: the sizes lost. WHY: an unknown size is never taken as right.
}

#[test]
fn a_fault_inside_a_run_is_a_failed_run_reported_and_cleaned_up() {
    let w = world();
    FAULT.with(|f| f.set(true));
    let e = job(&w).backup();
    FAULT.with(|f| f.set(false)); // WHAT: a fault inside the run. WHY: it must not escape.
    assert_eq!(e.code, 3, "{e:?}");
    assert!(e.why.contains("stopped part way"), "{}", e.why);
    assert!(report(&w.data).last_good.is_none(), "never marked"); // WHAT: a failed run. WHY: the night reports it.
    assert!(
        std::fs::read_to_string(w.work.join("view/offsite.log"))
            .unwrap()
            .contains("FAILED"),
        "the log says so"
    );
    assert!(!w.work.join("view/stage").exists(), "no plaintext left");
    assert_eq!(job(&w).backup().code, 0, "and the lock was let go"); // WHAT: cleaned up. WHY: tomorrow runs.
}

#[test]
fn a_file_changed_while_it_is_sent_is_not_sent_and_the_run_says_so() {
    let w = world();
    CHANGE_WHILE_SENT.with(|c| c.set(true));
    let e = job(&w).backup();
    CHANGE_WHILE_SENT.with(|c| c.set(false)); // WHAT: every file rewritten after its check. WHY: the fingerprint taken while sending decides.
    assert_eq!(e.code, 5, "good with a warning: {e:?}");
    assert!(
        files_in(&w.bucket.join("blobs")).is_empty(),
        "nothing sent under a name its bytes don't have"
    ); // WHAT: not sent. WHY: an object holds exactly what its name says.
    assert!(
        std::fs::read_to_string(w.work.join("view/offsite.log"))
            .unwrap()
            .contains("changed while it was being sent"),
        "the log says so"
    ); // WHAT: said. WHY: never hidden.
}

#[test]
fn a_private_key_anywhere_in_the_work_folder_is_refused() {
    let w = world();
    create_private_dir(&w.work.join("bin")).unwrap();
    put(
        w.work.join("bin/age"),
        format!("a program naming its own format: {}", secret_line()),
    )
    .unwrap(); // WHAT: age's program, which holds the words. WHY: bin is skipped.
    assert_eq!(job(&w).backup().code, 0, "bin is age's own");
    create_private_dir(&w.work.join("old")).unwrap();
    put(
        w.work.join("old/key.txt"),
        format!("# created\n{}\n", secret_line().to_lowercase()),
    )
    .unwrap(); // WHAT: a key left in a folder of the work folder. WHY: any case, any depth.
    std::thread::sleep(WAIT);
    let e = job(&w).backup();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(
        e.why.contains("PRIVATE key") && e.why.contains("key.txt"),
        "{}",
        e.why
    ); // WHAT: refused, naming the file. WHY: it opens every backup.
}

#[test]
fn a_link_inside_the_scratch_folder_is_found() {
    let t = Tmp::new("links");
    std::fs::create_dir_all(t.0.join("a/b")).unwrap();
    std::fs::write(t.0.join("a/b/f"), "x").unwrap();
    assert_eq!(link_inside(&t.0), None, "plain files and folders"); // WHAT: a clean restore. WHY: nothing found.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", t.0.join("a/b/l")).unwrap();
        assert_eq!(
            link_inside(&t.0),
            Some(t.0.join("a/b/l")),
            "a link deep inside"
        );
    } // WHAT: a link. WHY: nothing the drill opens may point elsewhere.
}

#[test]
fn a_scratch_folder_opened_up_after_it_is_made_is_refused() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0);
    LOOSEN_SCRATCH.with(|l| l.set(true));
    let e = drill(&w, &w.id, "s-loose").run();
    LOOSEN_SCRATCH.with(|l| l.set(false)); // WHAT: a scratch folder open to others once made. WHY: what another account could do in the gap.
    assert_eq!(e.code, 4, "{e:?}");
    #[cfg(unix)]
    assert!(e.why.contains("not 0700"), "{}", e.why); // WHAT: refused for its mode. WHY: what Unix says.
    #[cfg(windows)]
    assert!(e.why.contains("lets in S-1-5-32-545"), "{}", e.why); // WHAT: refused for the extra entry. WHY: what Windows says when every account was let in.
    assert!(!w.root.join("s-loose").exists(), "and taken away"); // WHAT: refused before anything is restored. WHY: plaintext only in a folder that is ours alone.
}

#[test]
fn a_folder_the_link_check_cannot_read_is_a_refusal() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let t = Tmp::new("closed");
        let shut = t.0.join("shut");
        std::fs::create_dir_all(&shut).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = link_inside(&t.0);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(found, Some(shut), "fails closed");
    } // WHAT: a folder it can't look into. WHY: what can't be checked isn't trusted.
}

#[test]
fn a_work_folder_others_could_change_is_refused() {
    #[cfg(windows)]
    {
        let w = world();
        assert_eq!(job(&w).backup().code, 0, "the locked folder is fine"); // WHAT: the folder the server locked. WHY: the baseline.
        let open = |p: &Path, how: &str| {
            let ok = std::process::Command::new("icacls")
                .arg(p)
                .args(["/grant", &format!("*S-1-5-32-545:{how}")])
                .output()
                .unwrap();
            assert!(ok.status.success(), "icacls: {ok:?}");
        }; // WHAT: let every account (BUILTIN\Users) change it, with Windows' own tool. WHY: what a folder View didn't lock, or someone opening it later, looks like.
        for (what, path, how) in [
            ("the work folder", w.work.clone(), "(OI)(CI)M"),
            ("the public key file", w.work.join("age-recipients.txt"), "M"),
        ] {
            open(&path, how);
            std::thread::sleep(WAIT); // WHAT: open one to other accounts. WHY: each is checked.
            let e = job(&w).backup();
            let d = drill(&w, &w.id, &format!("s-open-{}", what.len())).run();
            assert_eq!(e.code, 2, "{what}: {e:?}");
            assert!(e.why.contains("can be changed by S-1-5-32-545"), "{what}: {}", e.why);
            assert_eq!(d.code, 2, "{what}: the drill too: {d:?}"); // WHAT: a backup and a drill both refuse. WHY: whoever can swap age or rclone.conf can read or redirect every backup.
            lock_work(&w.work); // WHAT: the server's lock again, as at its next start. WHY: it takes the extra entry off the whole tree.
            std::thread::sleep(WAIT);
            assert_eq!(job(&w).backup().code, 0, "{what}: locked again, it runs"); // WHAT: back to good. WHY: the lock, not luck, is what made it pass.
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let w = world();
        create_private_dir(&w.work.join("bin")).unwrap();
        put(w.work.join("rclone.conf"), "[r2]\n").unwrap();
        std::fs::set_permissions(
            w.work.join("rclone.conf"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(job(&w).backup().code, 0, "owner-only is fine"); // WHAT: a good work folder. WHY: the baseline.
        for (what, path, mode) in [
            ("the work folder", w.work.clone(), 0o770),
            ("bin", w.work.join("bin"), 0o777),
            ("rclone.conf", w.work.join("rclone.conf"), 0o666),
        ] {
            let was = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            std::thread::sleep(WAIT); // WHAT: open one to other accounts. WHY: each is checked.
            let e = job(&w).backup();
            let d = drill(&w, &w.id, &format!("s-open-{}", mode)).run();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(was)).unwrap(); // WHAT: a backup and a drill, then put back. WHY: both refuse.
            assert_eq!(e.code, 2, "{what}: {e:?}");
            assert!(
                e.why.contains("changed by other accounts"),
                "{what}: {}",
                e.why
            );
            assert_eq!(d.code, 2, "{what}: the drill too: {d:?}");
        } // WHAT: refused. WHY: whoever can swap age or rclone.conf can read or redirect every backup.
        for (what, path, mode, says) in [
            (
                "a program in bin",
                w.work.join("bin/age"),
                0o664,
                "chmod go-w",
            ),
            (
                "the public key file",
                w.work.join("age-recipients.txt"),
                0o666,
                "chmod go-w",
            ),
            (
                "rclone.conf readable by others",
                w.work.join("rclone.conf"),
                0o644,
                "chmod 600",
            ),
        ] {
            put(&path, std::fs::read(&path).unwrap_or_default()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            std::thread::sleep(WAIT); // WHAT: one file anywhere in the tree opened up. WHY: the whole tree is checked, not only the folder, bin and rclone.conf.
            let e = job(&w).backup();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(e.code, 2, "{what}: {e:?}");
            assert!(e.why.contains(says), "{what}: {}", e.why);
        } // WHAT: refused, saying how to fix it. WHY: as above.
        std::os::unix::fs::symlink("/etc/hostname", w.work.join("bin/rclone")).unwrap();
        std::thread::sleep(WAIT);
        let e = job(&w).backup();
        std::fs::remove_file(w.work.join("bin/rclone")).unwrap();
        assert_eq!(e.code, 2, "{e:?}");
        assert!(e.why.contains("is a link"), "{}", e.why); // WHAT: a link in the work folder. WHY: never followed, always refused.
        PRETEND_UID.with(|c| c.set(Some(4242)));
        std::thread::sleep(WAIT);
        let e = job(&w).backup();
        PRETEND_UID.with(|c| c.set(None));
        assert_eq!(e.code, 2, "{e:?}");
        assert!(e.why.contains("is owned by account"), "{}", e.why); // WHAT: every file a stranger's. WHY: an owner can always change its file back.
        std::thread::sleep(WAIT);
        assert_eq!(
            job(&w).backup().code,
            0,
            "and the job's own files pass, night after night"
        );
    } // WHAT: the normal case after all that. WHY: the log, salt, sizes and spool are made owner-only.
}

#[test]
fn a_batch_cut_short_doesnt_block_the_nights_after() {
    let w = world();
    FAIL_PUT_AFTER.with(|f| f.set(Some(1)));
    let e = job(&w).backup();
    FAIL_PUT_AFTER.with(|f| f.set(None));
    assert_eq!(e.code, 3, "{e:?}"); // WHAT: the connection drops after one file. WHY: a failed batch.
    std::thread::sleep(WAIT);
    let e = job(&w).backup();
    assert_eq!(e.code, 0, "the next night finishes: {e:?}");
    assert!(report(&w.data).last_good.is_some()); // WHAT: the next night. WHY: the sizes were kept before sending, so nothing is unknown.
}

#[test]
fn a_failed_night_reaches_the_ops_report_until_a_good_one() {
    let w = world();
    std::fs::write(&w.bucket, "a file where the target folder should be").unwrap();
    let e = job(&w).backup();
    assert_eq!(e.code, 3, "{e:?}"); // WHAT: a failed night. WHY: the marker.
    let r = report(&w.data);
    assert!(
        r.last_failed.is_some()
            && r.last_failed_why
                .as_deref()
                .is_some_and(|y| y.contains("failed")),
        "{r:?}"
    );
    assert!(r.last_good.is_none()); // WHAT: when and why. WHY: Fleet sees it.
    std::fs::remove_file(&w.bucket).unwrap();
    let held = hold_lock(&w.work.join("view/offsite.lock")).unwrap();
    let before = report(&w.data).last_failed;
    assert_eq!(job(&w).backup().code, 2);
    drop(held);
    assert_eq!(
        report(&w.data).last_failed,
        before,
        "a run already going is not a failure"
    ); // WHAT: refused by the lock. WHY: not a failed night.
    assert_eq!(job(&w).backup().code, 0);
    let r = report(&w.data);
    assert!(
        r.last_failed.is_none() && r.last_failed_why.is_none(),
        "a good night clears it"
    ); // WHAT: cleared. WHY: the failure is over.
    std::fs::remove_dir_all(&w.bucket).unwrap();
    std::fs::write(&w.bucket, "x").unwrap();
    assert_eq!(
        Job {
            mark: false,
            ..job(&w)
        }
        .backup()
        .code,
        3
    );
    assert!(
        report(&w.data).last_failed.is_none(),
        "a hand run elsewhere marks no failure either"
    ); // WHAT: another target. WHY: says nothing of the nightly copy.
}

#[test]
fn a_stopped_drill_leaves_no_key_and_a_leftover_one_is_cleared() {
    let t = Tmp::new("secrets");
    let key = t.0.join(".drill-key-abc.txt");
    let dir = t.0.join("restore");
    std::fs::write(&key, "opened").unwrap();
    create_private_dir(&dir.join("restored")).unwrap();
    hold_secret(&key);
    hold_secret(&dir); // WHAT: an opened key and a restore, noted. WHY: what Ctrl+C deletes.
    delete_secrets(Some(&t.0));
    assert!(!key.exists() && !dir.exists(), "both deleted"); // WHAT: the stop handler's work. WHY: neither outlives the drill.
    assert!(
        key_beside(Path::new("key.age")).unwrap().is_absolute(),
        "an opened key's place is a full path"
    ); // WHAT: a relative key file. WHY: never whatever folder this was started from.
    let w = world();
    assert_eq!(job(&w).backup().code, 0);
    let stick = w.root.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    let id = stick.join("id.txt");
    std::fs::copy(&w.id, &id).unwrap();
    std::fs::write(
        stick.join(".drill-key-0a0b0c0d0e0f.txt"),
        "left by a drill stopped earlier",
    )
    .unwrap(); // WHAT: a leftover opened key beside the key. WHY: an earlier drill was killed.
    assert_eq!(drill(&w, &id, "s-left").run().code, 0);
    assert!(
        !stick.join(".drill-key-0a0b0c0d0e0f.txt").exists(),
        "deleted"
    );
    assert!(
        std::fs::read_to_string(w.work.join("view/offsite.log"))
            .unwrap()
            .contains("left behind"),
        "and said"
    ); // WHAT: cleared at the start. WHY: it opens every backup.
}

#[test]
fn a_folder_target_is_a_full_path() {
    let t = Target::parse("some/relative/folder", Path::new("/w")).unwrap();
    assert!(
        matches!(t, Target::Folder(ref p) if p.is_absolute()),
        "made absolute"
    ); // WHAT: a relative folder. WHY: one place wherever this was started.
}

// ---------------------------------------------------------------- real age, where it is installed

fn real_age() -> Option<(PathBuf, PathBuf)> {
    let dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default(); // WHAT: the PATH. WHY: where age would be.
    let find = |n: &str| {
        dirs.iter()
            .map(|d| {
                d.join(if cfg!(windows) {
                    format!("{n}.exe")
                } else {
                    n.to_string()
                })
            })
            .find(|p| p.is_file())
    }; // WHAT: a program on it. WHY: the test needs both.
    Some((find("age")?, find("age-keygen")?)) // WHAT: age and its key maker. WHY: the real thing.
}

#[test]
fn with_real_age_the_backup_cannot_be_read_without_the_private_key() {
    let Some((age, keygen)) = real_age() else {
        eprintln!("NOTE: age is not installed here; the real-age proof did not run on this machine (it runs on the build server).");
        return;
    }; // WHAT: skip only where age is absent. WHY: CI machines without it still run every other test.
    let w = world();
    let key = w.root.join("real-key.txt");
    let other = w.root.join("other-key.txt"); // WHAT: two real key pairs. WHY: the right one and a wrong one.
    for k in [&key, &other] {
        assert!(Command::new(&keygen)
            .arg("-o")
            .arg(k)
            .output()
            .unwrap()
            .status
            .success());
    } // WHAT: make them. WHY: as Creede does on his own machine.
    let pubk = std::fs::read_to_string(&key)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("# public key: ").map(str::to_string))
        .unwrap();
    put(w.work.join("age-recipients.txt"), format!("{pubk}\n")).unwrap(); // WHAT: only the public line on the "server". WHY: the design.
    let real = |w: &World| Job {
        cipher: Cipher::Age(age.clone()),
        ..job(w)
    }; // WHAT: the job with real age. WHY: the real lock.
    let e = real(&w).backup();
    assert_eq!(e.code, 0, "{e:?}"); // WHAT: a real backup. WHY: the baseline.
    let objects: Vec<_> = files_in(&w.bucket)
        .into_iter()
        .filter(|p| p.extension().map(|x| x == "age").unwrap_or(false))
        .collect();
    assert!(objects.len() >= 3); // WHAT: the encrypted objects. WHY: catalog copies and files.
    for o in &objects {
        let b = std::fs::read(o).unwrap();
        assert!(b.starts_with(b"age-encryption.org/v1\n"), "{}", o.display());
        for plain in [
            &b"SQLite format 3"[..],
            b"Real Job Name",
            b"SECRET-DRAWING-CONTENT",
        ] {
            assert!(!contains(&b, plain));
        } // WHAT: real age output, no plaintext. WHY: the claim.
        let out = w.root.join("attempt");
        for args in [
            vec![
                "-d".to_string(),
                "-i".into(),
                other.to_string_lossy().into_owned(),
            ],
            vec!["-d".into()],
        ] {
            let r = Command::new(&age)
                .args(&args)
                .arg("-o")
                .arg(&out)
                .arg(o)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(!r.status.success(), "{} opened with {args:?}", o.display());
            assert!(!out.exists() || std::fs::read(&out).unwrap().is_empty());
            let _ = std::fs::remove_file(&out);
        }
    } // WHAT: the wrong key, and no key, both fail. WHY: unreadable without the private key.
    let wrong = Drill {
        job: real(&w),
        identity: other.clone(),
        scratch: w.root.join("rw"),
        object: None,
        full: true,
        sample: 0,
        keep: false,
    }
    .run();
    assert_eq!(wrong.code, 2, "{wrong:?}"); // WHAT: the drill with the wrong key. WHY: refused.
    let right = Drill {
        job: real(&w),
        identity: key.clone(),
        scratch: w.root.join("rr"),
        object: None,
        full: true,
        sample: 0,
        keep: true,
    }
    .run();
    assert_eq!(right.code, 0, "{right:?}");
    assert!(report(&w.data).restore_tested.is_some()); // WHAT: the right key restores everything, kept to look at. WHY: the backup is real.
    owner_only(&w.root.join("rr")); // WHAT: what real age wrote is owner-only too. WHY: age never makes the file itself.
    eprintln!("NOTE: the real-age proof ran with {}", age.display()); // WHAT: say it ran. WHY: a skip and a pass must not look the same.
}

/// The office id the world's server named itself.
fn installation(w: &World) -> String {
    installation_of(&w.data.join("hyperview.sqlite")).unwrap() // WHAT: read it as the backup does. WHY: compared below
}

/// Every file under a folder with its bytes. WHY: proves a refused run changed nothing.
fn snapshot_of(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    files_in(dir).into_iter().map(|f| { let b = std::fs::read(&f).unwrap(); (f, b) }).collect() // WHAT: path to bytes. WHY: compared whole
}

#[test]
fn the_first_night_claims_the_target_for_this_office_and_says_so_only_in_ids() {
    let w = world();
    let e = job(&w).backup();
    assert_eq!(e.code, 0, "{e:?}"); // WHAT: a first night. WHY: it adopts the empty target
    let raw = std::fs::read(w.bucket.join("owner-view.json")).expect("the owner marker is at the top of the target"); // WHAT: the marker. WHY: the claim
    let o: Owner = serde_json::from_slice(&raw).unwrap();
    assert_eq!((o.format.as_str(), o.product.as_str(), o.installation.clone()), (OWNER_FORMAT, "view", installation(&w)), "it names this office"); // WHAT: whose. WHY: the one thing compared
    assert!(!contains(&raw, b"Test Fab Co") && !contains(&raw, b"Real Job Name"), "the unencrypted marker holds no names"); // WHAT: ids only. WHY: nothing readable travels unencrypted
    std::thread::sleep(std::time::Duration::from_millis(1100)); // WHAT: the next second. WHY: names are to the second
    let e2 = job(&w).backup();
    assert_eq!(e2.code, 0, "{e2:?}"); // WHAT: the next night. WHY: its own target is accepted
    assert_eq!(std::fs::read(w.bucket.join("owner-view.json")).unwrap(), raw, "the marker is never rewritten"); // WHAT: unchanged. WHY: the first office keeps it
    let log = std::fs::read_to_string(w.work.join("view/offsite.log")).unwrap();
    assert!(log.contains("target was new") && log.contains("target belongs to this office"), "the log says both"); // WHAT: the story. WHY: the morning read
}

#[test]
fn a_second_office_pointed_at_the_same_target_is_refused_and_changes_nothing() {
    let a = world();
    assert_eq!(job(&a).backup().code, 0); // WHAT: office A's backup. WHY: the target is A's
    let before = snapshot_of(&a.bucket); // WHAT: every byte of A's target. WHY: B must change none of it
    let b = world(); // WHAT: another office, its own id and key. WHY: the mistake: two offices, one NAS folder
    assert_ne!(installation(&a), installation(&b));
    let mut jb = job(&b);
    jb.target = Target::Folder(a.bucket.clone()); // WHAT: B pointed at A's folder. WHY: the case
    let e = jb.backup();
    assert_eq!(e.code, 2, "{e:?}"); // WHAT: refused before anything is marked. WHY: the guard
    assert!(e.why.contains("belongs to another office") && e.why.contains(&installation(&a)), "says whose it is: {}", e.why); // WHAT: plain words. WHY: a person can fix it
    assert_eq!(snapshot_of(&a.bucket), before, "nothing sent, copied or pruned in A's target"); // WHAT: untouched. WHY: B's keep rule would have pruned A's catalogs
    assert_eq!(report(&b.data).last_good, None, "B is not marked backed up"); // WHAT: no false green. WHY: Fleet must see B has no backup
}

#[test]
fn a_target_that_cant_be_read_or_has_a_broken_marker_is_never_adopted() {
    let w = world();
    std::fs::create_dir_all(&w.bucket).unwrap();
    std::fs::write(w.bucket.join("owner-view.json"), b"{not json").unwrap(); // WHAT: a broken marker. WHY: must not be taken for "no owner"
    let e = job(&w).backup();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(e.why.contains("owner-view.json"), "{}", e.why);
    assert!(!w.bucket.join("daily").exists(), "nothing sent"); // WHAT: refused first. WHY: the guard runs before the upload
    std::fs::write(w.bucket.join("owner-view.json"), vec![b' '; usize::try_from(OWNER_CAP + 1).unwrap()]).unwrap(); // WHAT: a giant one. WHY: refused unfetched
    let e = job(&w).backup();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(e.why.contains("over the"), "{}", e.why);
    let planted = Owner { format: OWNER_FORMAT.into(), product: "view".into(), installation: "inst_$(rm -rf)".into(), since: "x".into() };
    std::fs::write(w.bucket.join("owner-view.json"), serde_json::to_vec(&planted).unwrap()).unwrap(); // WHAT: an id the server never makes. WHY: refused, not compared
    assert_eq!(job(&w).backup().code, 2);
    let other = Owner { format: OWNER_FORMAT.into(), product: "archetype".into(), installation: installation(&w), since: "x".into() };
    std::fs::write(w.bucket.join("owner-view.json"), serde_json::to_vec(&other).unwrap()).unwrap(); // WHAT: another product's marker under View's name. WHY: refused
    assert_eq!(job(&w).backup().code, 2);
}

#[test]
fn data_with_no_office_id_is_not_backed_up() {
    let w = world();
    let store = crate::Store::open(&w.data.join("hyperview.sqlite"), &w.data.join("files")).unwrap();
    store.with(|db| { db.execute("DELETE FROM settings WHERE key = ?1", [crate::api::settings::INSTALLATION])?; Ok(()) }).unwrap(); // WHAT: a server never started. WHY: nothing to name the target's office by
    drop(store);
    let e = job(&w).backup();
    assert_eq!(e.code, 2, "{e:?}");
    assert!(e.why.contains("start the server once"), "{}", e.why); // WHAT: what to do. WHY: plain words
    assert!(!w.bucket.join("owner-view.json").exists(), "no target adopted"); // WHAT: nothing claimed. WHY: an unnamed office claims nothing
}

#[test]
fn the_drill_says_whose_backup_it_restored_and_refuses_another_offices_catalog() {
    let w = world();
    assert_eq!(job(&w).backup().code, 0);
    let cat = files_in(&w.bucket.join("daily")).into_iter().find(|p| p.to_string_lossy().ends_with(EXT)).unwrap();
    let plain = fake::open(&std::fs::read_to_string(&w.id).unwrap(), &std::fs::read(&cat).unwrap()).unwrap();
    let t = Tmp::new("man");
    untar(&mut flate2::read::GzDecoder::new(&plain[..]), &t.0.join("x"), 1 << 30, EXPECT).unwrap();
    let m: Manifest = serde_json::from_slice(&std::fs::read(t.0.join("x/manifest.json")).unwrap()).unwrap();
    assert_eq!((m.installation, m.company.as_deref()), (Some(installation(&w)), Some("Test Fab Co")), "the encrypted manifest names the office"); // WHAT: whose data. WHY: a restore can tell
    let e = drill(&w, &w.id, "s1").run();
    assert_eq!(e.code, 0, "{e:?}");
    let log = std::fs::read_to_string(w.work.join("view/offsite.log")).unwrap();
    assert!(log.contains(&format!("Test Fab Co, office {}", installation(&w))), "the summary names the office"); // WHAT: said. WHY: the person restoring knows whose it is
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let later = object_name("view", time::OffsetDateTime::now_utc());
    plant(&w, &later, |m| m["installation"] = "inst_0000000000000000000000ff".into(), 0); // WHAT: a newer catalog from another office, locked to the same key. WHY: someone copied it in
    let e = drill(&w, &w.id, "s2").run();
    assert_eq!(e.code, 2, "{e:?}"); // WHAT: refused. WHY: never restored as this office's data
    assert!(e.why.contains("was made by office inst_0000000000000000000000ff"), "{}", e.why);
    plant(&w, &later, |m| { m.as_object_mut().unwrap().remove("installation"); }, 0); // WHAT: a catalog from before catalogs named their office. WHY: still restores
    std::fs::remove_dir_all(w.root.join("s3")).ok();
    let e = drill(&w, &w.id, "s3").run();
    assert_eq!(e.code, 0, "an old catalog still drills: {e:?}");
}
