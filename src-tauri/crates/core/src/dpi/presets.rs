use rusqlite::{Connection, params};

use super::strategies::db_err;
use super::parse_strategy_args;

/// Ready-made strategies adapted from the ByeByeDPI Android app's
/// `proxytest_strategies.list` (github.com/romanvht/ByeByeDPI). The upstream
/// `{sni}` placeholder (its fake-SNI template value, default `google.com`)
/// is baked in — our ciadpi is spawned without a shell, so runtime
/// substitution would only add moving parts.
pub(super) const PRESET_STRATEGIES: [(&str, &str); 61] = [
    ("Full Barrage", "-f-200 -Qr -s3:5+sm -a1 -As -d1 -s4+sm -s8+sh -f-300 -d6+sh -a1 -At,r,s -o2 -f-30 -As -r5 -Mh -r6+sh -f-250 -s2:7+s -s3:6+sm -a1 -At,r,s -s3:5+sm -s6+s -s7:9+s -q30+sm -a1"),
    ("Split-Disorder Ladder", "-d1 -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -r1+s -S -a1 -As -d1 -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -S -a1"),
    ("Split-TLSRec Weave", "-q2 -s2 -s3+s -r3 -s4 -r4 -s5+s -r5+s -s6 -s7+s -r8 -s9+s -Qr -Mh,d,r -a1 -At,r -s2+s -r2 -d2 -s3 -r3 -r4 -s4 -d5+s -r5 -d6 -s7+s -d7 -a1"),
    ("OOB Ladder", "-o1 -d1 -a1 -At,r,s -s1 -d1 -s5+s -s10+s -s15+s -s20+s -r1+s -S -a1 -As -s1 -d1 -s5+s -s10+s -s15+s -s20+s -S -a1"),
    ("Fake SNI Cascade", "-n google.com -Qr -f-204 -s1:5+sm -a1 -As -d1 -s3+s -s5+s -q7 -a1 -As -o2 -f-43 -a1 -As -r5 -Mh -s1:5+s -s3:7+sm -a1"),
    ("Fake SNI Multi-Hit", "-n google.com -Qr -f-205 -a1 -As -s1:3+sm -a1 -As -s5:8+sm -a1 -As -d3 -q7 -o2 -f-43 -f-85 -f-165 -r5 -Mh -a1"),
    ("Slow Motion Split", "-d1+s -s50+s -a1 -As -f20 -r2+s -a1 -At -d2 -s1+s -s5+s -s10+s -s15+s -s25+s -s35+s -s50+s -s60+s -a1"),
    ("Full Fake Takeover", "-o1 -a1 -At,r,s -f-1 -a1 -At,r,s -d1:11+sm -S -a1 -At,r,s -n google.com -Qr -f1 -d1:11+sm -s1:11+sm -S -a1"),
    ("Auto Split Ladder", "-d1 -s1 -q1 -a1 -Ar -s5 -o1+s -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -a1"),
    ("Fake End Split", "-f1+nme -t6 -a1 -As -n google.com -Qr -s1:6+sm -a1 -As -s5:12+sm -a1 -As -d3 -q7 -r6 -Mh -a1"),
    ("Classic Ladder", "-d1 -s1+s -d3+s -s6+s -d9+s -s12+s -d15+s -s20+s -d25+s -s30+s -d35+s -a1"),
    ("Tight Ladder", "-d1 -s1+s -d1+s -s3+s -d6+s -s12+s -d14+s -s20+s -s24+s -s30+s -a1"),
    ("OOB Fake Bounce", "-o1 -a1 -At,r,s -f-1 -a1 -Ar,s -o1 -a1 -At -r1+s -f-1 -t6 -a1"),
    ("Wide Ladder", "-d1 -s1+s -s3+s -s6+s -s9+s -s12+s -s15+s -s20+s -s30+s -a1"),
    ("Early Bird", "-d1 -d3+s -s6+s -d6+s -s7+s -d8+s -s10+s -a1 -t12 -At,s -r3"),
    ("Fake TLS Hopscotch", "-f1 -t5 -n google.com -q3+h -Qr -f2 -q1 -r1+s -t15 -q1 -o2 -a1"),
    ("Host Shift", "-n google.com -d2:5:2+h -f-3 -r2+sm -o2 -o50+s -r2+s -f-4 -a1"),
    ("Fake First", "-f-1 -Qr -s1+sm -d3+s -s5+sm -o2 -a1 -As -r1+s -d8+s -a1"),
    ("Record Shuffle", "-r-1+s -o20+sm -s3:7+sm -d5:3+sm -f300+s -Qr -f-1 -a1"),
    ("Offset OOB", "-o2 -O4 -s1 -q1 -a1 -Ar -s5 -o1+s -f1+s -r20+s -a1"),
    ("Tail Record", "-o1 -r-5+se -a1 -At,r,s -d1 -n google.com -Qr -f-1 -a1"),
    ("Fake Classic", "--fake -1 --ttl 8 --split 1+s --disorder 3+s -a1"),
    ("Fake SNI Slices", "-n google.com -Qr -f6+nr -d2 -d11 -f9+hm -o3 -t7 -a1"),
    ("Double Record", "-r5+s -s25+s -a1 -At,r,s -s50 -r5+s -s50+s -a1"),
    ("Short Ladder", "-d1 -d3+s -s6+s -d9+s -s20+s -d25+s -s30+s -a1"),
    ("Midstream OOB", "-d9+s -q20+s -s25+s -t5 -a1 -At,r,s -r1+h -a1"),
    ("Tail Split OOB", "-q1+s -s29+s -s30+s -s14+s -o5+s -f-1 -S -a1"),
    ("Minor TLS Mix", "-d1 -s1+s -r1+s -e1 -m1 -o1+s -f-1 -t2 -a1"),
    ("Auto OOB", "-d1 -o1 -a1 -Ar -o1 -a1 -At -f-1 -r1+s -a1"),
    ("Front Load", "-d1 -s4 -d8 -s1+s -d5+s -s10+s -d20+s -a1"),
    ("Fake SNI Record", "-f-1 -n google.com -Qr -s2+s -r3 -o20 -t4 -a1"),
    ("SNI Middle Fake", "-n google.com -Qr -d5+sm -f3+sm -o2 -t4 -a1"),
    ("Auto Queue", "-o1 -a1 -Ar -q1 -a1 -At -f-1 -r1+s -a1"),
    ("Queue First", "-q1 -a1 -Ar -o1 -a1 -At -f-1 -r1+s -a1"),
    ("SNI Nudge", "-s4+sn -r9+s -Qr -n google.com -S -a1"),
    ("Compact Mix", "-o1 -d1 -r1+s -S -s1+s -d3+s -a1"),
    ("OOB Tail", "-q1+s -s29+s -o5+s -f-1 -S -a1"),
    ("TLS Minor Fake", "-n google.com -Qr -m2 -f-1 -d7 -a1"),
    ("Simple Fake", "-d1 -s1+s -r1+s -f-1 -t8 -a1"),
    ("OOB End Fake", "-o1 -a1 -An -f1+nme -t6 -a1"),
    ("Fake Record", "-n google.com -Qr -f-1 -r1+s -a1"),
    ("Triple Disorder", "-n google.com -Qr -d1:3 -f-1 -a1"),
    ("Minimal Auto", "-s1 -d3+s -a1 -At -r1+s -a1"),
    ("TTL Split", "-f-1 -t8 -n google.com -s1+s -a1"),
    ("Fake Duo", "-n google.com -Qr -d1 -f-1 -a1"),
    ("End Fake", "-f64+se -n google.com -t5 -a1"),
    ("Auto Disorder", "-o1 -a1 -At,r,s -d1 -a1"),
    ("Quick Mix", "-d1+s -o2 -s5 -r5 -a1"),
    ("Record Split", "-r8 -o2 -s7 -q4+s -a1"),
    ("Fake Tail", "-o1 -f-1 -r-5+se -a1"),
    ("Host OOB", "-d6+s -q4+hm -o2 -a1"),
    ("TLS Minor Split", "-s5+s -s35+s -m4 -a1"),
    ("Fake Middle", "-f-1+sm -t7 -m2 -a1"),
    ("End Record", "-o1 -r-5+se -a1"),
    ("Twin Split", "-o1+s -d3+s -a1"),
    ("Double Split", "-o1 -s4 -s6 -a1"),
    ("Late Record", "-q1 -r25+s -a1"),
    ("Basic Split", "-d1 -s3+s -a1"),
    ("OOB Disorder", "-o3 -d7 -a1"),
    ("Reverse Duo", "-d7 -s2 -a1"),
    (DESPAIR_NAME, DESPAIR_ARGS),
];

