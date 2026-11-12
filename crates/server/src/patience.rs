//! How many wrong guesses a server will sit through.
//!
//! There was no limit at all. Not on signing in, not on the join code, not
//! anywhere. On a shop network that is defensible: to guess at anything you
//! have to already be in the building. The moment a server is reachable from
//! a jobsite it is not, and the join code is why — one server-wide secret
//! that names nobody, never expires, is not used up, and gets whoever types
//! it an account.
//!
//! So this counts wrong answers and makes them cost time. Nothing here locks
//! anybody out: the wait grows and then stops growing, and one right answer
//! clears it. An estimator who has forgotten which password they used waits
//! a few seconds. Somebody working through a dictionary from a hotel wi-fi
//! gets a quarter of an hour per guess, which is the end of that.
//!
//! **In memory, on purpose.** A failed guess is not worth a disk write, and
//! writing one would hand an attacker a way to fill the disk. Restarting the
//! server clears the counts, and that is fine: restarting a server is not
//! something anybody outside the building can make happen.
//!
//! **Bounded, on purpose.** Somebody with a thousand addresses must not be
//! able to grow this without limit, so it holds a fixed number of entries and
//! forgets the least recently touched. That does blunt per-address counting
//! for an attacker who rotates addresses — which is exactly why the count is
//! kept per account as well. Rotating addresses does not change whose
//! password you are guessing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Entries kept before the least recently touched is forgotten. Roughly a
/// megabyte of short strings, and far more than any real shop has people or
/// addresses.
const MOST_KEPT: usize = 4096;

/// What a kind of guess costs.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    /// Wrong answers allowed before any wait at all. Somebody typing their
    /// own password badly should never meet this.
    pub free: u32,
    /// The wait after the first one past `free`. It doubles from there.
    pub first: Duration,
    /// The longest it ever gets. Past this the answer is simply no, at a
    /// rate nobody can work with.
    pub longest: Duration,
}

/// Signing in. Five tries free, because people mistype passwords, then a
/// wait that doubles to a quarter of an hour.
pub const SIGNING_IN: Rule = Rule {
    free: 5,
    first: Duration::from_secs(2),
    longest: Duration::from_secs(15 * 60),
};

/// Typing a join code. Harsher, and deliberately: a join code has no
/// username in front of it, so a guess is a guess at the whole secret.
/// Nobody legitimately types one twenty times.
pub const JOINING: Rule = Rule {
    free: 3,
    first: Duration::from_secs(5),
    longest: Duration::from_secs(60 * 60),
};

/// Giving the maintenance key. Excalibur's Fleet always has it right, so a
/// wrong one is somebody guessing (audit, 2026-11-11).
pub const MAINTAINING: Rule = Rule {
    free: 5,
    first: Duration::from_secs(5),
    longest: Duration::from_secs(60 * 60),
};

#[derive(Clone, Copy)]
struct Wrong {
    times: u32,
    last: Instant,
    kept: bool, // WHAT: whether this is a real account's count. WHY: those are never forgotten to make room, so no flood of other names can wipe the wait on an account being guessed at.
}

/// The counts, and the lock around them.
#[derive(Default)]
pub struct Patience {
    wrong: Mutex<HashMap<String, Wrong>>,
}

impl std::fmt::Debug for Patience {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self.wrong.lock().map(|w| w.len()).unwrap_or(0);
        write!(f, "Patience({held} watched)")
    }
}

impl Patience {
    /// How much longer this one has to wait, if it does.
    ///
    /// Ask before doing the work, so a refusal costs the server nothing. An
    /// empty `who` is not counted at all: that is the case where nothing is
    /// known about where a request came from, and counting every one of those
    /// together would let one bad actor lock out a whole shop behind the same
    /// proxy.
    pub fn wait_for(&self, rule: Rule, who: &str) -> Option<Duration> {
        if who.is_empty() {
            return None;
        }
        let held = self.wrong.lock().ok()?;
        owed_by(rule, held.get(who)?) // WHAT: what this one owes, if anything. WHY: the same sum `begin` does.
    }

    /// The first of these that has to wait, in the order given.
    pub fn wait_for_any(&self, rule: Rule, who: &[&str]) -> Option<Duration> {
        who.iter().filter_map(|one| self.wait_for(rule, one)).max()
    }

