use rusqlite::{Connection, params};

use super::queries::db_err;
use super::validation::{validate_rule_value, RULE_TYPE_DOMAIN_SUFFIX};

/// The default catalog: (category name, [bare hosts]). Hosts are stored as
/// `domain_suffix` rules — one rule covers the host and every subdomain.
pub(super) const DEFAULT_SITES: &[(&str, &[&str])] = &[
    (
        "General",
        &["rutracker.org", "nyaa.si", "rutor.org", "nnmclub.to", "speedtest.net", "ookla.com"],
    ),
    (
        "Cloudflare",
        &["cloudflare.net", "cloudflare.com", "cloudflarecn.net", "cloudflare-ech.com"],
    ),
    (
        "Discord",
        &[
            "dis.gd",
            "discord.co",
            "discord.gg",
            "discord.app",
            "discord.com",
            "discord.dev",
            "discord.new",
            "discord.gift",
            "discord.gifts",
            "discord.media",
            "discord.store",
            "discord.design",
            "discordapp.com",
            "discordcdn.com",
            "discordsez.com",
            "discordsays.com",
            "discordmerch.com",
            "discordpartygames.com",
            "discordactivities.com",
            "stable.dl2.discordapp.net",
            "discord-attachments-uploads-prd.storage.googleapis.com",
        ],
    ),
    (
        "Google Video",
        &[
            "rr1---sn-4axm-n8vs.googlevideo.com",
            "rr1---sn-gvnuxaxjvh-o8ge.googlevideo.com",
            "rr1---sn-ug5onuxaxjvh-p3ul.googlevideo.com",
            "rr1---sn-ug5onuxaxjvh-n8v6.googlevideo.com",
            "rr4---sn-q4flrnsl.googlevideo.com",
            "rr10---sn-gvnuxaxjvh-304z.googlevideo.com",
            "rr14---sn-n8v7kn7r.googlevideo.com",
            "rr16---sn-axq7sn76.googlevideo.com",
            "rr1---sn-8ph2xajvh-5xge.googlevideo.com",
            "rr1---sn-gvnuxaxjvh-5gie.googlevideo.com",
            "rr12---sn-gvnuxaxjvh-bvwz.googlevideo.com",
            "rr5---sn-n8v7knez.googlevideo.com",
            "rr1---sn-u5uuxaxjvhg0-ocje.googlevideo.com",
            "rr2---sn-q4fl6ndl.googlevideo.com",
            "rr5---sn-gvnuxaxjvh-n8vk.googlevideo.com",
            "rr4---sn-jvhnu5g-c35d.googlevideo.com",
            "rr1---sn-q4fl6n6y.googlevideo.com",
            "rr2---sn-hgn7ynek.googlevideo.com",
            "rr1---sn-xguxaxjvh-gufl.googlevideo.com",
            "googlevideo.com",
        ],
    ),
    (
        "Social",
        &[
            "snapchat.com",
            "snap.com",
            "linkedin.com",
            "facebook.com",
            "fb.com",
            "fb.me",
            "fbcdn.net",
            "messenger.com",
            "meta.com",
            "instagram.com",
            "static.cdninstagram.com",
            "proton.me",
            "medium.com",
            "x.com",
            "twitter.com",
            "twimg.com",
            "soundcloud.com",
        ],
    ),
    (
        "Telegram",
        &[
            "telegram.org",
            "core.telegram.org",
            "web.telegram.org",
            "webk.telegram.org",
            "my.telegram.org",
            "translations.telegram.org",
            "instantview.telegram.org",
            "blog.telegram.org",
            "comments.telegram.org",
            "verify.telegram.org",
            "login.telegram.org",
            "auth.telegram.org",
            "api.telegram.org",
            "promo.telegram.org",
            "desktop.telegram.org",
            "macos.telegram.org",
            "ios.telegram.org",
            "android.telegram.org",
            "reactions.telegram.org",
            "claims.telegram.org",
            "x.telegram.org",
            "help.telegram.org",
            "docs.telegram.org",
            "schema.telegram.org",
            "dev.telegram.org",
            "contest.telegram.org",
            "premium.telegram.org",
            "settings.telegram.org",
            "qr.telegram.org",
            "stickers.telegram.org",
            "emoji.telegram.org",
            "themes.telegram.org",
            "donate.telegram.org",
            "fragment.telegram.org",
            "ton.telegram.org",
            "wallet.telegram.org",
            "pay.telegram.org",
            "telegram.me",
            "t.me",
            "telegram.dog",
            "telegra.ph",
            "telesco.pe",
            "web.telegram.me",
            "zws1.web.telegram.org",
            "zws2.web.telegram.org",
            "zws1.web.telegram.me",
            "zws2.web.telegram.me",
            "venus.web.telegram.org",
            "pluto.web.telegram.org",
            "aurora.web.telegram.org",
            "vesta.web.telegram.org",
            "voice.telegram.org",
            "cdn.telegram.org",
        ],
    ),
    (
        "Türkiye",
        &[
            "roblox.com",
            "wattpad.com",
            "pastebin.com",
            "4shared.com",
            "wikileaks.org",
            "bitly.com",
            "cutt.ly",
            "t2m.io",
        ],
    ),
    (
        "YouTube",
        &[
            "youtu.be",
            "youtube.com",
            "i.ytimg.com",
            "i9.ytimg.com",
            "yt3.ggpht.com",
            "yt4.ggpht.com",
            "googleapis.com",
            "jnn-pa.googleapis.com",
            "googleusercontent.com",
            "signaler-pa.youtube.com",
            "youtubei.googleapis.com",
            "manifest.googlevideo.com",
            "yt3.googleusercontent.com",
            "ytimg.com",
            "ggpht.com",
        ],
    ),
];

/// Inserts the default catalog once — only into a completely empty table
/// (any user edits, even a single added category, block re-seeding). Called
/// from app setup, not from `db::open`, so unit tests start clean.
pub fn seed_default_sites(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM test_site_categories", [], |row| row.get(0))
        .map_err(db_err)?;
    if count > 0 {
        return Ok(());
    }
    for (position, (name, hosts)) in DEFAULT_SITES.iter().enumerate() {
        conn.execute(
            "INSERT INTO test_site_categories (name, position) VALUES (?1, ?2)",
            params![name, position as i64],
        )
        .map_err(db_err)?;
        let category_id = conn.last_insert_rowid();
        for host in *hosts {
            let value = validate_rule_value(RULE_TYPE_DOMAIN_SUFFIX, host)?;
            conn.execute(
                "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, ?2, ?3)",
                params![category_id, RULE_TYPE_DOMAIN_SUFFIX, value],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}
