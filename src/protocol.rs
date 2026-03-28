// Reverse-engineered from the decompiled Android app in:
// com.elitech.environment.en/sources/com/elitech/environment/en/util/BleSendParse.java
// com.elitech.environment.en/sources/defpackage/si1.java
//
// Quick check against the decompiled app shows the framing helper is shared
// across multiple device activities; model-specific command ids and payload
// layouts live in the sensor profiles instead.
use anyhow::{bail, Result};

// Request frame
// +------+------+------+------+----------------------+-----------+----------+
// | 0x5a | 0xa5 | len  | cmd  | guid[10]             | body[..]  | checksum |
// +------+------+------+------+----------------------+-----------+----------+
//                        len = body.len() + 11
//                        checksum = sum(bytes[2..last))
//
// Response frame
// +------+------+--------+------+----------------------+-----------+----------+
// | 0xd5 | 0xc8 | status | cmd  | guid[10]             | payload   | checksum |
// +------+------+--------+------+----------------------+-----------+----------+
//                           checksum = sum(bytes[2..last))
const REQ_HEAD_1: u8 = 0x5a;
const REQ_HEAD_2: u8 = 0xa5;
const RESP_HEAD_1: u8 = 0xd5;
const RESP_HEAD_2: u8 = 0xc8;

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

fn guid_to_bytes(guid: &str) -> Result<Vec<u8>> {
    if guid.len() % 2 != 0 {
        bail!("GUID must contain an even number of digits");
    }
    if !guid.chars().all(|c| c.is_ascii_digit()) {
        bail!("GUID must contain only decimal digits");
    }
    let mut out = Vec::with_capacity(guid.len() / 2);
    for i in (0..guid.len()).step_by(2) {
        out.push(guid[i..i + 2].parse::<u8>()?);
    }
    Ok(out)
}

pub fn guid_bytes(guid: &str) -> Result<Vec<u8>> {
    let guid_bytes = guid_to_bytes(guid)?;
    if guid_bytes.len() != 10 {
        bail!(
            "GUID must decode to exactly 10 bytes (20 digits), got {} bytes",
            guid_bytes.len()
        );
    }
    Ok(guid_bytes)
}

pub fn build_request(cmd: u8, guid: &str, body: &[u8]) -> Result<Vec<u8>> {
    let guid_bytes = guid_bytes(guid)?;

    let mut frame = Vec::with_capacity(16 + body.len());
    frame.push(REQ_HEAD_1);
    frame.push(REQ_HEAD_2);
    frame.push((body.len() as u8).wrapping_add(11));
    frame.push(cmd);
    frame.extend_from_slice(&guid_bytes);
    frame.extend_from_slice(body);
    frame.push(checksum(&frame[2..]));
    Ok(frame)
}

pub fn verify_response(frame: &[u8]) -> bool {
    frame.len() >= 5
        && frame[0] == RESP_HEAD_1
        && frame[1] == RESP_HEAD_2
        && checksum(&frame[2..frame.len() - 1]) == frame[frame.len() - 1]
}

pub fn response_matches_guid(frame: &[u8], guid: &str) -> Result<bool> {
    let expected_guid = guid_bytes(guid)?;
    Ok(frame.len() >= 14 && frame[4..14] == expected_guid)
}

#[cfg(test)]
mod tests {
    use super::{build_request, response_matches_guid, verify_response};

    const CMD_GET_HISTORY_META: u8 = 0x87;
    const CMD_GET_HISTORY_CHUNK: u8 = 0x88;

    fn hex_spaced(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn builds_expected_history_meta_request() {
        let frame = build_request(CMD_GET_HISTORY_META, "90158797465673526885", &[1]).unwrap();
        assert_eq!(hex_spaced(&frame), "5a a5 0c 87 5a 0f 57 61 2e 38 49 34 44 55 01 31");
    }

    #[test]
    fn accepts_known_history_responses() {
        let meta = [
            0xd5, 0xc8, 0x12, 0x87, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49, 0x34, 0x44, 0x55,
            0x01, 0x00, 0x02, 0x00, 0x12, 0x0b, 0x16, 0x6c,
        ];
        let chunk = [
            0xd5, 0xc8, 0x02, CMD_GET_HISTORY_CHUNK, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49,
            0x34, 0x44, 0x55, 0x20, 0x00, 0x12, 0x07, 0xea, 0x03, 0x1b, 0x15, 0x00, 0x00, 0x00,
            0x00, 0xe9, 0x01, 0xc0, 0x00, 0x00, 0x03, 0x7e, 0x00, 0x00, 0x07, 0xea, 0x03, 0x1b,
            0x15, 0x1e, 0x00, 0x00, 0x00, 0xf2, 0x01, 0xb3, 0x00, 0x00, 0x04, 0x67, 0x00, 0x00,
            0xfb,
        ];

        assert!(verify_response(&meta));
        assert!(verify_response(&chunk));
    }

    #[test]
    fn rejects_invalid_guids() {
        let cases = [
            ("001122334455667788gg", "GUID must contain only decimal digits"),
            ("0011223344556677889", "GUID must contain an even number of digits"),
            (
                "0011223344556677889900",
                "GUID must decode to exactly 10 bytes (20 digits)",
            ),
        ];

        for (guid, expected) in cases {
            let err = build_request(CMD_GET_HISTORY_META, guid, &[1]).unwrap_err();
            assert!(err.to_string().contains(expected), "guid {guid} should mention {expected}");
        }
    }

    #[test]
    fn rejects_invalid_packets() {
        let valid = [
            0xd5, 0xc8, 0x12, 0x87, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49, 0x34, 0x44, 0x55,
            0x01, 0x00, 0x02, 0x00, 0x12, 0x0b, 0x16, 0x6c,
        ];
        let mut bad_checksum = valid;
        bad_checksum[21] ^= 0x01;
        let mut bad_header = valid;
        bad_header[0] = 0x00;

        assert!(!verify_response(&valid[..4]));
        assert!(!verify_response(&bad_checksum));
        assert!(!verify_response(&bad_header));
    }

    #[test]
    fn matches_response_guid() {
        let frame = [
            0xd5, 0xc8, 0x12, 0x87, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49, 0x34, 0x44, 0x55,
            0x01, 0x00, 0x02, 0x00, 0x12, 0x0b, 0x16, 0x6c,
        ];
        assert!(response_matches_guid(&frame, "90158797465673526885").unwrap());
        assert!(!response_matches_guid(&frame, "00112233445566778899").unwrap());
    }
}
