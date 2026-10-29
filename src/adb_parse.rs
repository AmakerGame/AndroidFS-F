/// Pulls serial numbers of devices in the `device` state out of `adb devices` output.
///
/// Unauthorized, offline and empty lines are ignored. A trailing `\r` (CRLF from
/// Windows adb) is stripped by [`str::lines`] plus an extra trim, so a phone is
/// still detected.
pub fn parse_device_serials(stdout: &str) -> Vec<String> {
    let mut serials = Vec::new();
    for line in stdout.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with("List of") || line.starts_with('*') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else {
            continue;
        };
        let Some(state) = parts.next() else {
            continue;
        };
        if state == "device" {
            serials.push(serial.to_string());
        }
    }
    serials
}

/// Devices adb sees but cannot use yet: `(serial, state)` for `unauthorized`,
/// `offline` and `authorizing`.
pub fn parse_problem_devices(stdout: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in stdout.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with("List of") || line.starts_with('*') {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(serial), Some(state)) = (parts.next(), parts.next()) {
            if matches!(state, "unauthorized" | "offline" | "authorizing") {
                found.push((serial.to_string(), state.to_string()));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::parse_device_serials;

    #[test]
    fn parses_crlf_and_skips_unauthorized() {
        let out = "List of devices attached\r\n\
                   emulator-5554\tunauthorized\r\n\
                   R58N123ABC\tdevice\r\n\
                   offline-one\toffline\r\n\
                   SECOND123\tdevice usb:1-2 product:foo\r\n";
        assert_eq!(
            parse_device_serials(out),
            vec!["R58N123ABC".to_string(), "SECOND123".to_string()]
        );
    }

    #[test]
    fn empty_when_nothing_attached() {
        assert!(parse_device_serials("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn finds_problem_devices() {
        let out = "List of devices attached\nAAA\tunauthorized\nBBB\tdevice\r\n192.0.2.9:5555\toffline\n";
        assert_eq!(
            super::parse_problem_devices(out),
            vec![
                ("AAA".to_string(), "unauthorized".to_string()),
                ("192.0.2.9:5555".to_string(), "offline".to_string())
            ]
        );
    }
}
