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
}