    /// Asks and counts in one step: the longest wait any of `who` owes, or,
    /// when none owes one, this try counted against each of them as wrong
    /// before it is checked. A right answer then clears it with [`right`].
    ///
    /// Asking and counting apart let tries sent together all be asked before
    /// any was counted, so a burst of fifty got fifty guesses where five were
    /// allowed. Done under one lock, each try sees every try before it.
    ///
    /// [`right`]: Patience::right
    ///
    /// # Errors
    ///
    /// The wait still owed, when any of `who` owes one; the try is then not counted.
    pub fn begin(&self, rule: Rule, who: &[&str]) -> Result<(), Duration> {
        self.begin_keeping(rule, who, &[]) // WHAT: nothing in `who` is a real account. WHY: every count may be forgotten to make room.
    }

    /// [`begin`], with the names in `keep` (real accounts) counted so that
    /// they are never forgotten to make room. A real account's count is
    /// bounded by the number of people on the server, so keeping it costs
    /// nothing a stranger can grow; letting it be pushed out handed whoever
    /// filled the ledger five fresh guesses at it every time.
    ///
    /// [`begin`]: Patience::begin
    ///
    /// # Errors
    ///
    /// The wait still owed, as [`begin`].
    pub fn begin_keeping(&self, rule: Rule, who: &[&str], keep: &[&str]) -> Result<(), Duration> {
        let Ok(mut held) = self.wrong.lock() else {
            return Ok(()); // WHAT: a poisoned lock lets the try through uncounted. WHY: the same as `wait_for` did; a panic elsewhere must not lock the whole office out.
        };
        let owed = who
            .iter()
            .filter(|one| !one.is_empty())
            .filter_map(|one| owed_by(rule, held.get(*one)?))
            .max(); // WHAT: the longest wait any of them owes. WHY: the worst of the account and the address decides.
        if let Some(owed) = owed {
            return Err(owed); // WHAT: refused, and not counted again. WHY: a refused try checks nothing, so it is not a guess.
        }
        for one in who.iter().filter(|one| !one.is_empty()) {
            count_wrong(&mut held, one, keep.contains(one)); // WHAT: counted as wrong before it is checked, kept for good when it is a real account. WHY: the next try, however soon, sees it.
        }
        Ok(())
    }

    /// That was wrong. Count it.
    pub fn wrong(&self, who: &str) {
        if who.is_empty() {
            return;
        }
        let Ok(mut held) = self.wrong.lock() else { return };
        count_wrong(&mut held, who, false); // WHAT: one more wrong. WHY: the same counting `begin` does.
    }

    /// That was right. Forget everything about them.
    pub fn right(&self, who: &str) {
        if who.is_empty() {
            return;
        }
        if let Ok(mut held) = self.wrong.lock() {
            held.remove(who);
        }
    }

    /// How many are being watched. For the tests and for a look at the box.
    pub fn watching(&self) -> usize {
        self.wrong.lock().map(|w| w.len()).unwrap_or(0)
    }
}

/// What one entry owes under `rule`, if anything.
fn owed_by(rule: Rule, seen: &Wrong) -> Option<Duration> {
    // `free` wrong answers cost nothing. The attempt after that waits.
    // So the test is on what has already been got wrong, not on what is
    // being tried now: at `free` wrongs the allowance is spent.
    let spent = seen.times.checked_sub(rule.free)?;
    let owed = penalty(rule, spent);
    let waited = seen.last.elapsed();
    (waited < owed).then(|| owed - waited)
}

/// Counts one wrong answer against `who`, making room first when full.
fn count_wrong(held: &mut HashMap<String, Wrong>, who: &str, kept: bool) {
    if held.len() >= MOST_KEPT && !held.contains_key(who) {
        forget_the_least_guessed_at(held); // WHAT: room made. WHY: the map never grows past its size.
    }
    let seen = held.entry(who.to_string()).or_insert(Wrong { times: 0, last: Instant::now(), kept }); // WHAT: this one's count, new or kept. WHY: added to.
    seen.times = seen.times.saturating_add(1); // WHAT: one more. WHY: counted.
    seen.last = Instant::now(); // WHAT: when. WHY: the wait runs from the last wrong one.
    seen.kept |= kept; // WHAT: once a real account, always kept. WHY: a later count of the same name must not make it forgettable again.
}

