//! Pure helpers for starting the on-device daemon: how to quote paths for the
//! Android shell, how to wrap a command in `su` for the different `su`
//! implementations found on phones and TV boxes, and how to read their output.
//!
//! Nothing here touches Windows or the network, so it is unit-tested on Linux.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where the daemon is pushed on the device.
pub const SERVER_PATH: &str = "/data/local/tmp/androidfs_server";
/// TCP port the daemon listens on (device side, `127.0.0.1`).
pub const DAEMON_PORT: u16 = 12345;

/// How the daemon should access the device storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessMode {
    /// Start as the ADB `shell` user; if that cannot read the chosen folder
    /// and the device is rooted, restart the daemon as root automatically.
    Auto,
    /// Never use `su` (works on every device, limited to what `shell` may read).
    Shell,
    /// Always start the daemon through `su` (full access on rooted devices).
    Root,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::Auto => "auto",
            AccessMode::Shell => "shell",
            AccessMode::Root => "root",
        }
    }

    pub fn parse(value: &str) -> AccessMode {
        match value.trim().to_ascii_lowercase().as_str() {
            "shell" | "noroot" | "no-root" => AccessMode::Shell,
            "root" | "su" => AccessMode::Root,
            _ => AccessMode::Auto,
        }
    }
}

/// The different ways `su` accepts a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuStyle {
    /// Magisk, SuperSU, KernelSU, most modern ROMs: `su -c 'cmd'`.
    DashC,
    /// Old ChainsDD / some TV boxes: `su 0 sh -c 'cmd'`.
    Uid0,
    /// AOSP userdebug builds: `su root sh -c 'cmd'`.
    RootName,
}

pub const SU_STYLES: [SuStyle; 3] = [SuStyle::DashC, SuStyle::Uid0, SuStyle::RootName];

impl SuStyle {
    pub fn label(self) -> &'static str {
        match self {
            SuStyle::DashC => "su -c",
            SuStyle::Uid0 => "su 0",
            SuStyle::RootName => "su root",
        }
    }
}

/// How a command is executed on the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Plain `adb shell`. `uid_zero` is true when adbd itself already runs as root.
    Shell { uid_zero: bool },
    /// Through `su`.
    Root(SuStyle),
}

impl Access {
    pub fn label(self) -> String {
        match self {
            Access::Shell { uid_zero: false } => "shell (no root)".to_string(),
            Access::Shell { uid_zero: true } => "root (adb root)".to_string(),
            Access::Root(style) => format!("root ({})", style.label()),
        }
    }

    pub fn is_root(self) -> bool {
        !matches!(self, Access::Shell { uid_zero: false })
    }
}

/// Paths that may be handed to the Android shell. Spaces are allowed (the path
/// is single-quoted), quotes, `$`, backticks, `;` and friends are not.
pub fn is_safe_android_path(path: &str) -> bool {
    let path = path.trim();
    path.starts_with('/')
        && path.len() < 240
        && path.chars().all(|ch| {
            ch.is_alphanumeric()
                || matches!(ch, '/' | '_' | '.' | '-' | ' ' | '+' | '@' | '(' | ')' | ',' | '=' | '~')
        })
        && !path.split('/').any(|part| part == "..")
}

/// Special drive-root value: the first SD card / USB drive found under `/storage`.
pub const EXTERNAL_ROOT: &str = "@external";

/// A drive-root setting that may be passed to the daemon.
pub fn is_valid_root_setting(value: &str) -> bool {
    let value = value.trim();
    value == EXTERNAL_ROOT || is_safe_android_path(value)
}

/// POSIX single-quote quoting: `it's` becomes `'it'\''s'`.
pub fn sh_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Wraps `command` so that it runs with the given access.
pub fn wrap(access: Access, command: &str) -> String {
    match access {
        Access::Shell { .. } => command.to_string(),
        Access::Root(SuStyle::DashC) => format!("su -c {}", sh_quote(command)),
        Access::Root(SuStyle::Uid0) => format!("su 0 sh -c {}", sh_quote(command)),
        Access::Root(SuStyle::RootName) => format!("su root sh -c {}", sh_quote(command)),
    }
}

/// The daemon command line (before `wrap`).
pub fn server_command(root: Option<&str>, token: &str) -> String {
    let mut command = format!("{SERVER_PATH} --token {token}");
    if let Some(root) = root.map(str::trim).filter(|root| !root.is_empty()) {
        command.push_str(" --root ");
        command.push_str(&sh_quote(root));
    }
    command
}

/// Kills leftover daemons. `pidof` and `killall` are both tried because the
/// Android toolbox on old TV boxes lacks one or the other. (`pkill -f` is
/// deliberately not used: it would also match the shell running this script.)
pub const KILL_SCRIPT: &str = "for p in $(pidof androidfs_server 2>/dev/null); do kill -9 $p 2>/dev/null; done; killall androidfs_server 2>/dev/null; true";

/// Reads a user id from `id -u` (`0`) or `id` (`uid=0(root) gid=0(root)`) output.
pub fn parse_uid(output: &str) -> Option<u32> {
    let text = output.trim();
    if let Some(rest) = text.strip_prefix("uid=") {
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        return digits.parse().ok();
    }
    let first = text.lines().next()?.trim();
    if !first.is_empty() && first.chars().all(|ch| ch.is_ascii_digit()) {
        first.parse().ok()
    } else {
        None
    }
}