/// The brute-force "Despair" preset: a giant `-Ku -l:<payload>` fake plus a
/// full split/disorder ladder — the closing entry of `PRESET_STRATEGIES`,
/// shipped like every other preset.
pub(super) const DESPAIR_NAME: &str = "Despair";
const DESPAIR_ARGS: &str = "-Ku -l:\\xC2\\x00\\x00\\x00\\x01\\x14\\x2E\\xE3\\xE3\\x5F\\x6B\\xBB\\x23\\xA8\\xE6\\x5D\\xA9\\x78\\x21\\xCF\\xC2\\x72\\x4C\\x8F\\xC4\\x5E\\x14\\x00\\x00\\x00\\x00\\xC5\\x00\\x00\\x00\\x00\\x4C\\x00\\xA7\\x00\\x00\\x00\\x00\\x00\\x00\\x44\\x00\\x00\\x80\\x00\\x00\\x00\\x0D\\xFC\\xFA\\x1D\\xCD\\x73\\xBA\\x2A\\x90\\x93\\xB3\\xEE\\xF7\\x43\\xC5\\x85\\xDA\\xFF\\x45\\x3C\\x00\\x00\\x00\\x00\\x00\\x00\\x7C\\x00\\x9B\\x00\\xF6\\x00\\x00\\xDD\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x59\\xA8\\xE4\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x7B\\x00\\x0F\\x00\\x00\\x00\\x48\\x4E\\x00\\x00\\x00\\x06\\xF3\\x00\\x00\\x00\\x00\\xD9\\x5A\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00 -a3 -An -o1 -d1 -r1+s -t10 -b4000 -s1+s -s3+s -s6+s -s9+s -s12+s -s15+s -s20+s -s30+s -As -q1+s -s29+s -o5+s -f3 -S -As -d1+s -s3+s -d5+s -s7+s -r2+s -Mh,d -An";

/// Inserts the bundled preset catalog once — only when the user has no
/// strategies of their own (an existing collection is never touched).
/// Called from app setup, not from `db::open`, so unit tests with throwaway
/// databases start from a clean slate.
pub fn seed_default_strategies(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM dpi_strategies", [], |row| row.get(0))
        .map_err(db_err)?;
    if count > 0 {
        return Ok(());
    }
    for (name, args) in PRESET_STRATEGIES {
        // validated like user input — a preset the current rules reject must
        // fail loudly at startup instead of poisoning the spawn path later
        let tokens = parse_strategy_args(args)?;
        conn.execute(
            "INSERT INTO dpi_strategies (name, args) VALUES (?1, ?2)",
            params![name, tokens.join(" ")],
        )
        .map_err(db_err)?;
    }
    Ok(())
}