/// The wait owed once the free ones are spent: the first wait, then double
/// it for each further wrong answer, then flat forever.
fn penalty(rule: Rule, spent: u32) -> Duration {
    let doublings = spent.min(20);
    rule.first
        .saturating_mul(1u32 << doublings)
        .min(rule.longest)
}

/// Drops a tenth of the entries, those with the fewest wrong answers first
/// and the stalest of those, so this is not done on every call once the map
/// is full.
///
/// Fewest first, not oldest first: an account somebody is guessing at has
/// many wrong answers against it, and four thousand made-up names tried once
/// each must not be able to push it out and give the guesser five free tries
/// again. Oldest first did exactly that.
fn forget_the_least_guessed_at(held: &mut HashMap<String, Wrong>) {
    let only_kept = held.values().all(|v| v.kept); // WHAT: whether every entry is a real account. WHY: then one of them has to go, or the map would grow past its size.
    let mut least: Vec<(u32, Instant, String)> = held
        .iter()
        .filter(|(_, v)| only_kept || !v.kept)
        .map(|(k, v)| (v.times, v.last, k.clone()))
        .collect(); // WHAT: each forgettable entry's count, age and name; a real account's count is never among them. WHY: made-up names and addresses with more wrong tries than the account could otherwise push it out.
    least.sort_by_key(|(times, when, _)| (*times, *when)); // WHAT: fewest wrong first, then oldest. WHY: as said above.
    for (_, _, key) in least.into_iter().take(MOST_KEPT / 10) {
        held.remove(&key); // WHAT: forgotten. WHY: room for new ones.
    }
}

/// The name an address is counted under. An IPv4 address is itself. An
/// IPv6 address is its /64: one home, office or cloud machine is handed a
/// whole /64 and can take a new address from it for every request, so
/// counting the address alone let it make a new one per guess.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn counted_as(address: &str) -> String {
    match address.parse::<std::net::IpAddr>().map(|ip| ip.to_canonical()) {
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments(); // WHAT: its eight groups. WHY: the first four are the network.
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3]) // WHAT: the network it is on. WHY: what one caller can't step out of.
        }
        Ok(v4) => v4.to_string(), // WHAT: an IPv4 address, or one written as IPv6, as itself. WHY: one machine.
        Err(_) => address.to_string(), // WHAT: not an address (empty, in a test). WHY: counted as it is, or not at all when empty.
    }
}