/// Turns user input like `192.168.0.5` into `192.168.0.5:5555`; rejects anything
/// that is not a plain host name or address with an optional port.
pub fn normalize_adb_host(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty()
        || text.len() > 64
        || !text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '-' | '_'))
    {
        return None;
    }
    match text.matches(':').count() {
        0 => Some(format!("{text}:5555")),
        1 => {
            let (host, port) = text.split_once(':')?;
            if host.is_empty() || port.parse::<u16>().ok()? == 0 {
                None
            } else {
                Some(text.to_string())
            }
        }
        _ => None,
    }
}

/// `adb connect` exits with 0 even when it fails; only its text tells.
pub fn connect_succeeded(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    (lower.contains("connected to") || lower.contains("already connected"))
        && !lower.contains("failed")
        && !lower.contains("cannot")
        && !lower.contains("unable")
}

/// A short random hex token so the host can recognise the daemon it started
/// (and not a stale one that is still bound to the port).
pub fn make_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = nanos ^ (u128::from(std::process::id()) << 64) ^ 0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C834;
    format!("{:016x}", (mixed ^ (mixed >> 64)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        assert!(is_safe_android_path("/storage/emulated/0"));
        assert!(is_safe_android_path("/storage/1A2B-3C4D/My Files (old)"));
        assert!(is_safe_android_path("/"));
        assert!(!is_safe_android_path("storage"));
        assert!(!is_safe_android_path("/sdcard/a;rm -rf /"));
        assert!(!is_safe_android_path("/sdcard/$(id)"));
        assert!(!is_safe_android_path("/sdcard/it's"));
        assert!(!is_safe_android_path("/sdcard/`id`"));
        assert!(!is_safe_android_path("/sdcard/../data"));
    }

    #[test]
    fn root_settings() {
        assert!(is_valid_root_setting("@external"));
        assert!(is_valid_root_setting("/storage/emulated/0"));
        assert!(!is_valid_root_setting("@other"));
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/sdcard"), "'/sdcard'");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn server_command_line() {
        assert_eq!(
            server_command(None, "abc"),
            "/data/local/tmp/androidfs_server --token abc"
        );
        assert_eq!(
            server_command(Some("  "), "abc"),
            "/data/local/tmp/androidfs_server --token abc"
        );
        assert_eq!(
            server_command(Some("/storage/My Disk"), "abc"),
            "/data/local/tmp/androidfs_server --token abc --root '/storage/My Disk'"
        );
    }

    #[test]
    fn su_wrapping() {
        let cmd = server_command(Some("/a b"), "t");
        assert_eq!(wrap(Access::Shell { uid_zero: false }, &cmd), cmd);
        assert_eq!(
            wrap(Access::Root(SuStyle::DashC), "id -u"),
            "su -c 'id -u'"
        );
        assert_eq!(
            wrap(Access::Root(SuStyle::Uid0), "id -u"),
            "su 0 sh -c 'id -u'"
        );
        assert_eq!(
            wrap(Access::Root(SuStyle::RootName), "id -u"),
            "su root sh -c 'id -u'"
        );
        // nested quoting survives: the inner single quotes are escaped
        assert_eq!(
            wrap(Access::Root(SuStyle::DashC), &cmd),
            "su -c '/data/local/tmp/androidfs_server --token t --root '\\''/a b'\\'''"
        );
        // $( ) in the kill script must not be expanded by the outer shell
        assert!(wrap(Access::Root(SuStyle::DashC), KILL_SCRIPT).starts_with("su -c 'for p in $(pidof"));
    }

    #[test]
    fn uid_parsing() {
        assert_eq!(parse_uid("0\r\n"), Some(0));
        assert_eq!(parse_uid("2000\n"), Some(2000));
        assert_eq!(parse_uid("uid=0(root) gid=0(root) groups=0(root)"), Some(0));
        assert_eq!(parse_uid("uid=2000(shell) gid=2000(shell)"), Some(2000));
        assert_eq!(parse_uid("/system/bin/sh: su: not found"), None);
        assert_eq!(parse_uid(""), None);
    }

    #[test]
    fn mode_round_trip() {
        for mode in [AccessMode::Auto, AccessMode::Shell, AccessMode::Root] {
            assert_eq!(AccessMode::parse(mode.as_str()), mode);
        }
        assert_eq!(AccessMode::parse("garbage"), AccessMode::Auto);
    }

    #[test]
    fn tokens_are_hex_and_nonempty() {
        let token = make_token();
        assert_eq!(token.len(), 16);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn adb_hosts() {
        assert_eq!(normalize_adb_host("192.168.0.5").as_deref(), Some("192.168.0.5:5555"));
        assert_eq!(normalize_adb_host(" box.local:6000 ").as_deref(), Some("box.local:6000"));
        assert_eq!(normalize_adb_host("a:b:c"), None);
        assert_eq!(normalize_adb_host("1.2.3.4:0"), None);
        assert_eq!(normalize_adb_host("1.2.3.4; rm"), None);
        assert_eq!(normalize_adb_host(""), None);
        assert!(connect_succeeded("connected to 192.168.0.5:5555"));
        assert!(connect_succeeded("already connected to 192.168.0.5:5555\r\n"));
        assert!(!connect_succeeded("failed to connect to '192.168.0.5:5555': Connection refused"));
        assert!(!connect_succeeded("cannot connect to 1.2.3.4:5555: timed out (10060)"));
    }

    #[test]
    fn access_labels() {
        assert!(!Access::Shell { uid_zero: false }.is_root());
        assert!(Access::Shell { uid_zero: true }.is_root());
        assert!(Access::Root(SuStyle::DashC).is_root());
        assert_eq!(Access::Root(SuStyle::DashC).label(), "root (su -c)");
    }
}