/// What to tell somebody who has to wait. Never says how many tries are left
/// or what the limit is; that is a hint about the thing being guarded.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn refusal(owed: Duration) -> String {
    let seconds = owed.as_secs().max(1);
    let how_long = if seconds < 90 {
        format!("{seconds} seconds")
    } else {
        format!("{} minutes", seconds.div_ceil(60))
    };
    format!(
        "Too many wrong tries. Wait {how_long} and try again. \
         If you have forgotten it, an administrator can set it for you."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule with no waiting in it, so the tests do not sleep.
    const QUICK: Rule = Rule {
        free: 2,
        first: Duration::from_secs(10),
        longest: Duration::from_secs(40),
    };

    #[test]
    fn the_free_ones_really_are_free_and_the_next_one_is_not() {
        let p = Patience::default();
        for attempt in 1..=QUICK.free {
            assert!(
                p.wait_for(QUICK, "somebody").is_none(),
                "attempt {attempt} of {} is free", QUICK.free
            );
            p.wrong("somebody");
        }
        assert!(
            p.wait_for(QUICK, "somebody").is_some(),
            "the allowance is spent, so the next one waits"
        );
    }

    #[test]
    fn the_wait_doubles_and_then_stops() {
        assert_eq!(penalty(QUICK, 0), Duration::from_secs(10), "the first wait");
        assert_eq!(penalty(QUICK, 1), Duration::from_secs(20));
        assert_eq!(penalty(QUICK, 2), Duration::from_secs(40));
        assert_eq!(penalty(QUICK, 3), Duration::from_secs(40), "capped");
        assert_eq!(penalty(QUICK, 99), Duration::from_secs(40), "still capped");
        // And the real rules cannot overflow their way to nothing.
        assert_eq!(penalty(SIGNING_IN, 1000), SIGNING_IN.longest);
        assert_eq!(penalty(JOINING, 1000), JOINING.longest);
    }

    #[test]
    fn getting_it_right_clears_it() {
        let p = Patience::default();
        for _ in 0..10 {
            p.wrong("somebody");
        }
        assert!(p.wait_for(QUICK, "somebody").is_some());
        p.right("somebody");
        assert!(p.wait_for(QUICK, "somebody").is_none(), "one right answer is the end of it");
        assert_eq!(p.watching(), 0);
    }

    #[test]
    fn one_person_guessing_does_not_hold_up_anybody_else() {
        let p = Patience::default();
        for _ in 0..20 {
            p.wrong("the-guesser");
        }
        assert!(p.wait_for(QUICK, "the-guesser").is_some());
        assert!(p.wait_for(QUICK, "somebody-else").is_none());
    }

    /// The case that matters when a server is on the internet: nothing is
    /// known about where the request came from, so nothing is counted. The
    /// alternative -- counting them all together -- would let one bad actor
    /// lock out every seat behind the same proxy.
    #[test]
    fn a_caller_we_know_nothing_about_is_not_counted_with_the_others() {
        let p = Patience::default();
        for _ in 0..50 {
            p.wrong("");
        }
        assert!(p.wait_for(QUICK, "").is_none());
        assert_eq!(p.watching(), 0, "nothing was kept");
    }

    #[test]
    fn it_cannot_be_grown_without_limit() {
        let p = Patience::default();
        for n in 0..(MOST_KEPT + 500) {
            p.wrong(&format!("address-{n}"));
        }
        assert!(p.watching() <= MOST_KEPT, "held {} entries", p.watching());
    }

    #[test]
    fn the_worst_of_the_two_is_the_one_that_counts() {
        let p = Patience::default();
        for _ in 0..3 {
            p.wrong("their-account");
        }
        let by_account = p.wait_for(QUICK, "their-account").unwrap();
        let either = p.wait_for_any(QUICK, &["their-address", "their-account"]).unwrap();
        // Not equality: what is owed counts down between the two calls, and
        // the property worth holding is which name decided the answer, not
        // that a shrinking number stopped shrinking.
        let apart = by_account.abs_diff(either);
        assert!(apart < Duration::from_secs(1), "{by_account:?} vs {either:?}");
        // Three wrong, two of them free, so one past: the first wait, ten
        // seconds. The address has nothing against it and contributed
        // nothing, which is the point.
        assert!(either > Duration::from_secs(19), "the account decided it: {either:?}");
        assert!(p.wait_for(QUICK, "their-address").is_none());
        assert!(p.wait_for_any(QUICK, &["nobody", "no-one"]).is_none());
    }

    #[test]
    fn tries_sent_together_are_each_counted_before_the_next_is_let_in() {
        let p = std::sync::Arc::new(Patience::default()); // WHAT: one ledger. WHY: shared by many threads at once, as the server's is.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(32)); // WHAT: a starting gate. WHY: every try goes at the same instant.
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let (p, barrier) = (p.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait(); // WHAT: all at once. WHY: the burst a script sends.
                    p.begin(QUICK, &["their-account", "their-address"]).is_ok() // WHAT: whether this try was let in. WHY: counted below.
                })
            })
            .collect();
        let let_in = threads.into_iter().filter_map(|t| t.join().ok()).filter(|got_in| *got_in).count(); // WHAT: how many got to be checked. WHY: the free ones and no more.
        assert_eq!(let_in, QUICK.free as usize, "a burst got {let_in} tries where {} are free", QUICK.free);
    }

    #[test]
    fn a_flood_of_made_up_names_does_not_push_out_the_account_being_guessed_at() {
        let p = Patience::default(); // WHAT: a ledger. WHY: filled below.
        for _ in 0..(QUICK.free + 1) {
            let _ = p.begin(QUICK, &["the-victim"]); // WHAT: wrong tries at one account. WHY: until it owes a wait.
        }
        assert!(p.wait_for(QUICK, "the-victim").is_some(), "the account owes a wait");
        for n in 0..(MOST_KEPT * 2) {
            p.wrong(&format!("made-up-{n}@example.com")); // WHAT: a name nobody has, tried once. WHY: what a guesser sends to fill the ledger.
        }
        assert!(p.wait_for(QUICK, "the-victim").is_some(), "the account still owes its wait after the flood");
        assert!(p.watching() <= MOST_KEPT, "and the ledger stayed its size: {}", p.watching());
    }

    #[test]
    fn an_account_being_guessed_at_outlives_a_flood_of_names_guessed_at_more_often() {
        let p = Patience::default(); // WHAT: a ledger. WHY: filled below.
        for _ in 0..(QUICK.free + 1) {
            let _ = p.begin_keeping(QUICK, &["the-victim@example.com"], &["the-victim@example.com"]); // WHAT: wrong tries at a real account. WHY: until it owes a wait.
        }
        assert!(p.wait_for(QUICK, "the-victim@example.com").is_some(), "the account owes a wait");
        for n in 0..(MOST_KEPT * 2) {
            for _ in 0..(QUICK.free + 5) {
                p.wrong(&format!("made-up-{n}@example.com")); // WHAT: a made-up name with more wrong tries than the account. WHY: fewest-first alone forgot the account before these.
            }
        }
        assert!(p.wait_for(QUICK, "the-victim@example.com").is_some(), "a real account's wait survives names guessed at more often than it");
        assert!(p.watching() <= MOST_KEPT, "and the ledger stayed its size: {}", p.watching());
    }

    #[test]
    fn a_ledger_of_nothing_but_real_accounts_still_keeps_its_size() {
        let p = Patience::default(); // WHAT: a ledger. WHY: filled with kept names only.
        for n in 0..(MOST_KEPT + 50) {
            let name = format!("person-{n}@example.com"); // WHAT: a real account. WHY: kept.
            let _ = p.begin_keeping(QUICK, &[name.as_str()], &[name.as_str()]); // WHAT: one wrong try. WHY: kept for good.
        }
        assert!(p.watching() <= MOST_KEPT, "held {}", p.watching());
    }

    #[test]
    fn an_ipv6_caller_is_counted_by_its_64_and_cannot_step_out_of_it() {
        assert_eq!(counted_as("2001:db8:1:2:aaaa::1"), counted_as("2001:db8:1:2:ffff:ffff:ffff:fffe"), "two addresses in one /64 are one caller");
        assert_ne!(counted_as("2001:db8:1:2::1"), counted_as("2001:db8:1:3::1"), "the next /64 is somebody else");
        assert_eq!(counted_as("2001:db8:1:2::1"), "2001:db8:1:2::/64");
        assert_eq!(counted_as("203.0.113.9"), "203.0.113.9", "IPv4 is itself");
        assert_eq!(counted_as("::ffff:203.0.113.9"), "203.0.113.9", "IPv4 written as IPv6 is the IPv4 address");
        assert_eq!(counted_as(""), "", "nothing known stays nothing");
        let p = Patience::default(); // WHAT: a ledger. WHY: a caller rotating through its /64.
        let mut let_in = 0; // WHAT: how many guesses got through. WHY: the free ones and no more.
        for n in 0..50u32 {
            let address = counted_as(&format!("2001:db8:1:2::{n:x}")); // WHAT: a fresh address per guess. WHY: what an IPv6 caller can do for free.
            if p.begin(QUICK, &[address.as_str()]).is_ok() {
                let_in += 1; // WHAT: one more let in. WHY: counted.
            }
        }
        assert_eq!(let_in, QUICK.free as usize, "a new address per guess got {let_in} guesses");
    }

    #[test]
    fn the_refusal_says_how_long_and_nothing_else() {
        let said = refusal(Duration::from_secs(45));
        assert!(said.contains("45 seconds"), "{said}");
        assert!(said.contains("administrator"), "{said}");
        assert!(!said.contains("tries left"), "never a hint about what is left");
        assert!(refusal(Duration::from_secs(600)).contains("10 minutes"));
        // Never zero, whatever it is handed.
        assert!(refusal(Duration::from_millis(1)).contains("1 second"));
    }
}
